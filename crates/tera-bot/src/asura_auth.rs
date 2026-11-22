use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub const BASE: &str = "https://tera-asura.com";
pub const DATA_CENTER: &str = "EUR";
pub const CLIENT_VERSION: &str = "Asura 100.02 #91";
pub const DATA_CENTER_HASH: &str = "bc400568d58d46e0f3dbbabd76b1f8ea";
pub const HMAC_KEY_HEX: &str = "dc3ebe3b152c2e79176bd68884aac7204384bb7fe673396c81fd949ef0bb90a8";

const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/115.0.0.0 Safari/537.36";

fn is_js_challenge(body: &str) -> bool {
    body.contains("__js_p_") && body.contains("get_jhash")
}

fn is_xml(body: &str) -> bool {
    body.trim_start().starts_with('<') && !is_js_challenge(body)
}

fn get_jhash(code: i64) -> i64 {
    let mut x: i64 = 123456789;
    let mut k: i64 = 0;
    for i in 0..1_677_696i64 {
        x = ((x + code) ^ (x + (x % 3) + (x % 17) + code) ^ i) % 16_776_960;
        if x % 117 == 0 {
            k = (k + 1) % 1111;
        }
    }
    k
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod challenge_tests {
    use super::{get_jhash, percent_encode};

    #[test]
    fn jhash_matches_reference() {
        assert_eq!(get_jhash(702), 184);
    }

    #[test]
    fn ua_encodes_like_fixed_encode_uri_component() {
        assert_eq!(percent_encode("a b(c)"), "a%20b%28c%29");
    }
}

pub struct Session {
    cookies: BTreeMap<String, String>,
    agent: ureq::Agent,
}

pub struct Authenticated {
    pub account_name: String,
    pub user_id: i64,
}

impl Session {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build();
        Self {
            cookies: BTreeMap::new(),
            agent: config.new_agent(),
        }
    }

    fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn absorb(&mut self, resp: &ureq::http::Response<ureq::Body>) {
        for value in resp.headers().get_all("set-cookie") {
            let Ok(raw) = value.to_str() else { continue };
            let Some(pair) = raw.split(';').next() else { continue };
            if let Some((name, val)) = pair.split_once('=') {
                self.cookies.insert(name.trim().to_string(), val.trim().to_string());
            }
        }
    }

    fn post_json(&mut self, path: &str, body: Value) -> Result<Value> {
        let url = format!("{BASE}{path}");
        let mut last = String::new();
        for attempt in 0..5 {
            let mut request = self
                .agent
                .post(&url)
                .header("User-Agent", UA)
                .header("Origin", BASE)
                .header("Referer", BASE);
            let jar = self.cookie_header();
            if !jar.is_empty() {
                request = request.header("Cookie", &jar);
            }
            let mut resp = request
                .send_json(body.clone())
                .with_context(|| format!("POST {path}"))?;
            let status = resp.status().as_u16();
            let location = resp
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            self.absorb(&resp);
            let text = resp
                .body_mut()
                .read_to_string()
                .with_context(|| format!("lecture de {path}"))?;
            if let Ok(value) = serde_json::from_str(&text) {
                return Ok(value);
            }
            last = format!(
                "status={status} location={location} cookies=[{}] body({}o)={}",
                self.cookies.keys().cloned().collect::<Vec<_>>().join(","),
                text.len(),
                text.chars().take(120).collect::<String>()
            );
            if attempt < 4 && self.is_challenge(status, &text) {
                self.solve_js_challenge()
                    .with_context(|| format!("resolution du challenge anti-bot avant {path}"))?;
                std::thread::sleep(Duration::from_millis(1100));
                continue;
            }
            bail!("JSON invalide de {path} : {last}");
        }
        bail!("JSON invalide de {path} apres resolutions : {last}")
    }

    fn is_challenge(&self, status: u16, body: &str) -> bool {
        is_js_challenge(body) || (300..400).contains(&status) || self.cookies.contains_key("__js_p_")
    }

    fn solve_js_challenge(&mut self) -> Result<()> {
        let params = self
            .cookies
            .get("__js_p_")
            .context("challenge anti-bot sans cookie __js_p_")?
            .clone();
        let code: i64 = params
            .split(',')
            .next()
            .and_then(|field| field.trim().parse().ok())
            .context("code de challenge illisible dans __js_p_")?;
        self.cookies.insert("__jhash_".to_string(), get_jhash(code).to_string());
        self.cookies.insert("__jua_".to_string(), percent_encode(UA));
        Ok(())
    }

    pub fn web_auth(&mut self, poll_timeout: Duration) -> Result<Authenticated> {
        let start = self.post_json("/launcher/api/web-auth/start", json!({}))?;
        let url = start
            .get("url")
            .and_then(Value::as_str)
            .context("web-auth/start sans url")?
            .to_string();
        let code = start
            .get("code")
            .and_then(Value::as_str)
            .context("web-auth/start sans code")?
            .to_string();
        println!("Asura : ouvre cette URL et confirme la connexion sur le site :\n  {url}");
        let _ = std::process::Command::new("open").arg(&url).status();

        let deadline = Instant::now() + poll_timeout;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(3));
            let poll = self.post_json("/launcher/api/web-auth/poll", json!({ "code": code }))?;
            if poll.get("pending").and_then(Value::as_bool) == Some(true) {
                print!(".");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                continue;
            }
            println!();
            return Self::finish(&poll);
        }
        bail!("web-auth : delai depasse, confirmation jamais recue");
    }

    fn finish(payload: &Value) -> Result<Authenticated> {
        let user = payload
            .get("user")
            .or_else(|| payload.pointer("/data/user"))
            .context(format!("reponse d'auth sans 'user' : {payload}"))?;
        let user_id = user
            .get("id")
            .and_then(Value::as_i64)
            .context("user sans id")?;
        let account_name = user
            .get("id")
            .map(|v| v.to_string())
            .unwrap_or_default();
        println!("Asura : connecte (userId {user_id}).");
        Ok(Authenticated { account_name, user_id })
    }

    pub fn is_osid_verified(&mut self, osid: i64) -> Result<bool> {
        let info = self.post_json("/launcher/api/check_osid", json!({ "osid": osid }))?;
        println!("Asura : check_osid -> {info}");
        Ok(info.get("isVerified").and_then(Value::as_bool).unwrap_or(false))
    }

    pub fn send_osid_code(&mut self, osid: i64) -> Result<()> {
        let resp = self.post_json("/launcher/api/send_new_otp_code", json!({ "osid": osid }))?;
        if resp.get("success").and_then(Value::as_bool) != Some(true) {
            bail!("send_new_otp_code a echoue : {resp}");
        }
        Ok(())
    }

    pub fn verify_osid(&mut self, osid: i64, otp_token: &str) -> Result<()> {
        let resp = self.post_json(
            "/launcher/api/verify_osid",
            json!({ "osid": osid, "otpToken": otp_token }),
        )?;
        if resp.get("success").and_then(Value::as_bool) != Some(true) {
            bail!("verify_osid refuse : {resp}");
        }
        Ok(())
    }

    pub fn challenge(&mut self) -> Result<String> {
        let challenge = self.post_json("/launcher/api/game_otp/challenge", json!({}))?;
        challenge
            .get("nonce")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("game_otp/challenge sans nonce")
    }

    pub fn game_otp_raw(&mut self, osid: i64, nonce: &str, signature: &str) -> Result<Value> {
        self.post_json(
            "/launcher/api/game_otp",
            json!({
                "osid": osid,
                "nonce": nonce,
                "dataCenter": DATA_CENTER,
                "version": CLIENT_VERSION,
                "dataCenterHash": DATA_CENTER_HASH,
                "signature": signature,
            }),
        )
    }

    pub fn game_otp(&mut self, osid: i64) -> Result<String> {
        let nonce = self.challenge()?;
        let signature = data_center_attestation_signature(&nonce);
        let otp = self.game_otp_raw(osid, &nonce, &signature)?;
        otp.get("otp")
            .and_then(Value::as_str)
            .map(str::to_string)
            .with_context(|| format!("game_otp sans otp : {otp}"))
    }

    pub fn probe_signature(&mut self, osid: i64, datacenter_path: &str) -> Result<()> {
        use sha2::{Digest, Sha256};
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let key: Vec<u8> = (0..HMAC_KEY_HEX.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&HMAC_KEY_HEX[i..i + 2], 16).unwrap())
            .collect();
        let mut ikey = [0x36u8; 64];
        let mut okey = [0x5cu8; 64];
        for (i, &k) in key.iter().enumerate() {
            ikey[i] ^= k;
            okey[i] ^= k;
        }
        let file = std::fs::read(datacenter_path)
            .with_context(|| format!("lecture du DataCenter {datacenter_path}"))?;
        let mut inner_file = Sha256::new();
        inner_file.update(ikey);
        inner_file.update(&file);
        let hmac = |with_file: bool, parts: &[&[u8]]| -> String {
            let mut inner = if with_file {
                inner_file.clone()
            } else {
                let mut h = Sha256::new();
                h.update(ikey);
                h
            };
            for p in parts {
                inner.update(p);
            }
            let id = inner.finalize();
            let mut outer = Sha256::new();
            outer.update(okey);
            outer.update(id);
            hex(&outer.finalize())
        };
        let p = b"|".as_slice();
        let dc = DATA_CENTER.as_bytes();
        let ver = CLIENT_VERSION.as_bytes();
        let dch = DATA_CENTER_HASH.as_bytes();
        for round in 0..16 {
            let nonce = self.challenge()?;
            let nb = nonce.as_bytes();
            let (label, sig): (&str, String) = match round {
                0 => ("nonce|dc|ver|dch", hmac(false, &[nb, p, dc, p, ver, p, dch])),
                1 => ("dc|nonce|ver|dch", hmac(false, &[dc, p, nb, p, ver, p, dch])),
                2 => ("nonce|dch|dc|ver", hmac(false, &[nb, p, dch, p, dc, p, ver])),
                3 => ("dch|nonce|dc|ver", hmac(false, &[dch, p, nb, p, dc, p, ver])),
                4 => ("dc|ver|dch|nonce", hmac(false, &[dc, p, ver, p, dch, p, nb])),
                5 => ("nonce|dch", hmac(false, &[nb, p, dch])),
                6 => ("dch|nonce", hmac(false, &[dch, p, nb])),
                7 => ("nonce|ver|dch", hmac(false, &[nb, p, ver, p, dch])),
                8 => ("dc|nonce|dch|ver", hmac(false, &[dc, p, nb, p, dch, p, ver])),
                9 => ("nonce|dc|dch|ver", hmac(false, &[nb, p, dc, p, dch, p, ver])),
                10 => ("dch|dc|ver|nonce", hmac(false, &[dch, p, dc, p, ver, p, nb])),
                11 => ("ver|dc|nonce|dch", hmac(false, &[ver, p, dc, p, nb, p, dch])),
                12 => ("nonce|dc|ver", hmac(false, &[nb, p, dc, p, ver])),
                13 => ("dc|nonce", hmac(false, &[dc, p, nb])),
                14 => ("nonce|dc", hmac(false, &[nb, p, dc])),
                15 => ("nonce|dc|ver|dch|", hmac(false, &[nb, p, dc, p, ver, p, dch, p])),
                _ => unreachable!(),
            };
            let resp = self.game_otp_raw(osid, &nonce, &sig)?;
            let ok = resp.get("otp").and_then(Value::as_str).is_some();
            println!("[{round:2}] {label:28} -> {}{}", if ok { "OTP! " } else { "" }, resp);
            if ok {
                println!("TROUVE : signature = HMAC-SHA256(cle_launcher, {label})");
                return Ok(());
            }
        }
        println!("Vraie cle HMAC + ces messages : aucun match. Il reste a cerner la composition exacte du buffer HMAC.");
        Ok(())
    }

    fn get_text(&mut self, path: &str) -> Result<String> {
        let url = format!("{BASE}{path}");
        for attempt in 0..3 {
            let mut request = self.agent.get(&url).header("User-Agent", UA).header("Referer", BASE);
            let jar = self.cookie_header();
            if !jar.is_empty() {
                request = request.header("Cookie", &jar);
            }
            let mut resp = request.call().with_context(|| format!("GET {path}"))?;
            let status = resp.status().as_u16();
            self.absorb(&resp);
            let text = resp
                .body_mut()
                .read_to_string()
                .with_context(|| format!("lecture de {path}"))?;
            if attempt < 2 && !is_xml(&text) && self.is_challenge(status, &text) {
                self.solve_js_challenge()
                    .with_context(|| format!("resolution du challenge anti-bot avant {path}"))?;
                std::thread::sleep(Duration::from_millis(1100));
                continue;
            }
            return Ok(text);
        }
        unreachable!()
    }

    pub fn server_address(&mut self) -> Result<String> {
        let xml = self.get_text("/launcher/api/sls?locale=en&exitLag=off")?;
        let ip = tag(&xml, "ip").context("sls sans <ip>")?;
        let port = tag(&xml, "port").context("sls sans <port>")?;
        Ok(format!("{ip}:{port}"))
    }
}

pub fn data_center_attestation_signature(nonce: &str) -> String {
    use sha2::{Digest, Sha256};
    let key: Vec<u8> = (0..HMAC_KEY_HEX.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&HMAC_KEY_HEX[i..i + 2], 16).unwrap())
        .collect();
    let message = format!("{nonce}|{DATA_CENTER}|{CLIENT_VERSION}|{DATA_CENTER_HASH}");
    let mut ikey = [0x36u8; 64];
    let mut okey = [0x5cu8; 64];
    for (i, &k) in key.iter().enumerate() {
        ikey[i] ^= k;
        okey[i] ^= k;
    }
    let mut inner = Sha256::new();
    inner.update(ikey);
    inner.update(message.as_bytes());
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(okey);
    outer.update(inner_digest);
    outer.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}
