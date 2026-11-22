//! Login Asura : au lieu de recreer la signature native du launcher
//! (attestation du fichier DataCenter), on pilote le VRAI launcher deja
//! lance, via son port de debug CEF, pour qu'il produise lui-meme un OTP
//! frais (`window.l2w_getOTP()`). Le launcher doit donc tourner, avec
//! `--remote-debugging-port` actif, au moment ou le bot se connecte.
//!
//! Flux reel observe dans le launcher (voir data/ ou la discussion) :
//!   POST /launcher/api/game_otp/challenge -> { nonce }
//!   window.launcher.file.createDataCenterAttestation(dataCenter, nonce, version) [natif]
//!   POST /launcher/api/game_otp { osid, nonce, dataCenter, version, dataCenterHash, signature } -> { otp }
//! On ne reimplemente rien de ca : on appelle l2w_getOTP() tel quel.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};

/// Valeur reelle du champ `patchVersion` de C_LOGIN_ARBITER, confirmee par
/// une capture du vrai client (le launcher annonce "Asura 100.02 #91",
/// mais le client envoie bien 2106 au patch #91, pas 10002).
pub const PATCH_VERSION: u32 = 2106;

/// Comment demarrer le launcher nous-memes s'il ne tourne pas deja. On ne
/// reimplemente jamais son login (captcha) ni sa signature native : on se
/// contente de lancer le VRAI binaire, avec Wine, dans sa bottle dediee.
/// Le login initial (captcha) reste une etape manuelle, une seule fois ;
/// la session du launcher semble ensuite persister par cookie.
pub struct LaunchConfig {
    pub wine: String,
    pub bottle: String,
    pub launcher_exe: String,
    pub cdp_port: u16,
}

/// Processus de launcher demarre par nous : on le ferme quand le bot
/// s'arrete (succes ou erreur), via Drop, puisque c'est nous qui l'avons
/// ouvert. None si on a reutilise un launcher deja en route.
pub struct LaunchedLauncher {
    child: Child,
}

