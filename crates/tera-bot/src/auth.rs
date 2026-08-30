use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::time::{Duration, Instant};
use std::process::Command;

const CLIENT_ID: &str = "tera-classicplus-live";
const AUTH_BASE: &str = "https://tera-europe-classic.com";

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedAuth {
    pub user_name: String,
    pub user_no: i32,
    pub auth_key: String,
    pub refresh_token: String,
}

impl SavedAuth {
    pub fn account(&self) -> String {
        self.user_no.to_string()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize)]
struct GameTicket {
    #[serde(rename = "UserNo")]
    user_no: i32,
    #[serde(rename = "UserName")]
    user_name: String,
    #[serde(rename = "AuthKey")]
    auth_key: String,
}

fn random_token() -> Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn open_browser(url: &str) {
    if Command::new("open").arg(url).spawn().is_err() {
        eprintln!("Impossible d'ouvrir le navigateur, copie l'URL a la main.");
    }
}

fn mint_ticket(access_token: &str) -> Result<GameTicket> {
    let ticket = ureq::post(&format!("{AUTH_BASE}/api/launcher-auth/game-ticket"))
        .header("Authorization", &format!("Bearer {access_token}"))
        .send_json(json!({ "client_id": CLIENT_ID }))
        .context("game-ticket")?
        .body_mut()
        .read_json::<GameTicket>()?;
    Ok(ticket)
}

fn from_parts(ticket: GameTicket, refresh_token: String) -> SavedAuth {
    SavedAuth {
        user_name: ticket.user_name,
        user_no: ticket.user_no,
        auth_key: ticket.auth_key,
        refresh_token,
    }
}

pub fn login() -> Result<SavedAuth> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let verifier = random_token()?;
    let state = random_token()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));

    let url = url::Url::parse_with_params(
        &format!("{AUTH_BASE}/launcher/authorize"),
        &[
            ("response_type", "code"),
            ("client_id", CLIENT_ID),
            ("redirect_uri", redirect_uri.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("device_label", "tera-bot"),
        ],
    )?;

    open_browser(url.as_str());
    println!("Connecte-toi ici (le navigateur devrait s'ouvrir) :\n{url}\n");
    println!("En attente du retour sur {redirect_uri} ...");

    let code = wait_for_callback(listener, &state)?;
    let token = ureq::post(&format!("{AUTH_BASE}/api/launcher-auth/token"))
        .send_json(json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }))
        .context("echange du code")?
        .body_mut()
        .read_json::<TokenResponse>()?;

    Ok(from_parts(mint_ticket(&token.access_token)?, token.refresh_token))
}

pub fn refresh(path: &Path, saved: &SavedAuth) -> Result<SavedAuth> {
    if saved.refresh_token.is_empty() {
        bail!("pas de refresh_token : lance d'abord `tera-bot --login`");
    }
    let token = ureq::post(&format!("{AUTH_BASE}/api/launcher-auth/refresh"))
        .send_json(json!({
            "grant_type": "refresh_token",
            "client_id": CLIENT_ID,
            "refresh_token": saved.refresh_token,
        }))
        .context("refresh")?
        .body_mut()
        .read_json::<TokenResponse>()?;

    let mut rotated = saved.clone();
    rotated.refresh_token = token.refresh_token.clone();
    save(path, &rotated)?;

    Ok(from_parts(mint_ticket(&token.access_token)?, token.refresh_token))
}

fn wait_for_callback(listener: TcpListener, state: &str) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if Instant::now() >= deadline {
            bail!("timeout d'attente du callback OAuth");
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut line = String::new();
        {
            let mut reader = BufReader::new((&stream).take(8192));
            if reader.read_line(&mut line).is_err() {
                continue;
            }
        }
        let path = line.split_whitespace().nth(1).unwrap_or_default();
        let code = url::Url::parse(&format!("http://127.0.0.1{path}"))
            .ok()
            .filter(|url| url.path() == "/callback")
            .filter(|url| url.query_pairs().any(|(key, value)| key == "state" && value == state))
            .and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "code")
                    .map(|(_, value)| value.into_owned())
            });
        let (status, body) = match &code {
            Some(_) => ("200 OK", "Connecte. Tu peux fermer cet onglet et revenir au bot."),
            None => ("404 Not Found", "Not found"),
        };
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        if let Some(code) = code {
            return Ok(code);
        }
    }
}

pub fn load(path: &Path) -> Result<SavedAuth> {
    let bytes = std::fs::read(path).with_context(|| format!("lecture de {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn save(path: &Path, auth: &SavedAuth) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(auth)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