impl Drop for LaunchedLauncher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Juste un test de connexion TCP : suffisant pour savoir si le port de
/// debug accepte deja des connexions, sans se soucier de l'API de timeout
/// d'ureq pour une simple verification de disponibilite.
fn cdp_is_up(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(mut addrs) = (host, port).to_socket_addrs() else { return false };
    let Some(addr) = addrs.next() else { return false };
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

/// S'assure que le launcher repond sur son port de debug. S'il n'y repond
/// pas encore et qu'une `LaunchConfig` est fournie, le demarre (via Wine,
/// dans sa bottle) et attend qu'il soit pret. Retourne le processus lance,
/// a garder en vie (et donc a ne pas laisser tomber) le temps du bot : il
/// se ferme tout seul au Drop.
pub fn ensure_launcher_running(
    cdp_host: &str,
    launch: Option<&LaunchConfig>,
    ready_timeout: Duration,
) -> Result<Option<LaunchedLauncher>> {
    let Some(config) = launch else {
        return Ok(None);
    };
    if cdp_is_up(cdp_host, config.cdp_port) {
        println!("Asura : launcher deja demarre (port {}), on le reutilise.", config.cdp_port);
        return Ok(None);
    }
    println!(
        "Asura : demarrage du launcher ({}) dans la bottle {}...",
        config.launcher_exe, config.bottle
    );
    let child = Command::new(&config.wine)
        .arg(&config.launcher_exe)
        .args([
            "--disable-gpu",
            "--disable-gpu-compositing",
            "--in-process-gpu",
            "--disable-d3d11",
            &format!("--remote-debugging-port={}", config.cdp_port),
        ])
        .env("CX_BOTTLE", &config.bottle)
        .env("WINEDEBUG", "-all")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("lancement de {} via {}", config.launcher_exe, config.wine))?;

    let start = Instant::now();
    while !cdp_is_up(cdp_host, config.cdp_port) {
        if start.elapsed() > ready_timeout {
            bail!(
                "le launcher ne repond pas sur le port {} apres {:?} ; verifie qu'il demarre bien (fenetre visible ?) et, si tu etais deja connecte avec ce compte, que la session n'a pas expire (il faudra alors te reconnecter une fois a la main)",
                config.cdp_port,
                ready_timeout
            );
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    println!("Asura : launcher pret (port {}).", config.cdp_port);
    Ok(Some(LaunchedLauncher { child }))
}

/// Identifiants de connexion obtenus via le launcher. `osid` sert a la
/// decouverte du relais local ; pour le champ `name` de C_LOGIN_ARBITER,
/// une vraie capture montre que c'est l'identifiant de compte du SITE
/// (celui de /launcher/api/login, visible dans le launcher), pas l'osid.
/// On ne sait pas encore le lire automatiquement : passe-le via --account.
pub struct AsuraTicket {
    pub osid: String,
    pub otp: String,
    /// Adresse reelle a utiliser pour la connexion TCP. Le launcher ne
    /// fait PAS se connecter le jeu a l'IP publique de la serverlist :
    /// il ouvre un relais local (ip=127.0.0.1, port ephemere, visible
    /// via window.l2w_getServerList()) qui tunnelise vers le vrai
    /// serveur (ExitLag ou equivalent). Ce relais doit rester actif
    /// (donc le launcher doit rester ouvert) pendant toute la session.
    /// None si la decouverte a echoue : il faut alors utiliser une
    /// adresse fixe en repli.
    pub server: Option<String>,
}

/// Extrait le contenu du premier `<tag>...</tag>` d'un XML simple (la
/// serverlist n'a pas besoin d'un vrai parseur XML).
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

/// Demande au launcher l'adresse reelle du relais local (ip/port) a
/// utiliser pour la connexion TCP, via window.l2w_getServerList().
fn fetch_server_address(client: &mut CdpClient) -> Result<Option<String>> {
    let expression = r#"
        (async () => {
            const url = window.l2w_getServerList();
            if (!url) return null;
            const response = await fetch(url);
            return await response.text();
        })()
    "#;
    let body = match client.evaluate(expression)? {
        Value::String(s) => s,
        _ => return Ok(None),
    };
    let ip = extract_xml_tag(&body, "ip");
    let port = extract_xml_tag(&body, "port");
    Ok(match (ip, port) {
        (Some(ip), Some(port)) => Some(format!("{ip}:{port}")),
        _ => None,
    })
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    ws_url: Option<String>,
    url: Option<String>,
}

fn connect_launcher_cdp(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<CdpClient> {
    let list_url = format!("http://{cdp_host}:{cdp_port}/json/list");
    let body = ureq::get(&list_url)
        .call()
        .with_context(|| format!("impossible de joindre le port de debug du launcher ({list_url}) ; verifie qu'il tourne avec --remote-debugging-port={cdp_port}"))?
        .body_mut()
        .read_to_string()
        .context("lecture de /json/list")?;
    let pages: Vec<PageInfo> = serde_json::from_str(&body).context("parsing /json/list")?;
    let page = pages
        .into_iter()
        .find(|p| p.kind == "page" && p.ws_url.is_some())
        .context("aucune page trouvee sur le port de debug du launcher")?;
    if let Some(url) = &page.url {
        if !url.contains("tera-asura.com") {
            bail!("la page ouverte sur le port de debug ({url}) ne ressemble pas au launcher Asura");
        }
    }
    let ws_url = page.ws_url.context("page sans websocket de debug")?;
    CdpClient::connect(&ws_url, timeout)
}

/// Recupere l'adresse du relais local (127.0.0.1:port) ouvert par le launcher
/// via window.l2w_getServerList(). Sert de transport pour contourner le
/// scrubber FAI quand la connexion directe est bloquee.
pub fn fetch_relay_server(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<Option<String>> {
    let mut client = connect_launcher_cdp(cdp_host, cdp_port, timeout)?;
    fetch_server_address(&mut client)
}

const KEEP_TUNNEL_PATCH: &str = r#"(()=>{
const el=window.launcher&&window.launcher.exitlag;
if(!el)return"no-bridge";
if(!el.__origReserveRoutes)el.__origReserveRoutes=el.reserveRoutes.bind(el);
if(!el.__origPublishSls)el.__origPublishSls=el.publishSls.bind(el);
if(!el.__origReleaseRoutes)el.__origReleaseRoutes=el.releaseRoutes.bind(el);
if(!el.__origStop)el.__origStop=el.stop.bind(el);
el.reserveRoutes=async(a)=>{if(el.__cacheRoutes!=null)return el.__cacheRoutes;el.__cacheRoutes=await el.__origReserveRoutes(a);return el.__cacheRoutes;};
el.publishSls=async(a)=>{if(el.__cacheSls!=null)return el.__cacheSls;el.__cacheSls=await el.__origPublishSls(a);return el.__cacheSls;};
el.releaseRoutes=async()=>true;
el.stop=async()=>true;
window.__keepTunnel=true;
return"ok";
})()"#;

const RELEASE_TUNNEL_PATCH: &str = r#"(async()=>{
const el=window.launcher&&window.launcher.exitlag;
if(!el)return"no-bridge";
if(el.__origReserveRoutes){el.reserveRoutes=el.__origReserveRoutes;delete el.__origReserveRoutes;}
if(el.__origPublishSls){el.publishSls=el.__origPublishSls;delete el.__origPublishSls;}
if(el.__origReleaseRoutes){el.releaseRoutes=el.__origReleaseRoutes;delete el.__origReleaseRoutes;}
if(el.__origStop){el.stop=el.__origStop;delete el.__origStop;}
el.__cacheRoutes=null;el.__cacheSls=null;window.__keepTunnel=false;
let r={};
try{r.release=await el.releaseRoutes();}catch(e){r.release=''+e;}
try{r.stop=await el.stop();}catch(e){r.stop=''+e;}
return JSON.stringify(r);
})()"#;

/// Empeche le launcher de demonter son tunnel ExitLag a la fermeture du
/// jeu. Le teardown (`jg.on("gameEnd")` -> `Ag.finishGame()` ->
/// `releaseRoutes()`/`stop()`) est du JS atteignable via
/// `window.launcher.exitlag`. On neutralise `releaseRoutes`/`stop` (no-op)
/// et on rend `reserveRoutes`/`publishSls` idempotents (ils renvoient la
/// session deja ouverte) : le relais local reste actif pour le bot, et un
/// clic Jouer ulterieur reutilise la meme session au lieu d'en reserver une
/// nouvelle (ce qui echouait et bloquait le jeu normal). A rejouer apres un
/// rechargement de la page du launcher.
pub fn keep_tunnel_alive(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<String> {
    let mut client = connect_launcher_cdp(cdp_host, cdp_port, timeout)?;
    match client.evaluate(KEEP_TUNNEL_PATCH)? {
        Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

/// Ping reseau reel du bot vers le serveur de jeu, mesure par ExitLag
/// (`getPrimaryConnectionStatistics`). Renvoie `(rtt_ms, jitter_ms, perte)`
/// ou `None` si aucune connexion active.
pub fn exitlag_ping(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<Option<(u32, u32, bool)>> {
    let mut client = connect_launcher_cdp(cdp_host, cdp_port, timeout)?;
    let expr = "(async()=>{try{const s=await window.launcher.exitlag.getPrimaryConnectionStatistics();if(!s||!s.ok)return null;return JSON.stringify({rtt:s.lastRtt|0,jitter:s.jitter|0,loss:!!s.packetLost});}catch(e){return null;}})()";
    match client.evaluate(expr)? {
        Value::String(s) => {
            let parsed: Value = serde_json::from_str(&s)?;
            let rtt = parsed.get("rtt").and_then(Value::as_u64).unwrap_or(0) as u32;
            let jitter = parsed.get("jitter").and_then(Value::as_u64).unwrap_or(0) as u32;
            let loss = parsed.get("loss").and_then(Value::as_bool).unwrap_or(false);
            Ok(Some((rtt, jitter, loss)))
        }
        _ => Ok(None),
    }
}

/// Annule `keep_tunnel_alive` : restaure les fonctions ExitLag d'origine et
/// libere la session (routes + stop). A appeler pour rendre la main au
/// launcher si l'etat ExitLag est coince.
pub fn release_tunnel(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<String> {
    let mut client = connect_launcher_cdp(cdp_host, cdp_port, timeout)?;
    match client.evaluate(RELEASE_TUNNEL_PATCH)? {
        Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

/// Recupere un OTP frais en pilotant le launcher Asura deja lance sur
/// `cdp_host:cdp_port` (par defaut 127.0.0.1:9222).
pub fn fetch_ticket(cdp_host: &str, cdp_port: u16, timeout: Duration) -> Result<AsuraTicket> {
    let mut client = connect_launcher_cdp(cdp_host, cdp_port, timeout)?;
    let expression = r#"
        (async () => {
            const otp = await window.l2w_getOTP();
            const osid = await window.launcher.os_id();
            return JSON.stringify({ otp, osid });
        })()
    "#;
    let result = client.evaluate(expression)?;
    let payload: Value = serde_json::from_str(
        result
            .as_str()
            .context("window.l2w_getOTP()/os_id() n'a pas renvoye une chaine JSON")?,
    )
    .context("parsing du resultat de l2w_getOTP")?;
    let otp = payload
        .get("otp")
        .and_then(Value::as_str)
        .context("l2w_getOTP() a renvoye null : le launcher n'a pas encore fini de detecter le client installe, reessaie une fois le bouton Jouer actif")?
        .to_string();
    let osid = payload
        .get("osid")
        .and_then(Value::as_str)
        .context("window.launcher.os_id() a renvoye null")?
        .to_string();
    let server = fetch_server_address(&mut client)
        .context("recuperation de l'adresse du relais local (l2w_getServerList)")?;
    Ok(AsuraTicket { osid, otp, server })
}

/// Client WebSocket minimal, juste ce qu'il faut pour piloter
/// Runtime.evaluate sur le protocole Chrome DevTools (frames texte,
/// pas de fragmentation, pas de compression). Pas un client WS general.
struct CdpClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl CdpClient {
    fn connect(ws_url: &str, timeout: Duration) -> Result<Self> {
        let rest = ws_url.strip_prefix("ws://").context("seul ws:// est supporte (le debug CEF ne chiffre pas)")?;
        let (authority, path) = rest.split_once('/').map(|(a, p)| (a, format!("/{p}"))).unwrap_or((rest, "/".into()));
        let (host, port) = authority.split_once(':').context("URL sans port")?;
        let addr = format!("{host}:{port}");
        let stream = TcpStream::connect(&addr).with_context(|| format!("connexion a {addr}"))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let reader_stream = stream.try_clone()?;

        let mut key_bytes = [0u8; 16];
        getrandom::fill(&mut key_bytes).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
        let key = STANDARD.encode(key_bytes);
        let mut writer = stream.try_clone()?;
        write!(
            writer,
            "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )?;

        let mut reader = BufReader::new(reader_stream);
        let mut status_line = String::new();
        reader.read_line(&mut status_line)?;
        if !status_line.contains("101") {
            bail!("handshake websocket refuse : {}", status_line.trim());
        }
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
        }

        Ok(Self { stream, reader, next_id: 0 })
    }

    fn evaluate(&mut self, expression: &str) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({
            "id": id,
            "method": "Runtime.evaluate",
            "params": { "expression": expression, "returnByValue": true, "awaitPromise": true },
        });
        self.send_text(&request.to_string())?;

        loop {
            let text = self.recv_text()?;
            let message: Value = serde_json::from_str(&text)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    bail!("Runtime.evaluate a echoue : {error}");
                }
                let result = &message["result"]["result"];
                if let Some(exception) = message.get("exceptionDetails") {
                    bail!("exception JS : {exception}");
                }
                return Ok(result.get("value").cloned().unwrap_or(Value::Null));
            }
            // Un autre message (evenement Network/Runtime) : on l'ignore et on continue d'attendre la reponse.
        }
    }

    fn send_text(&mut self, text: &str) -> Result<()> {
        let payload = text.as_bytes();
        let mut frame = vec![0x81u8]; // FIN + opcode texte
        let len = payload.len();
        if len < 126 {
            frame.push(0x80 | len as u8); // masque obligatoire cote client
        } else if len < 65536 {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(len as u64).to_be_bytes());
        }
        let mut mask = [0u8; 4];
        getrandom::fill(&mut mask).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
        frame.extend_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame)?;
        Ok(())
    }

    fn recv_text(&mut self) -> Result<String> {
        let mut message = Vec::new();
        loop {
            let mut header = [0u8; 2];
            self.reader.read_exact(&mut header)?;
            let fin = header[0] & 0x80 != 0;
            let opcode = header[0] & 0x0f;
            let masked = header[1] & 0x80 != 0;
            let mut len = (header[1] & 0x7f) as u64;
            if len == 126 {
                let mut ext = [0u8; 2];
                self.reader.read_exact(&mut ext)?;
                len = u16::from_be_bytes(ext) as u64;
            } else if len == 127 {
                let mut ext = [0u8; 8];
                self.reader.read_exact(&mut ext)?;
                len = u64::from_be_bytes(ext);
            }
            let mask = if masked {
                let mut m = [0u8; 4];
                self.reader.read_exact(&mut m)?;
                Some(m)
            } else {
                None
            };
            let mut payload = vec![0u8; len as usize];
            self.reader.read_exact(&mut payload)?;
            if let Some(mask) = mask {
                for (i, b) in payload.iter_mut().enumerate() {
                    *b ^= mask[i % 4];
                }
            }
            match opcode {
                0x1 | 0x0 => message.extend_from_slice(&payload),
                0x8 => bail!("le launcher a ferme la connexion de debug"),
                0x9 => self.send_pong(&payload)?,
                _ => {}
            }
            if fin {
                break;
            }
        }
        String::from_utf8(message).context("frame websocket non-UTF8")
    }

    fn send_pong(&mut self, payload: &[u8]) -> Result<()> {
        let mut frame = vec![0x8au8, 0x80 | payload.len() as u8];
        let mask = [0u8; 4];
        frame.extend_from_slice(&mask);
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame)?;
        Ok(())
    }
}

/// Calcule l'en-tete Sec-WebSocket-Accept attendu (sert surtout de
/// documentation / validation ; non verifie strictement ci-dessus pour
/// rester tolerant aux implementations CEF).
#[allow(dead_code)]
fn expected_accept(key: &str) -> String {
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(GUID.as_bytes());
    STANDARD.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_matches_rfc6455_example() {
        // Exemple officiel de la RFC 6455 section 1.3.
        assert_eq!(expected_accept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }
}
