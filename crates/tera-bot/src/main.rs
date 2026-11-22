mod auth;
mod arborea;
mod asura;
mod asura_auth;
mod auto;
mod broker;
mod discord;
mod fun;
mod gendef;
mod greet;
mod heal;
mod services;
mod traffic;
mod tui;

static COMPACT: AtomicBool = AtomicBool::new(false);

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::fs::File;
use std::io::{BufRead, ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tui::{UiEvent, UiHandle};

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
static CHAT_LOG: OnceLock<Mutex<File>> = OnceLock::new();
static UI_SINK: OnceLock<mpsc::Sender<UiEvent>> = OnceLock::new();
static RECORD_FILE: OnceLock<Mutex<File>> = OnceLock::new();

fn record_packet(dir: &str, t0: Instant, name: &str, opcode: u16, full: &[u8]) {
    let Some(file) = RECORD_FILE.get() else { return };
    let from = if dir == "->" { "client" } else { "server" };
    let mut hex = String::with_capacity(full.len() * 2);
    for byte in full {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    let line = format!(
        "{{\"at\":{:.3},\"from\":\"{from}\",\"opcode\":{opcode},\"name\":\"{name}\",\"len\":{},\"hex\":\"{hex}\"}}\n",
        t0.elapsed().as_secs_f64(),
        full.len()
    );
    if let Ok(mut file) = file.lock() {
        use std::io::Write;
        let _ = file.write_all(line.as_bytes());
    }
}

fn decode_hex(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    (0..bytes.len() / 2)
        .filter_map(|i| u8::from_str_radix(std::str::from_utf8(&bytes[i * 2..i * 2 + 2]).ok()?, 16).ok())
        .collect()
}

fn replay_capture(path: &PathBuf, opcodes: &OpcodeMap, registry: &Registry) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("lecture de la capture {}", path.display()))?;
    let mut total = 0usize;
    let mut decoded = 0usize;
    let mut no_def = 0usize;
    let mut unknown_op = 0usize;
    let mut seen_unknown: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                logln!("ligne JSON invalide ignoree : {e}");
                continue;
            }
        };
        let from = value.get("from").and_then(serde_json::Value::as_str).unwrap_or("?");
        let at = value.get("at").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
        let hex = value.get("hex").and_then(serde_json::Value::as_str).unwrap_or("");
        let opcode_field = value.get("opcode").and_then(serde_json::Value::as_u64).map(|o| o as u16);
        let name_field = value.get("name").and_then(serde_json::Value::as_str);
        let bytes = decode_hex(hex);
        total += 1;
        let dir = if from == "client" { "->" } else { "<-" };
        let has_header = bytes.len() >= 4
            && u16::from_le_bytes([bytes[0], bytes[1]]) as usize == bytes.len();
        let (opcode, full) = if has_header {
            (u16::from_le_bytes([bytes[2], bytes[3]]), bytes)
        } else {
            let opcode = opcode_field.unwrap_or(0);
            let total_len = (bytes.len() + 4) as u16;
            let mut full = Vec::with_capacity(bytes.len() + 4);
            full.extend_from_slice(&total_len.to_le_bytes());
            full.extend_from_slice(&opcode.to_le_bytes());
            full.extend_from_slice(&bytes);
            (opcode, full)
        };
        let body_len = full.len().saturating_sub(4);
        let resolved = opcodes.name(opcode).or(name_field);
        match resolved {
            None => {
                unknown_op += 1;
                seen_unknown.insert(opcode);
                logln!("[{at:8.3}] {dir} opcode {opcode} INCONNU ({body_len} o)");
            }
            Some(name) => match registry.get(name) {
                None => {
                    no_def += 1;
                    logln!("[{at:8.3}] {dir} {name} ({opcode}) pas de definition ({body_len} o)");
                }
                Some(def) => match tera_protocol::read(def, &full) {
                    Ok(obj) => {
                        decoded += 1;
                        logln!("[{at:8.3}] {dir} {name} ({opcode})");
                        for (k, v) in obj.fields.iter() {
                            logln!("    . {k} = {v:?}");
                        }
                    }
                    Err(e) => logln!("[{at:8.3}] {dir} {name} ({opcode}) decodage impossible : {e}"),
                },
            },
        }
    }
    logln!("\n{total} paquets : {decoded} decodes, {no_def} sans definition, {unknown_op} opcodes inconnus");
    if !seen_unknown.is_empty() {
        let list: Vec<String> = seen_unknown.iter().map(u16::to_string).collect();
        logln!("opcodes inconnus (a mapper pour le patch) : {}", list.join(", "));
    }
    Ok(())
}

fn chat_log_line(channel: u64, author: &str, text: &str) {
    if let Some(file) = CHAT_LOG.get() {
        if let Ok(mut file) = file.lock() {
            let label = match channel {
                1 => "party",
                2 => "guild",
                3 => "area",
                11..=18 => "whisper",
                21 | 25 => "notice",
                _ => "say",
            };
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(file, "{stamp}\t{label}\t{author}\t{text}");
        }
    }
}

fn write_log_file(line: &str) {
    if let Some(file) = LOG_FILE.get() {
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn emit(line: String) {
    write_log_file(&line);
    if let Some(sink) = UI_SINK.get() {
        let _ = sink.send(UiEvent::Act(line));
    } else {
        println!("{line}");
    }
}

fn emit_verbose(line: String) {
    write_log_file(&line);
    if UI_SINK.get().is_none() {
        println!("{line}");
    }
}

macro_rules! vlogln {
    () => { $crate::emit_verbose(String::new()) };
    ($($arg:tt)*) => { $crate::emit_verbose(format!($($arg)*)) };
}

#[macro_export]
macro_rules! logln {
    () => { $crate::emit(String::new()) };
    ($($arg:tt)*) => { $crate::emit(format!($($arg)*)) };
}

#[derive(serde::Deserialize, Default)]
struct BotConfig {
    admins: Option<String>,
    auto: Option<bool>,
    whisper_relay: Option<String>,
    greet: Option<bool>,
    greet_text: Option<String>,
    announce_gquest: Option<bool>,
    announce_interval: Option<u64>,
    announce_text: Option<String>,
    discord_webhook: Option<String>,
    reconnect: Option<bool>,
    auto_respawn: Option<bool>,
    refuse_trade_duel: Option<bool>,
    watchdog_zone: Option<bool>,
    afk_reply: Option<String>,
    tui: Option<bool>,
    log: Option<PathBuf>,
    chat_log: Option<PathBuf>,
    server: Option<String>,
    character: Option<String>,
    listen: Option<u64>,
    asura_osid: Option<i64>,
    asura_native: Option<bool>,
    asura_relay: Option<bool>,
    asura_keep_tunnel: Option<bool>,
    asura_launch: Option<bool>,
    asura_server: Option<String>,
    asura_cdp_host: Option<String>,
    asura_cdp_port: Option<u16>,
    asura_bottle: Option<String>,
    proxy: Option<bool>,
    heal: Option<bool>,
    farm: Option<bool>,
    broker: Option<bool>,
    broker_data: Option<String>,
    broker_arm: Option<bool>,
    pick_character: Option<bool>,
    zones: Option<PathBuf>,
    pocket: Option<String>,
    faq: Option<PathBuf>,
    record: Option<PathBuf>,
    auto_gen_defs: Option<bool>,
    commander: Option<String>,
    boss_alert: Option<bool>,
    auto_parcel: Option<bool>,
    salute: Option<bool>,
    parcel_interval: Option<u64>,
    boss_db: Option<PathBuf>,
    ping_log: Option<PathBuf>,
    activity_log: Option<PathBuf>,
}

fn apply_config(cli: &mut Cli, config: &BotConfig) {
    macro_rules! take {
        ($field:ident) => {
            if let Some(value) = config.$field.clone() {
                cli.$field = value;
            }
        };
    }
    macro_rules! take_opt {
        ($field:ident) => {
            if config.$field.is_some() {
                cli.$field = config.$field.clone();
            }
        };
    }
    take!(admins);
    take!(auto);
    take!(whisper_relay);
    take!(greet);
    take!(greet_text);
    take!(announce_gquest);
    take!(announce_interval);
    take!(announce_text);
    take!(reconnect);
    take!(auto_respawn);
    take!(refuse_trade_duel);
    take!(watchdog_zone);
    take!(tui);
    take!(commander);
    take!(listen);
    take!(asura_osid);
    take!(asura_native);
    take!(asura_relay);
    take!(asura_keep_tunnel);
    take!(asura_launch);
    take!(asura_server);
    take!(asura_cdp_host);
    take!(asura_cdp_port);
    take!(asura_bottle);
    take!(proxy);
    take!(heal);
    take!(farm);
    take!(broker);
    take!(broker_data);
    take!(broker_arm);
    take!(pick_character);
    take_opt!(zones);
    take_opt!(pocket);
    take_opt!(faq);
    take_opt!(record);
    take!(auto_gen_defs);
    take!(boss_alert);
    take!(auto_parcel);
    take!(salute);
    take!(parcel_interval);
    take_opt!(afk_reply);
    take_opt!(discord_webhook);
    take_opt!(log);
    take_opt!(chat_log);
    take_opt!(server);
    take_opt!(character);
    take_opt!(boss_db);
    take_opt!(ping_log);
    take_opt!(activity_log);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Guild,
    Admin,
}

#[derive(Clone)]
enum ReplyTarget {
    Whisper { server: u64, name: String },
    Guild { author: String },
    Local,
}
use tera_protocol::handshake::{random_key, ClientHandshake, MAGIC};
use tera_protocol::session::{Decrypting, Encrypting, KEY_LEN, MODERN};
use tera_protocol::{OpcodeMap, PacketBuffer};
use tera_protocol::Registry;

#[derive(Parser)]
#[command(name = "tera-bot", about = "Client TERA headless : handshake, login, liste de personnages")]
struct Cli {
    #[arg(long, help = "adresse:port du serveur ; si absent, resolu automatiquement depuis la serverlist")]
    server: Option<String>,
    #[arg(long, default_value = "https://launcher.tera-europe.net/classicplus/serverlist.json", help = "URL de la serverlist officielle utilisee quand --server est absent")]
    serverlist: String,
    #[arg(long, help = "nom du serveur a choisir dans la serverlist (sinon le 1er disponible)")]
    server_name: Option<String>,
    #[arg(long, default_value = "chat", help = "onglet TUI affiche au demarrage (chat, logs, map, group, network, world, bag, players, guild)")]
    tab: String,
    #[arg(long, help = "insere un tera-proxy (inspection/mods) entre le bot et --server")]
    proxy: bool,
    #[arg(long, help = "route la connexion au serveur via un proxy SOCKS5 (ex: 127.0.0.1:1080)")]
    socks5: Option<String>,
    #[arg(long, default_value = "41946")]
    account: String,
    #[arg(long, default_value = "0123456789abcdef0123456789abcdef")]
    ticket: String,
    #[arg(long, help = "lit le ticket BRUT depuis un fichier .bin (capture par le shim)")]
    ticket_file: Option<PathBuf>,
    #[arg(long, help = "connexion navigateur (OAuth) pour obtenir/sauver un refresh_token")]
    login: bool,
    #[arg(long, help = "refresh + affiche 'AUTH <compte> <ticket>' et sort (sans se connecter)")]
    print_ticket: bool,
    #[arg(long, help = "fichier d'auth (refresh_token + ticket) ; refresh auto avant connexion")]
    auth_file: Option<PathBuf>,
    #[arg(long, help = "auth Arborea Reborn : refresh le ticket + serveur automatiquement")]
    arborea: bool,
    #[arg(long, help = "login Arborea une seule fois (email/password/captcha) puis sauve le refresh_token")]
    arborea_login: bool,
    #[arg(long)]
    arborea_email: Option<String>,
    #[arg(long)]
    arborea_password: Option<String>,
    #[arg(long, help = "token captcha (header Captcha du login web)")]
    arborea_captcha: Option<String>,
    #[arg(long, default_value = "tera-arborea-auth.json")]
    arborea_auth_file: PathBuf,
    #[arg(long, help = "auth Asura : recupere un OTP frais via le VRAI launcher deja lance (port de debug CEF) ; utilise --account (ton id de compte du SITE, pas l'osid) et force patchVersion=2102")]
    asura: bool,
    #[arg(long, default_value = "127.0.0.1", help = "hote du port de debug du launcher Asura")]
    asura_cdp_host: String,
    #[arg(long, default_value_t = 9222, help = "port de debug du launcher Asura (lance-le avec --remote-debugging-port=<ce port>)")]
    asura_cdp_port: u16,
    #[arg(long, default_value = "91.142.86.57:7701", help = "serveur de jeu Asura (vu dans la serverlist officielle)")]
    asura_server: String,
    #[arg(long, help = "auth Asura SANS launcher : web-auth (confirme sur le site) + OTP natif")]
    asura_native: bool,
    #[arg(long, help = "transport via le relais local du launcher (127.0.0.1) au lieu de l'IP directe (scrubbee) ; le launcher doit tourner avec --remote-debugging-port")]
    asura_relay: bool,
    #[arg(long, help = "diagnostic : essaie plusieurs formules de signature sur game_otp puis sort")]
    asura_probe_sig: bool,
    #[arg(long, default_value = "/Users/anna/Library/Application Support/CrossOver/Bottles/tera/drive_c/Games/TERA Asura/TERA/S1Game/S1Data/DataCenter_Final_EUR.dat", help = "chemin du DataCenter EUR pour le probe de signature")]
    asura_datacenter: String,
    #[arg(long, default_value_t = 1700000001, help = "osid (empreinte machine) envoye a l'API Asura en mode --asura-native")]
    asura_osid: i64,
    #[arg(long, help = "demarre le launcher Asura tout seul (via Wine) s'il ne repond pas deja sur le port de debug ; le premier login (captcha) reste manuel")]
    asura_launch: bool,
    #[arg(long, help = "neutralise via CDP le demontage du tunnel ExitLag du launcher (releaseRoutes/stop) : fermer le jeu ne coupe plus le relais local, le bot tient 24/7")]
    asura_keep_tunnel: bool,
    #[arg(long, help = "restaure les fonctions ExitLag et libere la session (rend la main au launcher si l'etat est coince), puis quitte")]
    asura_release_tunnel: bool,
    #[arg(long, help = "chemin de wine (CrossOver) pour --asura-launch ; defaut : wine de CrossOver")]
    asura_wine: Option<String>,
    #[arg(long, default_value = "tera", help = "nom de la bottle CrossOver du launcher Asura (le launcher unique garde ouvert sur le compte du bot)")]
    asura_bottle: String,
    #[arg(long, help = "chemin (Unix) de launcher.exe pour --asura-launch ; defaut : deduit de --asura-bottle")]
    asura_launcher_exe: Option<String>,
    #[arg(long, default_value = "data/opcodes/protocol.376012.map")]
    opcodes: PathBuf,
    #[arg(long, default_values = ["data/definitions"])]
    definitions: Vec<PathBuf>,
    #[arg(long, default_value_t = 10002, help = "valeur du CHAMP C_LOGIN_ARBITER (build client)")]
    patch_version: u32,
    #[arg(long, default_value_t = 100, help = "patch majeur pour CHOISIR les versions de def (Classic = 100)")]
    major_patch: u32,
    #[arg(long, default_value_t = 6, help = "6 = EUR")]
    language: u32,
    #[arg(long, default_value_t = 376012)]
    version_a: i64,
    #[arg(long, default_value_t = 376001)]
    version_b: i64,
    #[arg(long, help = "nom du perso a selectionner (defaut : le premier)")]
    character: Option<String>,
    #[arg(long, help = "mode pocket healer : suit ce joueur de pres et le garde heal en priorite (bot priest)")]
    pocket: Option<String>,
    #[arg(long, help = "mode farm : active l'attaque et roame vers les mobs proches pour grind la zone tout seul")]
    farm: bool,
    #[arg(long, help = "broker sniper : scan le Trade Broker par paquets, apprend les prix et snipe les sous-cotes (immobile)")]
    broker: bool,
    #[arg(long, default_value = "data/broker", help = "dossier des donnees broker (template, categories, prix, state)")]
    broker_data: String,
    #[arg(long, help = "broker : achats reels actives au demarrage (sinon simulation)")]
    broker_arm: bool,
    #[arg(long, help = "liste les persos a la connexion et demande lequel choisir (prompt avant le TUI)")]
    pick_character: bool,
    #[arg(long, help = "enregistre tous les paquets (les 2 sens) en JSONL dans ce fichier pour analyse/replay")]
    record: Option<PathBuf>,
    #[arg(long, help = "mode hors-ligne : relit un JSONL enregistre, redecode chaque paquet avec les defs courantes, puis sort")]
    replay: Option<PathBuf>,
    #[arg(long, help = "genere en live une .def scaffold tera-toolbox pour chaque paquet inconnu/sans definition")]
    auto_gen_defs: bool,
    #[arg(long, default_value = "data/definitions", help = "dossier ou ecrire les .def auto-generees")]
    defs_out: PathBuf,
    #[arg(long, default_value_t = 9999, help = "numero de version des .def auto-generees (eleve = prioritaire au chargement)")]
    gen_version: u32,
    #[arg(long, help = "mode chat interactif : tape des messages, affiche les S_CHAT recus")]
    chat: bool,
    #[arg(long, help = "interface plein ecran (chat + etat + actions) ; pilote la saisie, canal guilde par defaut")]
    tui: bool,
    #[arg(long, help = "ecrit toute la sortie reseau horodatee dans ce fichier (utile avec --tui)")]
    log: Option<PathBuf>,
    #[arg(long, default_value_t = 0, help = "canal de chat par defaut (0=say, 1=party, 2=guild, 3=area)")]
    chat_channel: u32,
    #[arg(long, default_value = "double.meow", help = "pilote le bot : n'accepte les commandes (whisper) que de ce pseudo (vide = tout le monde)")]
    commander: String,
    #[arg(long, default_value = "Candrena,Pholecia", help = "pseudos admin (separes par des virgules) : seuls autorises aux commandes admin")]
    admins: String,
    #[arg(long, help = "active l'automatisation des quetes : vanguard + guildquest (relance) + guardian legion + daily credit")]
    auto: bool,
    #[arg(long, default_value = "auto", help = "redirige les whisper recus (des non-admins) : auto (admins en ligne, sinon guilde) | whisper (tous les admins) | guild | off")]
    whisper_relay: String,
    #[arg(long, help = "message de bienvenue en guilde quand un membre se connecte")]
    greet: bool,
    #[arg(long, default_value = "Bienvenue {name} !", help = "texte d'accueil ({name} = pseudo du membre)")]
    greet_text: String,
    #[arg(long, help = "annonce en guilde quand une guild quest est relancee (avec --auto)")]
    announce_gquest: bool,
    #[arg(long, default_value_t = 0, help = "annonce periodique en guilde toutes les N minutes (0 = off)")]
    announce_interval: u64,
    #[arg(long, default_value = "", help = "texte de l'annonce periodique")]
    announce_text: String,
    #[arg(long, help = "webhook Discord : relaie le chat guilde + DM recus vers ce salon")]
    discord_webhook: Option<String>,
    #[arg(long, help = "fichier d'historique du chat (horodate, tous canaux)")]
    chat_log: Option<PathBuf>,
    #[arg(long, default_value = "tera-bot-state.json", help = "fichier d'etat persistant (boite aux lettres, seen, watchlist, minuteurs)")]
    state: PathBuf,
    #[arg(long, help = "fichier JSON de noms de zones (map \"id\": \"nom\") pour afficher les zones au lieu des ids")]
    zones: Option<PathBuf>,
    #[arg(long, default_value = "", help = "liste de pseudos a surveiller (separes par des virgules) : alerte quand ils se co / apparaissent pres du bot")]
    watch: String,
    #[arg(long, default_value_t = 5, help = "alerte Discord si le bot reste deconnecte plus de N minutes (0 = off)")]
    heartbeat_down_min: u64,
    #[arg(long, default_value_t = 300, help = "alerte si le ping depasse N ms ou en cas de perte de paquets (0 = off)")]
    ping_alert_ms: u32,
    #[arg(long, default_value = "data/boss_npcs.json", help = "base de donnees des boss/marchands (hz:tid -> nom) pour le tracker ; vide pour desactiver")]
    boss_db: Option<PathBuf>,
    #[arg(long, help = "annonce en guilde (+ Discord) quand un boss de monde / marchand suivi apparait")]
    boss_alert: bool,
    #[arg(long, help = "ramasse automatiquement les colis (hors contre-remboursement) et alerte de leur contenu")]
    auto_parcel: bool,
    #[arg(long, help = "salut auto : rend la salutation (skill 60401301) quand un joueur salue le bot, + commande salut [nom]")]
    salute: bool,
    #[arg(long, default_value_t = 300, help = "intervalle (secondes) de verification de la boite aux colis avec --auto-parcel")]
    parcel_interval: u64,
    #[arg(long, help = "fichier CSV ou enregistrer l'historique de ping (timestamp,rtt,jitter,loss)")]
    ping_log: Option<PathBuf>,
    #[arg(long, help = "journal d'activite horodate (morts, changements de zone, deconnexions, pics de latence, colis, boss)")]
    activity_log: Option<PathBuf>,
    #[arg(long, help = "reconnexion automatique a la deconnexion (garde la session Asura vivante)")]
    reconnect: bool,
    #[arg(long, help = "ressuscite automatiquement a la mort (+ alerte)")]
    auto_respawn: bool,
    #[arg(long, help = "refuse auto les contrats non-party (trade/duel) pour ne pas rester bloque")]
    refuse_trade_duel: bool,
    #[arg(long, help = "alerte si le bot change de zone sans ordre (teleporte par un GM, kick...)")]
    watchdog_zone: bool,
    #[arg(long, help = "reponse auto AFK envoyee aux whisper des non-admins (ex: 'je suis AFK, contacte un admin')")]
    afk_reply: Option<String>,
    #[arg(long, help = "fichier JSON de FAQ : repond automatiquement aux whisper selon des mots-cles (voir services::Faq)")]
    faq: Option<PathBuf>,
    #[arg(long, help = "fichier de config JSON (ses valeurs ecrasent les defauts ; pratique pour ne pas tout retaper)")]
    config: Option<PathBuf>,
    #[arg(long, help = "active le heal de groupe automatique (le bot doit etre priest)")]
    heal: bool,
    #[arg(long, help = "heal aussi les joueurs hors groupe autour (WIP)")]
    heal_everyone: bool,
    #[arg(long, default_value_t = 190900, help = "id de base du focus heal (Focus Heal IX pretre)")]
    focus_heal_id: u32,
    #[arg(long, default_value_t = 370200, help = "id de base du healing immersion (Healing Immersion II pretre)")]
    healing_immersion_id: u32,
    #[arg(long, default_value_t = 85.0, help = "declenche le heal en dessous de ce %% de vie")]
    hp_limit: f64,
    #[arg(long, default_value_t = 20.0, help = "portee de heal en metres")]
    heal_range: f64,
    #[arg(long, default_value_t = 2, help = "1er champ 'type' du C_REQUEST_CONTRACT (ajustable via 'itype N'; unk2=type de contrat via 'iunk2 N')")]
    invite_type: i64,
    #[arg(long, default_value_t = 0, help = "secondes d'ecoute (0 = illimite, repond aux pings)")]
    listen: u64,
    #[arg(long, help = "ne teste que connexion + greeting + handshake (aucun fichier requis)")]
    probe: bool,
    #[arg(long, help = "envoie le handshake en premier au lieu d'attendre le greeting")]
    send_first: bool,
    #[arg(long, help = "affiche TOUS les paquets que le bot envoie, en clair, SANS se connecter")]
    show: bool,
    #[arg(long, help = "aller-retour parse/re-serialise sur C_CHECK_VERSION reel")]
    roundtrip: bool,
}

fn hexdump(prefix: &str, data: &[u8]) {
    for (i, chunk) in data.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
            .collect();
        vlogln!("{prefix}{:04x}  {:<47}  {ascii}", i * 16, hex.join(" "));
    }
}

fn show(
    dir: &str,
    t0: Instant,
    name: &str,
    opcode: u16,
    body: &[u8],
    registry: &Registry,
    full: &[u8],
) {
    record_packet(dir, t0, name, opcode, full);
    traffic::record(dir == "<-", opcode, name, full.len());
    if COMPACT.load(Ordering::Relaxed) {
        let preview: String = body
            .iter()
            .take(24)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        vlogln!(
            "[{:7.3}] {dir} {name} ({opcode}) {} o  {preview}",
            t0.elapsed().as_secs_f64(),
            body.len()
        );
        return;
    }
    vlogln!(
        "\n[{:7.3}] {dir} {name} ({opcode}) {} octets",
        t0.elapsed().as_secs_f64(),
        body.len()
    );
    hexdump("           ", body);
    match registry.get(name) {
        Some(def) => match tera_protocol::read(def, full) {
            Ok(obj) => {
                for (k, v) in obj.fields.iter() {
                    vlogln!("           . {k} = {v:?}");
                }
                if obj.fields.is_empty() {
                    vlogln!("           . (aucun champ)");
                }
            }
            Err(e) => vlogln!("           . decodage impossible : {e}"),
        },
        None => vlogln!("           . pas de definition pour {name}"),
    }
}

fn read_exactly(stream: &mut TcpStream, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).with_context(|| format!("lecture de {len} octets"))?;
    Ok(buf)
}

fn send(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    opcodes: &OpcodeMap,
    name: &str,
    object: &tera_protocol::Object,
    t0: Instant,
) -> Result<()> {
    let def = registry.get(name).with_context(|| format!("definition {name} introuvable"))?;
    let opcode = opcodes.code(name).with_context(|| format!("opcode {name} introuvable"))?;
    let mut bytes = tera_protocol::write(def, opcode, object)?;
    show("->", t0, name, opcode, &bytes[4..], registry, &bytes);
    enc.apply(&mut bytes);
    stream.write_all(&bytes)?;
    Ok(())
}

fn send_raw(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    name: &str,
    mut bytes: Vec<u8>,
    t0: Instant,
) -> Result<()> {
    if bytes.len() >= 4 {
        let opcode = u16::from_le_bytes([bytes[2], bytes[3]]);
        show("->", t0, name, opcode, &bytes[4..], registry, &bytes);
    }
    enc.apply(&mut bytes);
    stream.write_all(&bytes)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn send_alert(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    opcodes: &OpcodeMap,
    t0: Instant,
    ui: &Option<UiHandle>,
    discord: &Option<mpsc::Sender<String>>,
    admin_names: &[String],
    online_admins: &std::collections::HashSet<String>,
    server_id: u64,
    kind: tui::LogKind,
    message: &str,
) -> Result<bool> {
    logln!("[{:7.3}] {message}", t0.elapsed().as_secs_f64());
    if let Some(handle) = ui {
        handle.log(kind, message.to_string());
    }
    if let Some(sink) = discord {
        let _ = sink.send(format!("@here {message}"));
    }
    for admin in admin_names.iter().filter(|name| online_admins.contains(&name.to_lowercase())) {
        let notify = tera_protocol::Object::new()
            .with("targetServer", tera_protocol::Value::Uint(server_id))
            .with("target", tera_protocol::Value::Str(admin.clone()))
            .with("message", tera_protocol::Value::Str(message.to_string()));
        if reactive_send(stream, enc, registry, opcodes, "C_WHISPER", &notify, t0)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn is_disconnect(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .map(|e| {
            matches!(
                e.kind(),
                ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
            )
        })
        .unwrap_or(false)
}

fn reactive_send(
    stream: &mut TcpStream,
    enc: &mut Encrypting,
    registry: &Registry,
    opcodes: &OpcodeMap,
    name: &str,
    object: &tera_protocol::Object,
    t0: Instant,
) -> Result<bool> {
    match send(stream, enc, registry, opcodes, name, object, t0) {
        Ok(()) => Ok(false),
        Err(e) if is_disconnect(&e) => {
            logln!("[{:7.3}] serveur a ferme (ecriture {name})", t0.elapsed().as_secs_f64());
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

#[derive(serde::Deserialize)]
struct SlServer {
    name: String,
    address: String,
    port: u16,
    #[serde(default)]
    available: i64,
}

#[derive(serde::Deserialize)]
struct ServerListDoc {
    servers: Vec<SlServer>,
}

fn resolve_server(url: &str, name: Option<&str>) -> Result<String> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "tera-bot")
        .call()
        .context("recuperation de la serverlist")?;
    let doc: ServerListDoc = resp.body_mut().read_json().context("parsing de la serverlist")?;
    let chosen = match name {
        Some(n) => {
            let n = n.to_lowercase();
            doc.servers.iter().find(|s| s.name.to_lowercase().contains(&n))
        }
        None => doc
            .servers
            .iter()
            .find(|s| s.available != 0)
            .or_else(|| doc.servers.first()),
    }
    .ok_or_else(|| anyhow::anyhow!("aucun serveur trouve dans la serverlist"))?;
    Ok(format!("{}:{}", chosen.address, chosen.port))
}

fn probe(server: &str) -> Result<()> {
    let t0 = Instant::now();
    logln!("cible {server}");
    let mut stream = match TcpStream::connect(server) {
        Ok(s) => s,
        Err(e) => { logln!("[{:6.2}s] CONNEXION IMPOSSIBLE : {e}", t0.elapsed().as_secs_f64()); return Ok(()) }
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(25)))?;
    logln!("[{:6.2}s] TCP connecte", t0.elapsed().as_secs_f64());
    let mut greeting = [0u8; 4];
    match stream.read_exact(&mut greeting) {
        Ok(()) => logln!("[{:6.2}s] GREETING RECU {greeting:02x?}  -> le serveur repond, ce reseau marche", t0.elapsed().as_secs_f64()),
        Err(e) => {
            logln!("[{:6.2}s] AUCUN GREETING : {e}", t0.elapsed().as_secs_f64());
            logln!("           -> le serveur accepte le TCP mais ne parle jamais depuis ce reseau");
            return Ok(());
        }
    }
    let hs = ClientHandshake::new(random_key(), random_key()).with_constants(MODERN);
    stream.write_all(hs.first())?;
    let s1 = read_exactly(&mut stream, KEY_LEN)?;
    stream.write_all(hs.second())?;
    let s2 = read_exactly(&mut stream, KEY_LEN)?;
    let _ = (s1, s2);
    logln!("[{:6.2}s] HANDSHAKE COMPLET -> tout va bien depuis ce reseau", t0.elapsed().as_secs_f64());
    Ok(())
}

#[derive(Clone)]
struct GuildMember {
    name: String,
    class: i64,
    level: i64,
    online: bool,
    rank: i64,
    zone: i64,
    note: String,
    contribution: i64,
}

#[derive(Default, Clone)]
struct GuildInfo {
    id: i64,
    name: String,
    master: String,
    level: i64,
    funds: i64,
    characters: i64,
    accounts: i64,
    motd: String,
}

fn class_name(class: i64) -> &'static str {
    match class {
        0 => "Guerrier",
        1 => "Lancier",
        2 => "Pourfendeur",
        3 => "Berserker",
        4 => "Sorcier",
        5 => "Archer",
        6 => "Pretre",
        7 => "Elementaliste",
        8 => "Faucheur",
        9 => "Ingenieur",
        10 => "Bagarreur",
        11 => "Assassin",
        12 => "Valkyrie",
        _ => "?",
    }
}

fn class_from_template(template_id: i64) -> i64 {
    let base = template_id - 10101;
    if base < 0 {
        -1
    } else {
        base % 100
    }
}

struct NearbyPlayer {
    name: String,
    class_id: i64,
    level: i64,
    gm: bool,
    gm_invisible: bool,
    guild: String,
    location: tera_world::Vec3,
}

struct BossSpawn {
    name: String,
    kind: String,
    region: String,
    since: Instant,
}

fn human_duration(seconds: u64) -> String {
    let h = seconds / 3600;
    let m = (seconds % 3600) / 60;
    let s = seconds % 60;
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

fn strip_markup(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_tag = false;
    for ch in message.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

fn is_system_template(text: &str) -> bool {
    let trimmed = text.trim_start();
    let mut chars = trimmed.chars();
    if chars.next() != Some('@') {
        return false;
    }
    matches!(chars.next(), Some(c) if c.is_ascii_digit())
}

fn print_chat(registry: &Registry, name: &str, packet: &tera_protocol::Packet) {
    let Some(def) = registry.get(name) else { return };
    let Ok(object) = tera_protocol::read(def, &packet.encode()) else { return };
    let message = object.get("message").and_then(tera_protocol::Value::as_str).unwrap_or("");
    let channel = object.get("channel").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
    let clean = strip_markup(message);
    if clean.trim().is_empty() {
        return;
    }
    let author = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("");
    let label = match name {
        "S_WHISPER" | "S_PRIVATE_CHAT" => "chuchote",
        "S_DUNGEON_EVENT_MESSAGE" | "S_SYSTEM_MESSAGE" => "serveur",
        _ => "chat",
    };
    if author.is_empty() {
        logln!("  [{label} c{channel}] {clean}");
    } else {
        logln!("  [{label} c{channel}] {author}: {clean}");
    }
}

fn parse_chat(registry: &Registry, name: &str, packet: &tera_protocol::Packet) -> Option<(u64, String, String)> {
    let def = registry.get(name)?;
    let object = tera_protocol::read(def, &packet.encode()).ok()?;
    let message = object.get("message").and_then(tera_protocol::Value::as_str).unwrap_or("");
    let channel = object.get("channel").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
    let author = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
    let clean = strip_markup(message);
    if clean.trim().is_empty() {
        return None;
    }
    Some((channel, author, clean))
}

struct CharacterSlot {
    id: u64,
    name: String,
    level: i64,
    class: i64,
}

fn class_label(class: i64) -> &'static str {
    match class {
        0 => "Warrior",
        1 => "Lancer",
        2 => "Slayer",
        3 => "Berserker",
        4 => "Sorcerer",
        5 => "Archer",
        6 => "Priest",
        7 => "Mystic",
        8 => "Reaper",
        9 => "Gunner",
        10 => "Brawler",
        11 => "Ninja",
        12 => "Valkyrie",
        _ => "?",
    }
}

fn character_list(registry: &Registry, packet: &tera_protocol::Packet) -> Vec<CharacterSlot> {
    let Some(def) = registry.get("S_GET_USER_LIST") else { return Vec::new() };
    let Ok(object) = tera_protocol::read(def, &packet.encode()) else { return Vec::new() };
    let Some(tera_protocol::Value::Array(list)) = object.get("characters") else { return Vec::new() };
    list.iter()
        .filter_map(|c| {
            let id = c.get("id").and_then(tera_protocol::Value::as_uint)?;
            Some(CharacterSlot {
                id,
                name: c.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("?").to_string(),
                level: c.get("level").and_then(tera_protocol::Value::as_int).unwrap_or(0),
                class: c.get("class").and_then(tera_protocol::Value::as_int).unwrap_or(-1),
            })
        })
        .collect()
}

fn start_tui(start_tab: usize) -> (UiHandle, thread::JoinHandle<()>) {
    let (event_tx, handle, input_tx, event_rx) = tui::channels();
    let _ = UI_SINK.set(event_tx);
    let thread = thread::spawn(move || {
        let _ = tui::run(event_rx, input_tx, start_tab);
    });
    (handle, thread)
}

fn pick_character(
    registry: &Registry,
    packet: &tera_protocol::Packet,
    want: Option<&str>,
) -> Option<(u64, String)> {
    let list = character_list(registry, packet);
    match want {
        Some(wanted) => list
            .into_iter()
            .find(|c| c.name.eq_ignore_ascii_case(wanted))
            .map(|c| (c.id, c.name)),
        None => list.into_iter().next().map(|c| (c.id, c.name)),
    }
}

fn prompt_character(list: &[CharacterSlot]) -> Option<(u64, String)> {
    use std::io::Write;
    if list.is_empty() {
        return None;
    }
    println!("\nPersonnages du compte :");
    for (i, c) in list.iter().enumerate() {
        println!("  [{}] {} - niv {} - {}", i + 1, c.name, c.level, class_label(c.class));
    }
    loop {
        print!("Choisis un perso (1-{}, Entree = 1) : ", list.len());
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() {
            return list.first().map(|c| (c.id, c.name.clone()));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            let c = &list[0];
            return Some((c.id, c.name.clone()));
        }
        match trimmed.parse::<usize>() {
            Ok(n) if n >= 1 && n <= list.len() => {
                let c = &list[n - 1];
                return Some((c.id, c.name.clone()));
            }
            _ => println!("Entree invalide."),
        }
    }
}

fn build_outgoing(cli: &Cli, account: &str, ticket_bytes: &[u8]) -> Vec<(&'static str, tera_protocol::Object)> {
    let u = |v: u64| tera_protocol::Value::Uint(v);
    let version = tera_protocol::Object::new().with("version", tera_protocol::Value::Array(vec![
        tera_protocol::Object::new().with("index", tera_protocol::Value::Int(0)).with("value", tera_protocol::Value::Int(cli.version_a)),
        tera_protocol::Object::new().with("index", tera_protocol::Value::Int(1)).with("value", tera_protocol::Value::Int(cli.version_b)),
    ]));
    let login = tera_protocol::Object::new()
        .with("unk1", tera_protocol::Value::Int(0)).with("unk2", u(0))
        .with("language", u(cli.language as u64)).with("patchVersion", tera_protocol::Value::Int(cli.patch_version as i64))
        .with("name", tera_protocol::Value::Str(account.to_string()))
        .with("ticket", tera_protocol::Value::Bytes(ticket_bytes.to_vec()));
    let range = tera_protocol::Object::new().with("range", u(2000));
    let hw = tera_protocol::Object::new()
        .with("systemMemory", u(16383)).with("videoMemory", u(0)).with("resWidth", u(1920)).with("resHeight", u(1080))
        .with("isFullScreen", tera_protocol::Value::Bool(false)).with("resScreenWidth", u(1920)).with("resScreenHeight", u(1080))
        .with("numDisplays", u(1)).with("resVirtualWidth", u(1920)).with("resVirtualHeight", u(1080))
        .with("physicalCores", u(10)).with("logicalCores", u(10))
        .with("os", tera_protocol::Value::Str("Windows 10".into()))
        .with("cpu", tera_protocol::Value::Str("VirtualApple @ 2.50GHz".into()))
        .with("gpu", tera_protocol::Value::Str(String::new()));
    vec![
        ("C_CHECK_VERSION", version), ("C_LOGIN_ARBITER", login),
        ("C_SET_VISIBLE_RANGE", range), ("C_GET_USER_LIST", tera_protocol::Object::new()),
        ("C_HARDWARE_INFO", hw), ("C_PONG", tera_protocol::Object::new()),
    ]
}

fn show_all(cli: &Cli, account: &str, opcodes: &OpcodeMap, registry: &Registry, ticket_bytes: &[u8]) -> Result<()> {
    logln!("=== paquets que le bot ENVOIE (compte={}, ticket={} octets) ===", account, ticket_bytes.len());
    let t0 = Instant::now();
    for (name, obj) in build_outgoing(cli, account, ticket_bytes) {
        let def = registry.get(name).with_context(|| format!("def {name}"))?;
        let opcode = opcodes.code(name).with_context(|| format!("opcode {name}"))?;
        let bytes = tera_protocol::write(def, opcode, &obj)?;
        show("->", t0, name, opcode, &bytes[4..], registry, &bytes);
    }
    Ok(())
}

struct ProxyGuard(Option<std::process::Child>);

impl Drop for ProxyGuard {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_proxy(
    upstream: &str,
    opcodes: &std::path::Path,
    definitions: &[PathBuf],
    major_patch: u32,
    quiet: bool,
    log_path: Option<&PathBuf>,
) -> Result<(std::process::Child, String)> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").context("picking a proxy port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let listen = format!("127.0.0.1:{port}");

    let exe = std::env::current_exe()?;
    let binary = exe
        .parent()
        .map(|dir| dir.join("tera-proxy"))
        .context("locating tera-proxy next to tera-bot")?;
    let mut command = std::process::Command::new(&binary);
    command
        .arg("--listen")
        .arg(&listen)
        .arg("--upstream")
        .arg(upstream)
        .arg("--opcodes")
        .arg(opcodes)
        .arg("--patch-version")
        .arg(major_patch.to_string())
        .arg("--once");
    for definition in definitions {
        command.arg("--definitions").arg(definition);
    }
    if quiet {
        let sink = match log_path {
            Some(path) => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map(std::process::Stdio::from)
                .unwrap_or_else(|_| std::process::Stdio::null()),
            None => std::process::Stdio::null(),
        };
        let sink_err = match log_path {
            Some(path) => std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map(std::process::Stdio::from)
                .unwrap_or_else(|_| std::process::Stdio::null()),
            None => std::process::Stdio::null(),
        };
        command.stdout(sink).stderr(sink_err);
    }
    let child = command
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;

    for _ in 0..100 {
        if std::net::TcpListener::bind(&listen).is_err() {
            return Ok((child, listen));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok((child, listen))
}

fn socks5_connect(proxy: &str, target: &str) -> Result<TcpStream> {
    let proxy_addr = proxy
        .to_socket_addrs()
        .context("resolution du proxy socks5")?
        .next()
        .with_context(|| format!("aucune adresse pour {proxy}"))?;
    let (host, port) = target
        .rsplit_once(':')
        .with_context(|| format!("cible invalide {target}"))?;
    let port: u16 = port.parse().with_context(|| format!("port invalide {port}"))?;
    let mut stream = TcpStream::connect_timeout(&proxy_addr, Duration::from_secs(10))
        .with_context(|| format!("connexion au proxy socks5 {proxy}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.write_all(&[0x05, 0x01, 0x00])?;
    let mut method = [0u8; 2];
    stream.read_exact(&mut method)?;
    if method != [0x05, 0x00] {
        bail!("proxy socks5 refuse le no-auth (reponse {method:02x?})");
    }
    let mut request = vec![0x05, 0x01, 0x00];
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => {
            request.push(0x01);
            request.extend_from_slice(&ip.octets());
        }
        Err(_) => {
            let bytes = host.as_bytes();
            if bytes.len() > 255 {
                bail!("hote trop long pour socks5 : {host}");
            }
            request.push(0x03);
            request.push(bytes.len() as u8);
            request.extend_from_slice(bytes);
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request)?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply)?;
    if reply[1] != 0x00 {
        bail!("socks5 connect echoue (code {})", reply[1]);
    }
    let bound = match reply[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len)?;
            len[0] as usize
        }
        other => bail!("socks5 atyp inconnu {other}"),
    };
    let mut tail = vec![0u8; bound + 2];
    stream.read_exact(&mut tail)?;
    Ok(stream)
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();
    if let Some(path) = cli.config.clone() {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("lecture de la config {}", path.display()))?;
        let config: BotConfig = serde_json::from_str(&raw)
            .with_context(|| format!("config JSON invalide : {}", path.display()))?;
        apply_config(&mut cli, &config);
    }
    if cli.asura_release_tunnel {
        let out = asura::release_tunnel(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15))
            .context("release_tunnel")?;
        println!("Asura : tunnel rendu au launcher ({out}). Tu peux relancer ton jeu normalement.");
        return Ok(());
    }
    // Garde le launcher qu'on a eventuellement demarre nous-memes en vie
    // jusqu'a la fin (Drop le ferme), succes ou erreur.
    let mut _asura_launcher_guard: Option<asura::LaunchedLauncher> = None;

    if cli.arborea_login {
        let email = cli.arborea_email.as_deref().context("--arborea-email requis")?;
        let password = cli.arborea_password.as_deref().context("--arborea-password requis")?;
        let captcha = cli.arborea_captcha.as_deref().context("--arborea-captcha requis")?;
        let saved = arborea::login(email, password, captcha)?;
        arborea::save(&cli.arborea_auth_file, &saved)?;
        logln!(
            "Arborea : refresh_token sauve dans {}",
            cli.arborea_auth_file.display()
        );
        return Ok(());
    }

    let auth_path = cli
        .auth_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("tera-bot-auth.json"));

    if cli.print_ticket {
        let saved = if cli.login {
            let fresh = auth::login()?;
            auth::save(&auth_path, &fresh)?;
            fresh
        } else {
            let saved = auth::load(&auth_path).with_context(|| {
                format!("aucune auth dans {} (relance avec --login)", auth_path.display())
            })?;
            let fresh = auth::refresh(&auth_path, &saved)?;
            auth::save(&auth_path, &fresh)?;
            fresh
        };
        logln!("AUTH\t{}\t{}", saved.account(), saved.auth_key);
        return Ok(());
    }

    if cli.login {
        let saved = auth::login()?;
        auth::save(&auth_path, &saved)?;
        logln!(
            "Connecte en tant que {} (UserNo {}). Auth sauvee dans {}.",
            saved.account(),
            saved.user_no,
            auth_path.display()
        );
        logln!("Ticket frais : {}", saved.auth_key);
        return Ok(());
    }

    let mut asura_session: Option<asura_auth::Session> = None;
    let (account, mut ticket_bytes): (String, Vec<u8>) = if cli.arborea {
        let saved = arborea::load(&cli.arborea_auth_file)?;
        logln!("Arborea : refresh du ticket...");
        let game = arborea::prepare(&saved, &cli.arborea_auth_file)?;
        logln!(
            "Arborea : {} (id {}) -> serveur {} (ticket {})",
            game.display_name, game.name, game.server, game.ticket
        );
        cli.server = Some(game.server);
        cli.patch_version = arborea::BUILD_VERSION;
        (game.name, game.ticket.into_bytes())
    } else if cli.asura_native {
        let mut session = asura_auth::Session::new();
        let account = session.web_auth(Duration::from_secs(600))
            .context("web-auth Asura")?;
        if !session.is_osid_verified(cli.asura_osid)? {
            logln!("Asura : nouvel appareil (osid {}) non verifie pour ce compte.", cli.asura_osid);
            match session.send_osid_code(cli.asura_osid) {
                Ok(()) => logln!("Asura : un code de verification a ete envoye par email."),
                Err(e) => logln!("Asura : pas de nouveau code envoye ({e}) ; utilise le dernier code recu par email."),
            }
            logln!("Asura : entre le code de verification recu par email (puis Entree) :");
            let mut code = String::new();
            std::io::stdin().read_line(&mut code).context("lecture du code")?;
            session.verify_osid(cli.asura_osid, code.trim()).context("verification de l'osid")?;
            logln!("Asura : osid verifie.");
        }
        if cli.asura_probe_sig {
            session.probe_signature(cli.asura_osid, &cli.asura_datacenter)?;
            return Ok(());
        }
        if cli.asura_launch {
            let wine = cli.asura_wine.clone().unwrap_or_else(|| {
                "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine".to_string()
            });
            let launcher_exe = cli.asura_launcher_exe.clone().unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!(
                    "{home}/Library/Application Support/CrossOver/Bottles/{}/drive_c/Games/TERA Asura/Launcher/launcher.exe",
                    cli.asura_bottle
                )
            });
            let config = asura::LaunchConfig {
                wine,
                bottle: cli.asura_bottle.clone(),
                launcher_exe,
                cdp_port: cli.asura_cdp_port,
            };
            _asura_launcher_guard =
                asura::ensure_launcher_running(&cli.asura_cdp_host, Some(&config), Duration::from_secs(60))
                    .context("demarrage automatique du launcher dedie au bot")?;
            logln!("Asura : launcher pret (bottle {}, port CDP {})", cli.asura_bottle, cli.asura_cdp_port);
        }
        if cli.server.is_none() && cli.asura_relay {
            match asura::fetch_relay_server(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15)) {
                Ok(Some(addr)) if addr.starts_with("127.") => { logln!("Asura : relais local du launcher -> {addr}"); cli.server = Some(addr); }
                Ok(Some(addr)) => logln!("Asura : launcher renvoie un leurre non-local ({addr}) ; le tunnel n'est pas actif (lance Jouer une fois, ferme le JEU en gardant le launcher)."),
                Ok(None) => logln!("Asura : launcher joint mais pas de relais local actif (lance Jouer une fois pour l'activer)."),
                Err(e) => logln!("Asura : relais launcher indisponible ({e})."),
            }
        }
        if cli.asura_keep_tunnel {
            match asura::keep_tunnel_alive(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15)) {
                Ok(s) => logln!("Asura : tunnel ExitLag maintenu ({s}) ; fermer le jeu ne coupera plus le relais."),
                Err(e) => logln!("Asura : neutralisation du teardown ExitLag impossible ({e})."),
            }
        }
        if cli.server.is_none() {
            match session.server_address() {
                Ok(addr) => { logln!("Asura : serveur (sls) -> {addr}"); cli.server = Some(addr); }
                Err(e) => { logln!("Asura : sls indisponible ({e}), repli sur {}", cli.asura_server); cli.server = Some(cli.asura_server.clone()); }
            }
        }
        let otp = session.game_otp(cli.asura_osid)
            .context("game_otp Asura")?;
        cli.patch_version = asura::PATCH_VERSION;
        let name = account.account_name;
        logln!(
            "Asura natif : compte {} (userId {}) -> serveur {} (otp {} octets, patchVersion {})",
            name, account.user_id, cli.server.as_deref().unwrap_or("?"), otp.len(), cli.patch_version
        );
        asura_session = Some(session);
        (name, otp.into_bytes())
    } else if cli.asura {
        if cli.asura_launch {
            let wine = cli.asura_wine.clone().unwrap_or_else(|| {
                "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine".to_string()
            });
            let launcher_exe = cli.asura_launcher_exe.clone().unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!(
                    "{home}/Library/Application Support/CrossOver/Bottles/{}/drive_c/Games/TERA Asura/Launcher/launcher.exe",
                    cli.asura_bottle
                )
            });
            let config = asura::LaunchConfig {
                wine,
                bottle: cli.asura_bottle.clone(),
                launcher_exe,
                cdp_port: cli.asura_cdp_port,
            };
            _asura_launcher_guard =
                asura::ensure_launcher_running(&cli.asura_cdp_host, Some(&config), Duration::from_secs(60))
                    .context("demarrage automatique du launcher Asura")?;
        }
        logln!(
            "Asura : demande d'un OTP frais au launcher (port de debug {}:{})...",
            cli.asura_cdp_host, cli.asura_cdp_port
        );
        let ticket = asura::fetch_ticket(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15))
            .context("recuperation du ticket Asura via le launcher ; verifie qu'il tourne avec --remote-debugging-port et que le bouton Jouer est actif")?;
        cli.patch_version = asura::PATCH_VERSION;
        if cli.account == "41946" {
            eprintln!(
                "Attention : --account est reste a sa valeur par defaut ({}) ; le vrai client envoie l'id de compte du SITE (ex: 65140), pas l'osid ({}). Passe --account <ton id>.",
                cli.account, ticket.osid
            );
        }
        if cli.server.is_none() {
            match ticket.server.filter(|addr| addr.starts_with("127.")) {
                // Le launcher ne connecte PAS le jeu a l'IP publique de la serverlist :
                // il ouvre un relais local (127.0.0.1:<port ephemere>, type ExitLag) qui
                // doit rester actif, donc le launcher doit rester ouvert pendant la session.
                // Tant que le tunnel ExitLag n'est pas reellement etabli, le launcher
                // renvoie un leurre non-local (ex: 1.2.3.5) qu'on refuse d'utiliser.
                Some(addr) => {
                    logln!("Asura : relais local decouvert via le launcher -> {addr}");
                    cli.server = Some(addr);
                }
                None => {
                    logln!(
                        "Asura : le launcher n'a pas encore de relais local actif (leurre ExitLag ou aucune reponse). \
                         Lance Jouer une fois dans le vrai launcher pour activer le tunnel, puis ferme le JEU (pas le launcher) et relance le bot. \
                         Repli sur l'adresse fixe {} (risque de timeout, voir --asura-server).",
                        cli.asura_server
                    );
                    cli.server = Some(cli.asura_server.clone());
                }
            }
        }
        logln!(
            "Asura : compte {} (osid {}) -> serveur {} (otp {} octets, patchVersion {})",
            cli.account, ticket.osid, cli.server.as_deref().unwrap_or("?"), ticket.otp.len(), cli.patch_version
        );
        (cli.account.clone(), ticket.otp.into_bytes())
    } else if cli.auth_file.is_some() {
        let saved = auth::load(&auth_path)?;
        logln!("Refresh du ticket via le refresh_token sauve...");
        let fresh = auth::refresh(&auth_path, &saved)?;
        auth::save(&auth_path, &fresh)?;
        logln!(
            "Ticket frais pour {} : {} ({} octets)",
            fresh.account(),
            fresh.auth_key,
            fresh.auth_key.len()
        );
        (fresh.account(), fresh.auth_key.as_bytes().to_vec())
    } else {
        let bytes = match &cli.ticket_file {
            Some(f) => {
                let b = std::fs::read(f)
                    .with_context(|| format!("lecture du ticket {}", f.display()))?;
                logln!("ticket lu depuis {} : {} octets bruts", f.display(), b.len());
                b
            }
            None => cli.ticket.as_bytes().to_vec(),
        };
        (cli.account.clone(), bytes)
    };
    let opcodes = OpcodeMap::read(&cli.opcodes)
        .with_context(|| format!("lecture de {}", cli.opcodes.display()))?;
    let arborea_pins: std::collections::HashMap<String, u32> = [
        ("C_REQUEST_CONTRACT", 2),
        ("C_REPLY_THROUGH_ARBITER_CONTRACT", 1),
        ("S_BEGIN_THROUGH_ARBITER_CONTRACT", 1),
        ("S_REQUEST_CONTRACT", 1),
        ("S_PARTY_MEMBER_LIST", 7),
        ("S_PARTY_MEMBER_INTERVAL_POS_UPDATE", 3),
        ("S_PARTY_MEMBER_CHANGE_HP", 4),
        ("C_START_SKILL", 8),
        ("C_CAN_LOCKON_TARGET", 3),
        ("S_START_COOLTIME_SKILL", 3),
        ("S_EACH_SKILL_RESULT", 14),
        ("S_ACTION_STAGE", 9),
        ("S_SPAWN_NPC", 11),
        ("S_NPC_STATUS", 2),
        ("C_PRESS_SKILL", 4),
        ("S_DIALOG", 3),
        ("C_NPC_CONTACT", 2),
        ("C_DIALOG", 1),
        ("S_SPAWN_USER", 16),
        ("S_SPAWN_ME", 3),
        ("S_USER_LOCATION", 5),
        ("S_LOGIN", 14),
        ("C_PLAYER_LOCATION", 5),
    ]
    .iter()
    .map(|(name, version)| ((*name).to_string(), *version))
    .collect();
    let registry = Registry::pinned(&cli.definitions, Some(cli.major_patch), &arborea_pins)?;
    logln!("{} opcodes, {} definitions", opcodes.len(), registry.len());

    if let Some(path) = &cli.replay {
        return replay_capture(path, &opcodes, &registry);
    }
    if let Some(path) = &cli.record {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("ouverture du fichier d'enregistrement {}", path.display()))?;
        let _ = RECORD_FILE.set(Mutex::new(file));
        logln!("enregistrement des paquets dans {}", path.display());
    }

    if cli.roundtrip {
        let real: Vec<u8> = vec![
            0x20,0x00,0xbc,0x4d, 0x02,0x00,0x08,0x00,0x08,0x00,0x14,0x00,
            0x00,0x00,0x00,0x00,0xcc,0xbc,0x05,0x00,0x14,0x00,0x00,0x00,
            0x01,0x00,0x00,0x00,0xc1,0xbc,0x05,0x00,
        ];
        let def = registry.get("C_CHECK_VERSION").context("def C_CHECK_VERSION")?;
        logln!("octets reels du client ({} o) :", real.len());
        for c in real.chunks(16) { logln!("  {}", c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")); }
        let obj = tera_protocol::read(def, &real).context("PARSE echoue")?;
        logln!("\nparse -> {obj:?}");
        let reser = tera_protocol::write(def, 19900, &obj).context("re-serialisation echouee")?;
        logln!("\nre-serialise ({} o) :", reser.len());
        for c in reser.chunks(16) { logln!("  {}", c.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")); }
        logln!("\n=> {}", if reser == real { "IDENTIQUE : l'array se parse ET se re-encode a l'octet pres" } else { "DIFFERENT : il y a un probleme dans la def array" });
        return Ok(());
    }
    if cli.server.is_none() {
        let resolved = resolve_server(&cli.serverlist, cli.server_name.as_deref())?;
        logln!("serveur resolu depuis la serverlist : {resolved}");
        cli.server = Some(resolved);
    }
    let server = cli.server.clone().expect("serveur resolu");
    if cli.probe {
        return probe(&server);
    }
    if cli.chat || cli.tui {
        COMPACT.store(true, Ordering::Relaxed);
    }
    if let Some(path) = &cli.log {
        let file = File::create(path).with_context(|| format!("creation du log {}", path.display()))?;
        let _ = LOG_FILE.set(Mutex::new(file));
    }
    if let Some(path) = &cli.chat_log {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("ouverture du log chat {}", path.display()))?;
        let _ = CHAT_LOG.set(Mutex::new(file));
    }
    let admin_names: Vec<String> = cli
        .admins
        .split(',')
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    let admins: std::collections::HashSet<String> =
        admin_names.iter().map(|name| name.to_lowercase()).collect();
    let mut auto = auto::AutoBot::new(auto::AutoConfig {
        vanguard: cli.auto,
        guild_quest: cli.auto,
        guardian: cli.auto,
        daily: cli.auto,
        announce_guild_quest: cli.announce_gquest,
        ..auto::AutoConfig::default()
    });

    let discord = cli.discord_webhook.clone().map(discord::spawn);

    let mut tui_thread: Option<thread::JoinHandle<()>> = None;
    let defer_tui = cli.tui && !cli.show && cli.pick_character && cli.character.is_none();
    let mut did_prompt = false;
    let mut ui: Option<UiHandle> = None;
    if cli.tui && !cli.show && !defer_tui {
        let (handle, thread) = start_tui(tui::tab_index(&cli.tab));
        ui = Some(handle);
        tui_thread = Some(thread);
    }

    // Toujours montrer ce que le bot ENVERRA, avant toute connexion.
    show_all(&cli, &account, &opcodes, &registry, &ticket_bytes)?;
    if cli.show {
        return Ok(());
    }
    logln!("\n--- connexion et echange reel ---");

    let t0 = Instant::now();
    let mut reconnect_attempt: u32 = 0;
    let mut state = services::BotState::load(cli.state.clone());
    for name in cli.watch.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        state.watch_add(name);
    }
    let zone_names = services::ZoneNames::load(cli.zones.as_ref());
    let faq = services::Faq::load(cli.faq.as_ref());
    if !faq.is_empty() {
        logln!("{} regles de FAQ chargees pour les reponses auto", faq.len());
    }
    let item_names = broker::load_items(&PathBuf::from("data/items.json"));
    let mut session_stats = services::SessionStats::default();
    let mut broker = if cli.broker {
        match (opcodes.code("C_TRADE_BROKER_WAITING_ITEM_LIST_PAGE"), opcodes.code("C_TRADE_BROKER_BUY_IT_NOW")) {
            (Some(page_op), Some(buy_op)) => {
                let data_dir = PathBuf::from(&cli.broker_data);
                let items_path = PathBuf::from("data/items.json");
                let contract_op = opcodes.code("C_REQUEST_CONTRACT").unwrap_or(0);
                let b = broker::BrokerBuy::new(&data_dir, &items_path, page_op, buy_op, contract_op, Some(cli.broker_arm));
                if b.has_template() {
                    logln!("broker-buy actif ({})", if cli.broker_arm { "arme" } else { "simu" });
                } else {
                    logln!("broker-buy: template manquant dans {} (ouvre le broker une fois pour le capturer)", cli.broker_data);
                }
                Some(b)
            }
            _ => {
                logln!("broker-buy desactive : opcodes broker absents de la map");
                None
            }
        }
    } else {
        None
    };
    let mut broker_npcs: std::collections::HashMap<u64, (String, u32, [f32; 3])> = std::collections::HashMap::new();
    let mut broker_open_tried = false;
    let mut broker_dialog_sent = false;
    let mut defgen = gendef::DefGen::new(cli.auto_gen_defs, cli.defs_out.clone(), cli.gen_version);
    if cli.auto_gen_defs {
        logln!("auto-generation des .def manquantes activee -> {}", cli.defs_out.display());
    }
    let boss_db = services::BossDb::load(cli.boss_db.as_ref());
    if !boss_db.is_empty() {
        logln!("{} boss/marchands charges pour le tracker", boss_db.len());
    }
    let activity = services::ActivityLog::new(cli.activity_log.clone());
    let ping_csv = services::PingCsv::new(cli.ping_log.clone());
    let mut zone_time = services::ZoneTime::default();
    let mut downtime_total = Duration::ZERO;
    let mut disconnected_since: Option<Instant> = None;
    let mut last_ping_alert: Option<Instant> = None;
    let mut last_timer_tick = Instant::now() - Duration::from_secs(10);
    'reconnect: loop {
    if reconnect_attempt > 0 {
        let backoff = Duration::from_secs((2u64 * u64::from(reconnect_attempt)).clamp(2, 60));
        logln!("[{:7.3}] reconnexion dans {}s (tentative {reconnect_attempt})", t0.elapsed().as_secs_f64(), backoff.as_secs());
        if let Some(handle) = &ui { handle.act(format!("reconnexion dans {}s...", backoff.as_secs())); }
        thread::sleep(backoff);
        auto.on_leave_game();
        if cli.asura_launch && _asura_launcher_guard.is_none() {
            let wine = cli.asura_wine.clone().unwrap_or_else(|| {
                "/Applications/CrossOver.app/Contents/SharedSupport/CrossOver/bin/wine".to_string()
            });
            let launcher_exe = cli.asura_launcher_exe.clone().unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                format!(
                    "{home}/Library/Application Support/CrossOver/Bottles/{}/drive_c/Games/TERA Asura/Launcher/launcher.exe",
                    cli.asura_bottle
                )
            });
            let config = asura::LaunchConfig { wine, bottle: cli.asura_bottle.clone(), launcher_exe, cdp_port: cli.asura_cdp_port };
            match asura::ensure_launcher_running(&cli.asura_cdp_host, Some(&config), Duration::from_secs(60)) {
                Ok(guard) => { _asura_launcher_guard = guard; logln!("[{:7.3}] launcher relance pour la reconnexion", t0.elapsed().as_secs_f64()); }
                Err(e) => logln!("[{:7.3}] relance du launcher dedie echouee ({e})", t0.elapsed().as_secs_f64()),
            }
        }
        if cli.asura_relay {
            if let Ok(Some(addr)) = asura::fetch_relay_server(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15)) {
                if addr.starts_with("127.") {
                    cli.server = Some(addr);
                }
            }
        }
        if cli.asura_keep_tunnel {
            let _ = asura::keep_tunnel_alive(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(15));
        }
        if let Some(session) = asura_session.as_mut() {
            match session.game_otp(cli.asura_osid) {
                Ok(otp) => { ticket_bytes = otp.into_bytes(); logln!("[{:7.3}] OTP rafraichi pour la reconnexion", t0.elapsed().as_secs_f64()); }
                Err(e) => { logln!("[{:7.3}] OTP reconnexion echoue ({e}), abandon", t0.elapsed().as_secs_f64()); break 'reconnect; }
            }
        }
    }
    let server = cli.server.clone().expect("serveur resolu");
    let mut user_quit = false;
    let mut proxy_guard = ProxyGuard(None);
    let target = if cli.proxy {
        let (child, addr) = spawn_proxy(&server, &cli.opcodes, &cli.definitions, cli.major_patch, cli.tui, cli.log.as_ref())?;
        logln!("proxy insere : {addr} -> {server}");
        proxy_guard = ProxyGuard(Some(child));
        addr
    } else {
        server.clone()
    };
    let _ = &proxy_guard;
    logln!("connexion a {target}");
    let connect_result = match &cli.socks5 {
        Some(proxy) => {
            logln!("via SOCKS5 {proxy}");
            socks5_connect(proxy, &target)
        }
        None => target
            .to_socket_addrs()
            .context("resolution de l'adresse")
            .and_then(|mut addrs| addrs.next().with_context(|| format!("aucune adresse pour {target}")))
            .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(10)).context("connect")),
    };
    let mut stream = match connect_result {
        Ok(stream) => stream,
        Err(e) if cli.reconnect => {
            logln!("[{:7.3}] connexion echouee ({e}), on reessaie", t0.elapsed().as_secs_f64());
            reconnect_attempt += 1;
            continue 'reconnect;
        }
        Err(e) => return Err(e),
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    logln!("[{:7.3}] connecte", t0.elapsed().as_secs_f64());
    if let Some(since) = disconnected_since.take() {
        let down = since.elapsed();
        downtime_total += down;
        activity.log("reconnexion", &format!("hors ligne {}", human_duration(down.as_secs())));
        if cli.heartbeat_down_min > 0 && down >= Duration::from_secs(cli.heartbeat_down_min * 60) {
            let msg = format!("bot reconnecte apres {} hors ligne", human_duration(down.as_secs()));
            logln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
            if let Some(tx) = &discord { let _ = tx.send(format!(":warning: {msg}")); }
        }
    }
    match stream.local_addr() {
        Ok(local) => {
            logln!("[{:7.3}] IP source locale reellement utilisee : {local}", t0.elapsed().as_secs_f64());
            let ip = local.ip();
            let hint = if cli.socks5.is_some() {
                "via SOCKS5 -> l'IP publique de sortie est celle du proxy, pas celle-ci"
            } else if ip.to_string().starts_with("10.66.66.") {
                "= interface WireGuard : la connexion sort bien par le tunnel"
            } else if ip.is_loopback() {
                "= loopback : connexion detournee localement (relais)"
            } else {
                "= interface locale directe (pas de tunnel sur cette route)"
            };
            logln!("           {hint}");
        }
        Err(e) => logln!("[{:7.3}] local_addr indisponible : {e}", t0.elapsed().as_secs_f64()),
    }
    if let Ok(remote) = stream.peer_addr() {
        logln!("[{:7.3}] pair distant (peer) : {remote}", t0.elapsed().as_secs_f64());
    }

    if cli.send_first {
        logln!("[{:7.3}] mode send-first : on parle en premier (pas d'attente de greeting)", t0.elapsed().as_secs_f64());
    } else {
        logln!("[{:7.3}] en attente du greeting du serveur (il parle en premier)...", t0.elapsed().as_secs_f64());
        let greeting = read_exactly(&mut stream, MAGIC.len()).context("greeting du serveur")?;
        if greeting != MAGIC {
            bail!("greeting inattendu : {greeting:02x?} (attendu {MAGIC:02x?})");
        }
        logln!("[{:7.3}] <- greeting", t0.elapsed().as_secs_f64());
        hexdump("           ", &greeting);
    }

    let hs = ClientHandshake::new(random_key(), random_key()).with_constants(MODERN);
    logln!("[{:7.3}] -> cle client 1 ({} o)", t0.elapsed().as_secs_f64(), KEY_LEN);
    hexdump("           ", hs.first());
    stream.write_all(hs.first())?;
    let s1: [u8; KEY_LEN] = read_exactly(&mut stream, KEY_LEN)?.try_into().unwrap();
    logln!("[{:7.3}] <- cle serveur 1", t0.elapsed().as_secs_f64());
    hexdump("           ", &s1);
    logln!("[{:7.3}] -> cle client 2", t0.elapsed().as_secs_f64());
    hexdump("           ", hs.second());
    stream.write_all(hs.second())?;
    let s2: [u8; KEY_LEN] = read_exactly(&mut stream, KEY_LEN)?.try_into().unwrap();
    logln!("[{:7.3}] <- cle serveur 2", t0.elapsed().as_secs_f64());
    hexdump("           ", &s2);
    let (mut enc, mut dec): (Encrypting, Decrypting) = hs.finish(&s1, &s2).split();
    logln!("[{:7.3}] session chiffree etablie", t0.elapsed().as_secs_f64());

    let version = tera_protocol::Object::new()
        .with("version", tera_protocol::Value::Array(vec![
            tera_protocol::Object::new()
                .with("index", tera_protocol::Value::Int(0))
                .with("value", tera_protocol::Value::Int(cli.version_a)),
            tera_protocol::Object::new()
                .with("index", tera_protocol::Value::Int(1))
                .with("value", tera_protocol::Value::Int(cli.version_b)),
        ]));
    send(&mut stream, &mut enc, &registry, &opcodes, "C_CHECK_VERSION", &version, t0)?;

    let login = tera_protocol::Object::new()
        .with("unk1", tera_protocol::Value::Int(0))
        .with("unk2", tera_protocol::Value::Uint(0))
        .with("language", tera_protocol::Value::Uint(cli.language as u64))
        .with("patchVersion", tera_protocol::Value::Int(cli.patch_version as i64))
        .with("name", tera_protocol::Value::Str(account.clone()))
        .with("ticket", tera_protocol::Value::Bytes(ticket_bytes.clone()));
    send(&mut stream, &mut enc, &registry, &opcodes, "C_LOGIN_ARBITER", &login, t0)?;

    let mut packets = PacketBuffer::new();
    let mut buf = [0u8; 8192];
    let mut asked_list = false;
    let mut sent_hw = false;
    let mut selected = false;
    let mut player_loc: Option<tera_protocol::Value> = None;
    let mut player_w: i64 = 0;
    let mut in_world = false;
    let keepalive_interval = Duration::from_secs(50);
    let mut last_keepalive = Instant::now();
    let mut world = tera_world::World::new();
    let mut bot_name = String::new();
    let mut invite_type: i64 = cli.invite_type;
    let mut invite_unk2: i64 = 4;
    let mut reply_response: u64 = 1;
    let mut pending_sends: Vec<(Instant, &'static str, tera_protocol::Object)> = Vec::new();
    let mut bot_pos: Option<[f32; 3]> = None;
    let mut salute_cooldown: std::collections::HashMap<u64, Instant> = std::collections::HashMap::new();
    let mut user_ids: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut user_locs: std::collections::HashMap<u64, [f32; 3]> = std::collections::HashMap::new();
    let mut heal = heal::HealBot::new(heal::HealConfig {
        enabled: cli.heal,
        focus_heal: cli.focus_heal_id,
        healing_immersion: cli.healing_immersion_id,
        hp_limit: cli.hp_limit,
        max_distance: cli.heal_range,
        attack_enabled: cli.farm,
        ..heal::HealConfig::default()
    });
    let heal_packets = [
        "S_LOGIN", "S_SPAWN_ME", "S_PLAYER_STAT_UPDATE", "S_PARTY_MEMBER_LIST", "S_PARTY_MEMBER_CHANGE_HP",
        "S_SPAWN_USER", "S_USER_LOCATION", "S_PARTY_MEMBER_INTERVAL_POS_UPDATE",
        "S_EACH_SKILL_RESULT", "S_START_COOLTIME_SKILL", "S_CANNOT_START_SKILL", "S_SKILL_LIST",
        "S_LEAVE_PARTY", "S_LEAVE_PARTY_MEMBER", "S_BAN_PARTY_MEMBER", "S_LOGOUT_PARTY_MEMBER",
        "S_SPAWN_NPC", "S_DESPAWN_NPC", "S_NPC_LOCATION", "S_ACTION_STAGE", "S_DIALOG",
        "S_CREATURE_LIFE", "S_NPC_STATUS", "S_BOSS_GAGE_INFO",
        "S_ABNORMALITY_BEGIN", "S_ABNORMALITY_REFRESH", "S_ABNORMALITY_END",
        "S_CREST_INFO",
    ];
    let mut last_heal_tick = Instant::now();
    let heal_tick_interval = Duration::from_millis(500);
    let mut commander_party: Option<(u64, u64)> = None;
    let mut move_target: Option<[f32; 3]> = None;
    let mut follow_id: Option<u64> = None;
    let mut auto_follow = true;
    let mut combat_driving = false;
    let move_step_interval = Duration::from_millis(350);
    let move_dest_lookahead = 160.0f32;
    let mut last_step = Instant::now();
    let world_packets = [
        "S_LOGIN", "S_SPAWN_ME", "S_LOAD_TOPO", "S_PLAYER_STAT_UPDATE", "S_SPAWN_NPC",
        "S_SPAWN_USER", "S_NPC_LOCATION", "S_USER_LOCATION", "S_DESPAWN_NPC",
        "S_DESPAWN_USER", "S_CREATURE_LIFE", "S_CREATURE_CHANGE_HP", "S_NPC_STATUS",
        "S_ITEMLIST", "C_PLAYER_LOCATION",
    ];
    let auto_packets = [
        "S_AVAILABLE_EVENT_MATCHING_LIST", "S_COMPLETE_EVENT_MATCHING_QUEST",
        "S_UPDATE_GUILD_QUEST_STATUS", "S_FIELD_EVENT_PROGRESS_INFO", "S_FIELD_POINT_INFO",
    ];

    let stdin_rx = if cli.chat && !cli.tui {
        let (tx, rx) = mpsc::channel::<String>();
        thread::spawn(move || {
            let stdin = std::io::stdin();
            loop {
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let text = line.trim_end().to_string();
                if !text.is_empty() && tx.send(text).is_err() {
                    break;
                }
            }
        });
        logln!("=== chat interactif : tape un message + Entree (ou !help pour les commandes serveur) ===");
        Some(rx)
    } else {
        None
    };
    if cli.chat || cli.tui {
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    }

    let mut pending_command: std::collections::VecDeque<(Access, ReplyTarget, String)> = std::collections::VecDeque::new();
    let mut online_admins: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut guild_members: std::collections::HashMap<String, GuildMember> = std::collections::HashMap::new();
    let mut gm_alerted: std::collections::HashMap<String, Instant> = std::collections::HashMap::new();
    let mut nearby_players: std::collections::HashMap<u64, NearbyPlayer> = std::collections::HashMap::new();
    let mut spawned_bosses: std::collections::HashMap<u64, BossSpawn> = std::collections::HashMap::new();
    let mut last_parcel = Instant::now() - Duration::from_secs(3600);
    let mut guild_info = GuildInfo::default();
    let mut ping_rtt: Option<u32> = None;
    let mut ping_jitter: u32 = 0;
    let mut ping_loss = false;
    let mut last_ping_fetch = Instant::now() - Duration::from_secs(10);
    let mut afk_replied: std::collections::HashMap<String, Instant> = std::collections::HashMap::new();
    let mut faq_replied: std::collections::HashMap<String, Instant> = std::collections::HashMap::new();
    let mut my_server_id: u64 = 0;
    let mut watchdog_zone_prev: i32 = -1;
    let mut last_enter: Option<Instant> = None;
    let mut last_guild_refresh = Instant::now() - Duration::from_secs(120);
    let guild_refresh_interval = Duration::from_secs(60);
    let mut last_announce = Instant::now();
    let mut last_state_push = Instant::now();
    let mut net_mark = Instant::now();
    let mut net_prev = (0u64, 0u64, 0u64, 0u64);
    let mut net_rates = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let keep_alive = cli.listen == 0 || cli.chat || cli.tui;
    let listen_secs = cli.listen.min(86_400);
    let deadline = Instant::now() + Duration::from_secs(listen_secs.max(1));
    if !cli.chat && !cli.tui {
        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    }
    let inactivity_limit = Duration::from_secs(60);
    let mut last_recv = Instant::now();
    'session: while keep_alive || Instant::now() < deadline {
        {
            let now = Instant::now();
            let mut index = 0;
            while index < pending_sends.len() {
                if pending_sends[index].0 <= now {
                    let (_, opcode_name, packet_object) = pending_sends.remove(index);
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, opcode_name, &packet_object, t0)? {
                        break 'session;
                    }
                } else {
                    index += 1;
                }
            }
        }
        {
            let now = Instant::now();
            let out = auto.tick(now);
            for (delay_ms, opcode_name, packet_object) in out.sends {
                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
            }
            for line in out.logs {
                logln!("[{:7.3}] auto: {line}", t0.elapsed().as_secs_f64());
                if let Some(handle) = &ui {
                    handle.act(line);
                }
            }
        }
        if in_world && last_guild_refresh.elapsed() >= guild_refresh_interval {
            last_guild_refresh = Instant::now();
            let empty = tera_protocol::Object::new();
            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REQUEST_GUILD_MEMBER_LIST", &empty, t0)? {
                break 'session;
            }
            if registry.get("C_REQUEST_GUILD_INFO").is_some() {
                let req = tera_protocol::Object::new()
                    .with("guildId", tera_protocol::Value::Int(guild_info.id))
                    .with("type", tera_protocol::Value::Int(2));
                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REQUEST_GUILD_INFO", &req, t0)? {
                    break 'session;
                }
            }
        }
        if broker.is_some() && in_world && !broker_open_tried {
            if let Some((gid, tpl)) = world.nearest_villager().map(|npc| (npc.game_id, npc.template_id)) {
                if let Some(op) = opcodes.code("C_NPC_CONTACT") {
                    broker_open_tried = true;
                    send_raw(&mut stream, &mut enc, &registry, "C_NPC_CONTACT", broker::build_npc_contact(op, gid), t0)?;
                    logln!("[{:7.3}] broker: contact auto PNJ broker (template {tpl}, gid {gid}) - attente dialogue", t0.elapsed().as_secs_f64());
                    if let Some(h) = &ui { h.act(format!("broker: contact PNJ {tpl}")); }
                }
            }
        }
        if let Some(b) = broker.as_mut() {
            let out = b.tick(Instant::now());
            for p in out.packets {
                send_raw(&mut stream, &mut enc, &registry, p.name, p.bytes, t0)?;
            }
            for m in out.messages {
                logln!("[{:7.3}] broker: {m}", t0.elapsed().as_secs_f64());
                if let Some(h) = &ui {
                    h.act(format!("broker: {m}"));
                }
            }
        }
        if in_world && last_timer_tick.elapsed() >= Duration::from_secs(5) {
            last_timer_tick = Instant::now();
            for fire in state.tick_timers() {
                let text = match fire {
                    services::TimerFire::Lead { name, remaining } => format!("[rappel] {name} dans {}min", (remaining + 59) / 60),
                    services::TimerFire::Fire { name } => format!("[minuteur] {name} maintenant !"),
                };
                logln!("[{:7.3}] {text}", t0.elapsed().as_secs_f64());
                if let Some(handle) = &ui { handle.act(text.clone()); }
                let announce = tera_protocol::Object::new()
                    .with("channel", tera_protocol::Value::Uint(2))
                    .with("message", tera_protocol::Value::Str(text));
                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &announce, t0)? {
                    break 'session;
                }
            }
        }
        if in_world && cli.auto_parcel && opcodes.code("C_LIST_PARCEL").is_some()
            && last_parcel.elapsed() >= Duration::from_secs(cli.parcel_interval.max(30))
        {
            last_parcel = Instant::now();
            let req = tera_protocol::Object::new()
                .with("unk1", tera_protocol::Value::Uint(0))
                .with("page", tera_protocol::Value::Uint(1))
                .with("filter", tera_protocol::Value::Uint(0));
            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LIST_PARCEL", &req, t0)? {
                break 'session;
            }
        }
        if (cli.asura_relay || cli.asura_keep_tunnel) && last_ping_fetch.elapsed() >= Duration::from_secs(3) {
            last_ping_fetch = Instant::now();
            if let Ok(stats) = asura::exitlag_ping(&cli.asura_cdp_host, cli.asura_cdp_port, Duration::from_secs(2)) {
                match stats {
                    Some((rtt, jitter, loss)) => { ping_rtt = Some(rtt); ping_jitter = jitter; ping_loss = loss; ping_csv.record(rtt, jitter, loss); }
                    None => ping_rtt = None,
                }
            }
            if cli.ping_alert_ms > 0 {
                if let Some(rtt) = ping_rtt {
                    let bad = rtt > cli.ping_alert_ms || ping_loss;
                    let fresh = last_ping_alert.map(|t| t.elapsed() > Duration::from_secs(120)).unwrap_or(true);
                    if bad && fresh {
                        last_ping_alert = Some(Instant::now());
                        let msg = format!("ping degrade : {rtt}ms (jitter {ping_jitter}ms){}", if ping_loss { " + perte de paquets" } else { "" });
                        logln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
                        activity.log("ping", &msg);
                        if let Some(handle) = &ui { handle.log(tui::LogKind::Ping, msg.clone()); }
                        if let Some(tx) = &discord { let _ = tx.send(format!(":signal_strength: {msg}")); }
                    }
                }
            }
        }
        if in_world && cli.announce_interval > 0 && !cli.announce_text.is_empty()
            && last_announce.elapsed() >= Duration::from_secs(cli.announce_interval * 60)
        {
            last_announce = Instant::now();
            let announce = tera_protocol::Object::new()
                .with("channel", tera_protocol::Value::Uint(2))
                .with("message", tera_protocol::Value::Str(cli.announce_text.clone()));
            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &announce, t0)? {
                break 'session;
            }
            logln!("[{:7.3}] annonce periodique postee", t0.elapsed().as_secs_f64());
        }
        if let Some(handle) = &ui {
            if last_state_push.elapsed() >= Duration::from_millis(400) {
                last_state_push = Instant::now();
                let me = world.player.location;
                let mut nearby: Vec<tui::NearbyEntry> = nearby_players
                    .values()
                    .map(|p| tui::NearbyEntry {
                        name: p.name.clone(),
                        class: class_name(p.class_id).to_string(),
                        level: p.level,
                        dist: p.location.distance_to(me),
                        dx: p.location.x - me.x,
                        dy: p.location.y - me.y,
                        gm: p.gm,
                        gm_invisible: p.gm_invisible,
                        guildmate: guild_members.contains_key(&p.name.to_lowercase()),
                        guild: p.guild.clone(),
                    })
                    .collect();
                nearby.sort_by(|a, b| a.dist.partial_cmp(&b.dist).unwrap_or(std::cmp::Ordering::Equal));
                let gm_near = nearby.iter().filter(|e| e.gm || e.gm_invisible).count();
                let mut guild_online: Vec<tui::GuildEntry> = guild_members
                    .values()
                    .filter(|m| m.online)
                    .map(|m| tui::GuildEntry {
                        name: m.name.clone(),
                        class: class_name(m.class).to_string(),
                        level: m.level,
                        admin: admins.contains(&m.name.to_lowercase()),
                    })
                    .collect();
                guild_online.sort_by(|a, b| b.admin.cmp(&a.admin).then_with(|| a.name.cmp(&b.name)));
                let traffic = traffic::snapshot(150);
                let dt = net_mark.elapsed().as_secs_f64().max(0.001);
                if dt >= 0.9 {
                    let now_totals = (traffic.in_bytes, traffic.out_bytes, traffic.in_packets, traffic.out_packets);
                    net_rates = (
                        (now_totals.0.saturating_sub(net_prev.0)) as f64 / dt,
                        (now_totals.1.saturating_sub(net_prev.1)) as f64 / dt,
                        (now_totals.2.saturating_sub(net_prev.2)) as f64 / dt,
                        (now_totals.3.saturating_sub(net_prev.3)) as f64 / dt,
                    );
                    net_prev = now_totals;
                    net_mark = Instant::now();
                }
                let mut entities: Vec<tui::EntityEntry> = world
                    .npcs_by_distance()
                    .into_iter()
                    .take(60)
                    .map(|e| tui::EntityEntry {
                        label: if e.villager { format!("villager {}", e.template_id) } else { format!("mob {}", e.template_id) },
                        level: e.level,
                        hp_pct: e.hp_pct(),
                        dist: e.location.distance_to(me),
                        dx: e.location.x - me.x,
                        dy: e.location.y - me.y,
                        boss: e.is_boss_candidate(),
                        enraged: e.enraged,
                        aggro_me: e.target == world.player.game_id && world.player.game_id != 0,
                        villager: e.villager,
                        alive: e.alive,
                    })
                    .collect();
                entities.sort_by(|a, b| {
                    b.boss.cmp(&a.boss)
                        .then_with(|| b.aggro_me.cmp(&a.aggro_me))
                        .then_with(|| a.dist.partial_cmp(&b.dist).unwrap_or(std::cmp::Ordering::Equal))
                });
                let items: Vec<tui::ItemEntry> = world
                    .inventory_sorted()
                    .into_iter()
                    .take(300)
                    .map(|it| tui::ItemEntry {
                        name: item_names.get(&(it.id as u32)).cloned().unwrap_or_else(|| format!("#{}", it.id)),
                        amount: it.amount,
                        enchant: it.enchant,
                    })
                    .collect();
                handle.state(tui::Snapshot {
                    name: bot_name.clone(),
                    level: world.player.level,
                    class: if world.player.template_id > 0 { class_name(class_from_template(world.player.template_id as i64)).to_string() } else { String::new() },
                    zone_label: zone_names.label(world.player.zone as i64),
                    hp: world.player.hp.max(0) as u64,
                    max_hp: world.player.max_hp.max(0) as u64,
                    mp: world.player.mp.max(0) as u64,
                    max_mp: world.player.max_mp.max(0) as u64,
                    auto: auto.status(),
                    guardian: auto.guardian_summary(),
                    real_players: nearby_players.len(),
                    npcs: world.npc_count(),
                    gm_near,
                    server: server.clone(),
                    uptime: human_duration(t0.elapsed().as_secs()),
                    reconnects: reconnect_attempt,
                    ping_rtt,
                    ping_jitter,
                    ping_loss,
                    pos_x: me.x,
                    pos_y: me.y,
                    pos_z: me.z,
                    nearby,
                    guild_name: guild_info.name.clone(),
                    guild_master: guild_info.master.clone(),
                    guild_level: guild_info.level,
                    guild_funds: guild_info.funds,
                    guild_motd: guild_info.motd.clone(),
                    guild_online,
                    guild_total: guild_members.len(),
                    members: heal
                        .party_view()
                        .into_iter()
                        .map(|(name, hp_pct, dead)| tui::MemberEntry { name, hp_pct, dead })
                        .collect(),
                    cooldowns: heal
                        .cooldown_view(Instant::now())
                        .into_iter()
                        .map(|(name, remaining)| tui::CooldownEntry { name, remaining })
                        .collect(),
                    farm_xp_hr: session_stats.xp_per_hour(),
                    farm_kills: session_stats.kills(),
                    farm_kills_hr: session_stats.kills_per_hour(),
                    farm_loot: session_stats.loot(),
                    farm_deaths: session_stats.deaths(),
                    farm_eta: session_stats.eta_level(),
                    farm_level_pct: session_stats.level_pct(),
                    money: world.money,
                    inventory_slots: world.inventory_slots,
                    boss_count: world.boss_count(),
                    aggro_on_me: world.aggro_on_me(),
                    net_in_rate: net_rates.0,
                    net_out_rate: net_rates.1,
                    net_in_pps: net_rates.2,
                    net_out_pps: net_rates.3,
                    traffic,
                    entities,
                    items,
                });
            }
        }
        while let Some((access, target, text)) = pending_command.pop_front() {
            let command = text.split_whitespace().next().unwrap_or("").to_lowercase();
            let argument = text.splitn(2, char::is_whitespace).nth(1).unwrap_or("").trim().to_string();
            let default_who = match &target {
                ReplyTarget::Whisper { name, .. } => name.clone(),
                ReplyTarget::Guild { author } => author.clone(),
                ReplyTarget::Local => cli.commander.clone(),
            };
            let who_default = |argument: &str| -> String {
                if argument.is_empty() { default_who.clone() } else { argument.to_string() }
            };
            let reply = |body: String| -> Option<(&'static str, tera_protocol::Object)> {
                match &target {
                    ReplyTarget::Whisper { server, name } => Some(("C_WHISPER", tera_protocol::Object::new()
                        .with("targetServer", tera_protocol::Value::Uint(*server))
                        .with("target", tera_protocol::Value::Str(name.clone()))
                        .with("message", tera_protocol::Value::Str(body)))),
                    ReplyTarget::Guild { .. } => Some(("C_CHAT", tera_protocol::Object::new()
                        .with("channel", tera_protocol::Value::Uint(2))
                        .with("message", tera_protocol::Value::Str(body)))),
                    ReplyTarget::Local => {
                        logln!("[{:7.3}] [bot] {body}", t0.elapsed().as_secs_f64());
                        if let Some(handle) = &ui { handle.act(format!("[bot] {body}")); }
                        None
                    }
                }
            };
            let admin_only = matches!(
                command.as_str(),
                "say" | "gsay" | "g" | "w" | "tell" | "repete" | "pm" | "whisper" | "heal" | "soin" | "dodge"
                    | "esquive" | "healinfo" | "healset" | "testheal" | "castheal" | "testh"
                    | "rtype" | "itype" | "iunk2" | "enter" | "entrer" | "donjon" | "auto"
                    | "bye" | "logout" | "quit"
                    | "come" | "viens" | "follow" | "suis" | "goto" | "stop"
                    | "watch" | "timer" | "roster" | "grep" | "parcel" | "colis"
                    | "inactifs" | "inactives" | "menage" | "bbuy" | "broker"
                    | "netcsv" | "netexport"
            );
            let reply_packet: Option<(&'static str, tera_protocol::Object)> = if admin_only && access != Access::Admin {
                reply("desole, cette commande est reservee aux admins".to_string())
            } else {
                match command.as_str() {
                    "de" | "dice" | "des" => reply(fun::dice(&argument)),
                    "pile" | "face" | "flip" | "pf" => reply(fun::flip()),
                    "roll" => reply(fun::roll()),
                    "8ball" | "boule" | "magic8" => reply(fun::eightball(&argument)),
                    "pfc" | "rps" | "chifoumi" => reply(fun::rps(&argument)),
                    "choisis" | "choose" | "choix" => reply(fun::choose(&argument)),
                    "slot" | "machine" | "jackpot" => reply(fun::slot()),
                    "fortune" | "cookie" => reply(fun::fortune()),
                    "fun" | "jeux" => reply(fun::help()),
                    "salut" | "greet" | "salue" => {
                        if !cli.salute {
                            reply("salut desactive (lance avec --salute)".to_string())
                        } else if world.player.game_id == 0 {
                            reply("pas encore en jeu".to_string())
                        } else {
                            let loc = bot_pos.unwrap_or([world.player.location.x, world.player.location.y, world.player.location.z]);
                            let arg = argument.trim();
                            let target = if arg.is_empty() {
                                None
                            } else {
                                let key = arg.to_lowercase();
                                user_ids
                                    .iter()
                                    .find(|(name, _)| name.eq_ignore_ascii_case(arg))
                                    .or_else(|| user_ids.iter().find(|(name, _)| name.to_lowercase().contains(&key)))
                                    .map(|(name, id)| (name.clone(), *id))
                            };
                            let aimed = target.as_ref().and_then(|(_, id)| {
                                heal.ally_loc(*id).or_else(|| user_locs.get(id).copied()).map(|tloc| (*id, tloc))
                            });
                            let packet = match aimed {
                                Some((id, tloc)) => greet::build_targeted(loc, greet::face(loc, tloc), id, tloc),
                                None => greet::build_with(loc, player_w, false),
                            };
                            send_raw(&mut stream, &mut enc, &registry, "C_START_INSTANCE_SKILL", packet, t0)?;
                            logln!("[{:7.3}] salut envoye (loc {loc:?}, cible {:?})", t0.elapsed().as_secs_f64(), aimed.map(|(id, _)| id));
                            match &target {
                                Some((name, _)) => reply(format!("salut {name}")),
                                None => reply("salut".to_string()),
                            }
                        }
                    }
                    "bbuy" | "broker" => {
                        if broker.is_none() {
                            reply("broker-buy desactive (lance avec --broker)".to_string())
                        } else {
                            let mut sub = argument.split_whitespace();
                            let head = sub.next().unwrap_or("");
                            match head {
                                "npc" | "npcs" | "pnj" => {
                                    let mut lines: Vec<String> = broker_npcs
                                        .iter()
                                        .map(|(gid, (nm, tpl, _))| format!("{} [tpl {tpl}] gid {gid}", if nm.is_empty() { "?" } else { nm.as_str() }))
                                        .collect();
                                    lines.sort();
                                    lines.truncate(25);
                                    reply(if lines.is_empty() { "aucun PNJ proche".to_string() } else { lines.join(" | ") })
                                }
                                "open" => {
                                    let ctype = sub.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(28);
                                    let value = sub.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                                    let target = sub.next().and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                                    let b = broker.as_mut().unwrap();
                                    match b.build_open(ctype, target, value) {
                                        Some(bytes) => {
                                            send_raw(&mut stream, &mut enc, &registry, "C_REQUEST_CONTRACT", bytes, t0)?;
                                            let out = b.restart_scan();
                                            for p in out.packets {
                                                send_raw(&mut stream, &mut enc, &registry, p.name, p.bytes, t0)?;
                                            }
                                            reply(format!("C_REQUEST_CONTRACT type={ctype} value={value} target={target} envoye, scan relance"))
                                        }
                                        None => reply("echec construction C_REQUEST_CONTRACT (opcode absent ?)".to_string()),
                                    }
                                }
                                "contact" => match sub.next().and_then(|s| s.parse::<u64>().ok()) {
                                    Some(gid) => match opcodes.code("C_NPC_CONTACT") {
                                        Some(op) => {
                                            send_raw(&mut stream, &mut enc, &registry, "C_NPC_CONTACT", broker::build_npc_contact(op, gid), t0)?;
                                            let out = broker.as_mut().unwrap().restart_scan();
                                            for p in out.packets {
                                                send_raw(&mut stream, &mut enc, &registry, p.name, p.bytes, t0)?;
                                            }
                                            reply(format!("C_NPC_CONTACT {gid} envoye, scan relance"))
                                        }
                                        None => reply("opcode C_NPC_CONTACT absent".to_string()),
                                    },
                                    None => reply("usage: bbuy contact <gameId> (voir 'bbuy npc')".to_string()),
                                },
                                _ => {
                                    let out = broker.as_mut().unwrap().command(&argument);
                                    for p in out.packets {
                                        send_raw(&mut stream, &mut enc, &registry, p.name, p.bytes, t0)?;
                                    }
                                    reply(out.messages.join(" | "))
                                }
                            }
                        }
                    }
                    "come" | "viens" => {
                        let who = who_default(&argument);
                        let position = user_ids.get(&who).copied()
                            .and_then(|id| heal.ally_loc(id).or_else(|| user_locs.get(&id).copied()))
                            .or_else(|| heal.first_target().and_then(|id| heal.ally_loc(id)));
                        match position {
                            Some(position) => { move_target = Some(position); follow_id = None; auto_follow = false; reply(format!("j'arrive vers {who}")) }
                            None => reply(format!("je ne vois pas {who}")),
                        }
                    }
                    "follow" | "suis" => {
                        let who = who_default(&argument);
                        match user_ids.get(&who).copied().or_else(|| heal.first_target()) {
                            Some(id) => { follow_id = Some(id); auto_follow = true; reply(format!("je te suis {who}")) }
                            None => reply(format!("je ne vois pas {who}")),
                        }
                    }
                    "goto" => {
                        let coords: Vec<f32> = argument.split_whitespace().filter_map(|part| part.parse().ok()).collect();
                        if coords.len() >= 2 {
                            let z = coords.get(2).copied().or_else(|| bot_pos.map(|p| p[2])).unwrap_or(0.0);
                            move_target = Some([coords[0], coords[1], z]);
                            follow_id = None;
                            auto_follow = false;
                            reply("j'y vais".to_string())
                        } else {
                            reply("usage: goto x y".to_string())
                        }
                    }
                    "stop" => { move_target = None; follow_id = None; auto_follow = false; reply("stop (suivi auto off, 'follow' pour reprendre)".to_string()) }
                    "invite" | "inviter" | "grp" | "groupe" | "party" => {
                        let who = who_default(&argument);
                        let request = tera_protocol::Object::new()
                            .with("type", tera_protocol::Value::Int(invite_type))
                            .with("unk2", tera_protocol::Value::Int(invite_unk2))
                            .with("name", tera_protocol::Value::Str(who.clone()))
                            .with("data", tera_protocol::Value::Bytes(vec![0u8]));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REQUEST_CONTRACT", &request, t0)? {
                            break 'session;
                        }
                        reply(format!("invite {who} (type {invite_type}), accepte"))
                    }
                    "lead" | "leader" | "chef" => {
                        match commander_party {
                            Some((server_id, player_id)) => {
                                let manager = tera_protocol::Object::new()
                                    .with("serverId", tera_protocol::Value::Uint(server_id))
                                    .with("playerId", tera_protocol::Value::Uint(player_id));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHANGE_PARTY_MANAGER", &manager, t0)? {
                                    break 'session;
                                }
                                reply("je te passe le lead du groupe".to_string())
                            }
                            None => reply("je ne te trouve pas dans le groupe (invite-toi d'abord)".to_string()),
                        }
                    }
                    "leave" | "leaveparty" | "quitgrp" | "quitgroupe" => {
                        let leave = tera_protocol::Object::new();
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LEAVE_PARTY", &leave, t0)? {
                            break 'session;
                        }
                        reply("j'ai quitte tout groupe".to_string())
                    }
                    "testheal" | "castheal" | "testh" => {
                        let target_id = match &target {
                            ReplyTarget::Whisper { name, .. } => user_ids.get(name).copied(),
                            _ => None,
                        }.or_else(|| heal.first_target()).unwrap_or(0);
                        if target_id == 0 {
                            reply("je ne te vois pas encore (bouge un peu pour spawn)".to_string())
                        } else {
                            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
                            let now = Instant::now();
                            for (delay_ms, opcode_name, packet_object) in heal.force_heal(target_id) {
                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                            }
                            reply(format!("test heal lance (target {target_id})"))
                        }
                    }
                    "enter" | "entrer" | "donjon" => {
                        let choice = argument.trim().parse::<usize>().ok();
                        match heal.enter_dungeon(choice) {
                            Some((_, opcode_name, packet_object)) => {
                                last_enter = Some(Instant::now());
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, opcode_name, &packet_object, t0)? {
                                    break 'session;
                                }
                                reply(match choice {
                                    Some(n) => format!("je contacte le portail, bouton {n}"),
                                    None => "je contacte le portail (dis 'enter N' pour choisir dans le menu)".to_string(),
                                })
                            }
                            None => reply("je ne vois pas de portail (Teleportal) ; rapproche-toi du teleporteur du donjon".to_string()),
                        }
                    }
                    "say" => {
                        let message = tera_protocol::Object::new()
                            .with("channel", tera_protocol::Value::Uint(u64::from(cli.chat_channel)))
                            .with("message", tera_protocol::Value::Str(argument));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &message, t0)? {
                            break 'session;
                        }
                        None
                    }
                    "gsay" | "g" => {
                        let message = tera_protocol::Object::new()
                            .with("channel", tera_protocol::Value::Uint(2))
                            .with("message", tera_protocol::Value::Str(argument));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &message, t0)? {
                            break 'session;
                        }
                        None
                    }
                    "w" | "tell" | "repete" => reply(argument),
                    "pm" | "whisper" => {
                        let mut parts = argument.splitn(2, char::is_whitespace);
                        let target = parts.next().unwrap_or("").trim().to_string();
                        let body = parts.next().unwrap_or("").trim().to_string();
                        if target.is_empty() || body.is_empty() {
                            reply("usage: pm <nom> <message>".to_string())
                        } else {
                            let whisper = tera_protocol::Object::new()
                                .with("targetServer", tera_protocol::Value::Uint(my_server_id))
                                .with("target", tera_protocol::Value::Str(target.clone()))
                                .with("message", tera_protocol::Value::Str(body));
                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &whisper, t0)? {
                                break 'session;
                            }
                            reply(format!("whisper -> {target}"))
                        }
                    }
                    "netcsv" | "netexport" => {
                        let snap = traffic::snapshot(1_000_000);
                        let mut csv = String::from("direction,opcode,name,packets,bytes,idle_ms\n");
                        for row in &snap.top {
                            csv.push_str(&format!(
                                "{},{},{},{},{},{}\n",
                                if row.incoming { "in" } else { "out" },
                                row.opcode,
                                row.name,
                                row.packets,
                                row.bytes,
                                row.idle_ms
                            ));
                        }
                        let path = "data/net-stats.csv";
                        match std::fs::write(path, csv) {
                            Ok(()) => reply(format!("net stats -> {path} ({} opcodes)", snap.top.len())),
                            Err(e) => reply(format!("export failed: {e}")),
                        }
                    }
                    "where" | "pos" => {
                        let p = world.player.location;
                        reply(format!("{} - pos ({:.0}, {:.0}, {:.0})", zone_names.label(world.player.zone as i64), p.x, p.y, p.z))
                    }
                    "ping" => reply(match ping_rtt {
                        Some(rtt) => format!("ping {rtt}ms (jitter {ping_jitter}ms){}", if ping_loss { " perte de paquets" } else { "" }),
                        None => "ping indisponible".to_string(),
                    }),
                    "guild" | "guilde" => {
                        if guild_info.name.is_empty() {
                            reply("pas d'info de guilde encore".to_string())
                        } else {
                            let online = guild_members.values().filter(|m| m.online).count();
                            reply(format!(
                                "{} niv.{} - chef {} - {}/{} membres ({} en ligne) - {} or{}",
                                guild_info.name, guild_info.level, guild_info.master,
                                guild_info.characters, guild_info.accounts, online,
                                guild_info.funds / 10_000,
                                if guild_info.motd.is_empty() { String::new() } else { format!(" - \"{}\"", guild_info.motd) }
                            ))
                        }
                    }
                    "ou" | "locate" => {
                        let who = argument.trim().to_lowercase();
                        if who.is_empty() {
                            reply("usage: ou <pseudo>".to_string())
                        } else {
                            match guild_members.get(&who) {
                                Some(m) if m.online => reply(format!("{} ({} niv.{}) en ligne, {}", m.name, class_name(m.class), m.level, zone_names.label(m.zone))),
                                Some(m) => reply(format!("{} ({} niv.{}) hors ligne", m.name, class_name(m.class), m.level)),
                                None => reply(format!("{who} n'est pas dans la guilde")),
                            }
                        }
                    }
                    "mail" | "msg" | "laisse" => {
                        let rest = argument.trim();
                        match rest.split_once(char::is_whitespace) {
                            Some((target, body)) if !body.trim().is_empty() => {
                                let from = if default_who.is_empty() { "anonyme".to_string() } else { default_who.clone() };
                                let count = state.add_mail(target, &from, body.trim());
                                reply(format!("message garde pour {target} (il l'aura a sa prochaine connexion) [{count} en attente]"))
                            }
                            _ => reply("usage: mail <pseudo> <message>".to_string()),
                        }
                    }
                    "seen" | "vu" => {
                        let who = argument.trim();
                        if who.is_empty() {
                            reply("usage: seen <pseudo>".to_string())
                        } else if guild_members.get(&who.to_lowercase()).map(|m| m.online).unwrap_or(false) {
                            reply(format!("{who} est en ligne la"))
                        } else {
                            match state.seen_ago(who) {
                                Some(secs) => reply(format!("{who} vu {}", services::ago(secs))),
                                None => reply(format!("jamais vu {who} depuis que je tourne")),
                            }
                        }
                    }
                    "gtime" | "timers" => reply(state.timers_summary()),
                    "watch" => {
                        let mut parts = argument.split_whitespace();
                        match parts.next().unwrap_or("list") {
                            "add" => match parts.next() {
                                Some(n) => reply(if state.watch_add(n) { format!("{n} ajoute a la watchlist") } else { format!("{n} y est deja") }),
                                None => reply("usage: watch add <pseudo>".to_string()),
                            },
                            "del" | "rm" => match parts.next() {
                                Some(n) => reply(if state.watch_del(n) { format!("{n} retire de la watchlist") } else { format!("{n} n'y etait pas") }),
                                None => reply("usage: watch del <pseudo>".to_string()),
                            },
                            "list" | "" => reply(if state.watch.is_empty() { "watchlist vide".to_string() } else { format!("watchlist : {}", state.watch.join(", ")) }),
                            other => reply(format!("usage: watch add|del|list (recu '{other}')")),
                        }
                    }
                    "timer" => {
                        let mut parts = argument.split_whitespace();
                        match parts.next().unwrap_or("list") {
                            "add" => {
                                let name = parts.next().unwrap_or("");
                                let mins: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                                let repeat: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                                let lead: i64 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(5);
                                if name.is_empty() || mins <= 0 {
                                    reply("usage: timer add <nom> <dans_min> [repeat_min] [rappel_min]".to_string())
                                } else {
                                    state.timer_add(name, mins * 60, repeat * 60, lead * 60);
                                    reply(format!("minuteur '{name}' dans {mins}min{}", if repeat > 0 { format!(", repete toutes les {repeat}min") } else { String::new() }))
                                }
                            }
                            "del" | "rm" => match parts.next() {
                                Some(n) => reply(if state.timer_del(n) { format!("minuteur '{n}' supprime") } else { format!("pas de minuteur '{n}'") }),
                                None => reply("usage: timer del <nom>".to_string()),
                            },
                            "list" | "" => reply(state.timers_summary()),
                            other => reply(format!("usage: timer add|del|list (recu '{other}')")),
                        }
                    }
                    "roster" | "export" => {
                        let mut rows: Vec<(bool, String)> = guild_members
                            .values()
                            .map(|m| (m.online, format!(
                                "{}\t{}\tniv {}\trang {}\tcontrib {}\t{}",
                                m.name, class_name(m.class), m.level, m.rank, m.contribution,
                                if m.online { "en ligne".to_string() } else { format!("hors ligne {}", state.seen_ago(&m.name).map(services::ago).unwrap_or_default()) }
                            )))
                            .collect();
                        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                        let header = format!("{} - niv {} - {}/{} membres\n", guild_info.name, guild_info.level, guild_info.characters, guild_info.accounts);
                        let body: String = std::iter::once(header).chain(rows.into_iter().map(|(_, s)| format!("{s}\n"))).collect();
                        let path = std::path::PathBuf::from("roster.txt");
                        match std::fs::write(&path, &body) {
                            Ok(()) => {
                                if let Some(tx) = &discord { let _ = tx.send(format!("Roster {}:\n```\n{}```", guild_info.name, &body[..body.len().min(1800)])); }
                                reply(format!("roster exporte dans roster.txt ({} membres)", guild_members.len()))
                            }
                            Err(e) => reply(format!("echec export roster : {e}")),
                        }
                    }
                    "grep" | "cherche-chat" => {
                        let needle = argument.trim().to_lowercase();
                        if needle.is_empty() {
                            reply("usage: grep <mot>".to_string())
                        } else {
                            match cli.chat_log.as_ref().and_then(|p| std::fs::read_to_string(p).ok()) {
                                Some(content) => {
                                    let hits: Vec<&str> = content.lines().filter(|l| l.to_lowercase().contains(&needle)).collect();
                                    if hits.is_empty() {
                                        reply(format!("aucune ligne avec '{needle}'"))
                                    } else {
                                        let total = hits.len();
                                        let tail: Vec<String> = hits.iter().rev().take(5).rev().map(|s| s.to_string()).collect();
                                        reply(format!("{total} resultats, derniers :\n{}", tail.join("\n")))
                                    }
                                }
                                None => reply("pas de --chat-log configure (rien a chercher)".to_string()),
                            }
                        }
                    }
                    "who" | "qui" => reply(format!("{} joueurs autour", nearby_players.len())),
                    "near" | "autour" | "proche" => {
                        if nearby_players.is_empty() {
                            reply("personne autour".to_string())
                        } else {
                            let me = world.player.location;
                            let mut list: Vec<(f32, String)> = nearby_players
                                .values()
                                .map(|p| {
                                    let tag = if p.gm_invisible {
                                        " [GM-INV]"
                                    } else if p.gm {
                                        " [GM]"
                                    } else if guild_members.contains_key(&p.name.to_lowercase()) {
                                        " [guilde]"
                                    } else {
                                        ""
                                    };
                                    let dist = p.location.distance_to(me);
                                    (dist, format!("{}{} {} niv.{} a {:.0}m", p.name, tag, class_name(p.class_id), p.level, dist))
                                })
                                .collect();
                            list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                            let body: Vec<String> = list.into_iter().take(8).map(|(_, s)| s).collect();
                            reply(format!("{} joueurs : {}", nearby_players.len(), body.join(", ")))
                        }
                    }
                    "mobs" | "mob" => {
                        let body = match world.nearest_npc() {
                            Some(npc) => format!("{} npc, plus proche template {} a {:.0} u", world.npc_count(), npc.template_id, npc.location.distance_to(world.player.location)),
                            None => format!("{} npc", world.npc_count()),
                        };
                        reply(body)
                    }
                    "hp" | "vie" => reply(format!("vie {}/{} mana {}/{}", world.player.hp, world.player.max_hp, world.player.mp, world.player.max_mp)),
                    "zone" => reply(format!("zone {}", world.player.zone)),
                    "etat" | "state" | "status" => reply(format!(
                        "zone {} vie {}/{} pos {:?}", world.player.zone, world.player.hp, world.player.max_hp, bot_pos
                    )),
                    "uptime" => reply(format!("en ligne depuis {} (reco {})", human_duration(t0.elapsed().as_secs()), reconnect_attempt)),
                    "find" | "cherche" => {
                        let who = argument.trim();
                        if who.is_empty() {
                            reply("usage: find <pseudo>".to_string())
                        } else {
                            match user_ids.get(who).copied().and_then(|id| heal.ally_loc(id)) {
                                Some(loc) => {
                                    let body = match bot_pos {
                                        Some(pos) => {
                                            let d = ((loc[0] - pos[0]).powi(2) + (loc[1] - pos[1]).powi(2)).sqrt();
                                            format!("{who} est a {d:.0} u de moi")
                                        }
                                        None => format!("{who} est visible"),
                                    };
                                    reply(body)
                                }
                                None => reply(format!("je ne vois pas {who} autour")),
                            }
                        }
                    }
                    "guardian" | "legion" | "ginfo" => reply(auto.guardian_summary()),
                    "autostatus" | "ainfo" => reply(auto.status()),
                    "gm" | "gms" => {
                        if gm_alerted.is_empty() {
                            reply("aucun GM detecte pour l'instant".to_string())
                        } else {
                            let now = Instant::now();
                            let mut list: Vec<String> = gm_alerted
                                .iter()
                                .map(|(name, seen)| format!("{name} (il y a {}s)", now.duration_since(*seen).as_secs()))
                                .collect();
                            list.sort_unstable();
                            reply(format!("GM vus: {}", list.join(", ")))
                        }
                    }
                    "online" | "co" => {
                        let mut names: Vec<&str> = guild_members
                            .values()
                            .filter(|member| member.online)
                            .map(|member| member.name.as_str())
                            .collect();
                        names.sort_unstable_by_key(|name| name.to_lowercase());
                        if names.is_empty() {
                            reply("personne en ligne (ou liste de guilde pas encore recue)".to_string())
                        } else {
                            reply(format!("{} en ligne: {}", names.len(), names.join(", ")))
                        }
                    }
                    "whois" | "qui-est" => {
                        let who = argument.trim().to_lowercase();
                        if who.is_empty() {
                            reply("usage: whois <pseudo>".to_string())
                        } else {
                            match guild_members.get(&who) {
                                Some(member) => reply(format!(
                                    "{} - {} niv {} - rang {} - {}{}{}",
                                    member.name,
                                    class_name(member.class),
                                    member.level,
                                    member.rank,
                                    if member.online { format!("en ligne (zone {})", member.zone) } else { "hors ligne".to_string() },
                                    if member.contribution > 0 { format!(" - contrib {}", member.contribution) } else { String::new() },
                                    if member.note.is_empty() { String::new() } else { format!(" - note: {}", member.note) },
                                )),
                                None => reply(format!("{} n'est pas dans la guilde (ou liste pas encore recue)", argument.trim())),
                            }
                        }
                    }
                    "auto" => {
                        let sub = argument.split_whitespace().next().unwrap_or("").to_lowercase();
                        match sub.as_str() {
                            "" | "status" | "info" => reply(auto.status()),
                            "gi" | "guardian" => reply(auto.guardian_summary()),
                            "gc" | "claim" => {
                                let now = Instant::now();
                                let out = auto.claim_now();
                                for (delay_ms, opcode_name, packet_object) in out.sends {
                                    pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                                }
                                reply("claim guardian lance".to_string())
                            }
                            other => match auto.toggle(other) {
                                Some(status) => reply(status),
                                None => reply("usage: auto vg|gq|gl|dc|gi|gc|status".to_string()),
                            },
                        }
                    }
                    "rtype" => {
                        if let Ok(value) = argument.trim().parse::<u64>() {
                            reply_response = value;
                            reply(format!("response d'accept = {reply_response}"))
                        } else {
                            reply(format!("response actuel = {reply_response} (usage: rtype N)"))
                        }
                    }
                    "itype" => {
                        if let Ok(value) = argument.trim().parse::<i64>() {
                            invite_type = value;
                            reply(format!("type d'invite = {invite_type} (unk2={invite_unk2})"))
                        } else {
                            reply(format!("type actuel = {invite_type} (usage: itype N)"))
                        }
                    }
                    "iunk2" => {
                        if let Ok(value) = argument.trim().parse::<i64>() {
                            invite_unk2 = value;
                            reply(format!("unk2 d'invite = {invite_unk2} (type={invite_type})"))
                        } else {
                            reply(format!("unk2 actuel = {invite_unk2} (usage: iunk2 N)"))
                        }
                    }
                    "heal" | "soin" => { heal.cfg.enabled = !heal.cfg.enabled; reply(heal.status()) }
                    "dps" | "combat" | "attack" | "tape" => { heal.cfg.attack_enabled = !heal.cfg.attack_enabled; reply(heal.status()) }
                    "testrota" | "testburst" => {
                        heal.set_self(bot_pos, player_w);
                        let now = Instant::now();
                        let seq = heal.force_opener();
                        let count = seq.len();
                        for (delay_ms, opcode_name, packet_object) in seq {
                            pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                        }
                        reply(format!("ouverture lancee ({count} paquets)"))
                    }
                    "rotation" | "boss-rota" => { heal.cfg.rotation_enabled = !heal.cfg.rotation_enabled; reply(heal.status()) }
                    "rotainfo" | "rota?" | "rotastatus" => reply(heal.rotation_report()),
                    "skills" | "skilllist" | "listskills" => reply(heal.skill_report()),
                    "glyphs" | "glyphes" | "crests" => reply(heal.glyph_report()),
                    "purify" | "cleanse" | "purge" => {
                        heal.set_self(bot_pos, player_w);
                        let now = Instant::now();
                        for (delay_ms, opcode_name, packet_object) in heal.force_purify() {
                            pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                        }
                        reply("purifying circle".to_string())
                    }
                    "infuse" | "mana" => {
                        heal.set_self(bot_pos, player_w);
                        let now = Instant::now();
                        for (delay_ms, opcode_name, packet_object) in heal.force_infuse() {
                            pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                        }
                        reply("mana infusion".to_string())
                    }
                    "trace" | "rotatrace" => { heal.trace_enabled = !heal.trace_enabled; reply(format!("trace rotation {} (detail en console)", if heal.trace_enabled { "on" } else { "off" })) }
                    "tracedump" | "rotalog" | "lastcast" => reply(heal.trace_tail()),
                    "setskill" => {
                        let mut parts = argument.split_whitespace();
                        match (parts.next(), parts.next().and_then(|v| v.parse::<u32>().ok())) {
                            (Some(name), Some(id)) => reply(heal.set_skill(name, id)),
                            _ => reply("usage: setskill <nom> <id>".to_string()),
                        }
                    }
                    "dodge" | "esquive" => { heal.dodge_enabled = !heal.dodge_enabled; reply(heal.status()) }
                    "healinfo" => reply(heal.status()),
                    "healset" => {
                        let ids: Vec<u32> = argument.split_whitespace().filter_map(|part| part.parse().ok()).collect();
                        if ids.len() >= 2 {
                            heal.cfg.focus_heal = ids[0];
                            heal.cfg.healing_immersion = ids[1];
                            reply(heal.status())
                        } else {
                            reply("usage: healset <focus_id> <immersion_id>".to_string())
                        }
                    }
                    "inactifs" | "inactives" | "menage" => {
                        let days: i64 = argument.trim().parse().unwrap_or(7);
                        let threshold = days.max(0) * 86_400;
                        let mut rows: Vec<(i64, String)> = guild_members
                            .values()
                            .filter(|m| !m.online)
                            .filter_map(|m| match state.seen_ago(&m.name) {
                                None => Some((i64::MAX, format!("{} ({} niv.{}) jamais vu", m.name, class_name(m.class), m.level))),
                                Some(secs) if secs >= threshold => Some((secs, format!("{} ({} niv.{}) vu {}", m.name, class_name(m.class), m.level, services::ago(secs)))),
                                Some(_) => None,
                            })
                            .collect();
                        if rows.is_empty() {
                            reply(format!("aucun membre inactif depuis plus de {days}j (parmi les hors-ligne connus)"))
                        } else {
                            rows.sort_by_key(|(secs, _)| std::cmp::Reverse(*secs));
                            let total = rows.len();
                            let shown: Vec<String> = rows.into_iter().take(15).map(|(_, s)| s).collect();
                            let suffix = if total > shown.len() { format!(" (+{} autres)", total - shown.len()) } else { String::new() };
                            reply(format!("{total} inactifs >{days}j : {}{suffix}", shown.join(" | ")))
                        }
                    }
                    "boss" | "boss-list" => {
                        if spawned_bosses.is_empty() {
                            reply("aucun boss/marchand suivi en vue".to_string())
                        } else {
                            let mut list: Vec<(u64, String)> = spawned_bosses
                                .values()
                                .map(|b| {
                                    let secs = b.since.elapsed().as_secs();
                                    (secs, format!("{} ({}) - {} depuis {}", b.name, b.kind, b.region, human_duration(secs)))
                                })
                                .collect();
                            list.sort_by_key(|(s, _)| *s);
                            let body: Vec<String> = list.into_iter().map(|(_, s)| s).collect();
                            reply(format!("{} en vue : {}", body.len(), body.join(" | ")))
                        }
                    }
                    "zonetime" | "zones-temps" => {
                        let snap = zone_time.snapshot();
                        if snap.is_empty() {
                            reply("pas encore de temps par zone".to_string())
                        } else {
                            let body: Vec<String> = snap
                                .into_iter()
                                .take(6)
                                .map(|(zone, secs)| format!("{} : {}", zone_names.label(zone), human_duration(secs)))
                                .collect();
                            reply(format!("temps par zone : {}", body.join(" | ")))
                        }
                    }
                    "sla" | "uptime%" | "dispo" => {
                        let elapsed = t0.elapsed();
                        let up = elapsed.saturating_sub(downtime_total);
                        let pct = if elapsed.as_secs_f64() > 0.0 {
                            up.as_secs_f64() / elapsed.as_secs_f64() * 100.0
                        } else {
                            100.0
                        };
                        reply(format!(
                            "dispo {pct:.2}% sur {} (hors ligne {}, {} reconnexions)",
                            human_duration(elapsed.as_secs()),
                            human_duration(downtime_total.as_secs()),
                            reconnect_attempt
                        ))
                    }
                    "parcel" | "colis" => {
                        if !cli.auto_parcel {
                            reply("auto-collecte des colis desactivee (--auto-parcel)".to_string())
                        } else if opcodes.code("C_LIST_PARCEL").is_none() {
                            reply("opcode colis indisponible sur ce serveur".to_string())
                        } else {
                            last_parcel = Instant::now() - Duration::from_secs(3600);
                            reply("verification de la boite aux colis lancee".to_string())
                        }
                    }
                    "help" | "aide" => {
                        let body = if access == Access::Admin {
                            "ADMIN: come follow goto stop enter auto gsay say heal dps testrota rotainfo skills glyphs trace purify infuse dodge watch timer roster grep parcel inactifs bye | tous: pos hp zone who near mobs ping etat uptime sla find guild ou whois seen mail gtime gm online guardian autostatus boss zonetime invite lead leave".to_string()
                        } else {
                            "guilde (@): pos hp zone who near mobs ping etat uptime sla find guild ou whois seen mail gtime gm online guardian autostatus boss zonetime invite lead leave | fun: de pile roll 8ball pfc choisis slot fortune".to_string()
                        };
                        reply(body)
                    }
                    "bye" | "logout" | "quit" => {
                        if let Some((name, object)) = reply("a plus !".to_string()) {
                            let _ = reactive_send(&mut stream, &mut enc, &registry, &opcodes, name, &object, t0);
                        }
                        logln!("[{:7.3}] logout demande", t0.elapsed().as_secs_f64());
                        user_quit = true;
                        break 'session;
                    }
                    _ => None,
                }
            };
            if let Some((name, object)) = reply_packet {
                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, name, &object, t0)? {
                    break 'session;
                }
            }
        }
        if last_heal_tick.elapsed() >= heal_tick_interval {
            last_heal_tick = Instant::now();
            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
            let now = Instant::now();
            for (delay_ms, opcode_name, packet_object) in heal.tick(now) {
                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
            }
        }
        for line in heal.drain_trace() {
            logln!("[{:7.3}] rota {line}", t0.elapsed().as_secs_f64());
        }
        heal.set_self(bot_pos, player_w);
        let pocket_id = cli
            .pocket
            .as_deref()
            .and_then(|name| user_ids.get(&name.to_lowercase()).copied());
        if let Some(id) = pocket_id {
            follow_id = Some(id);
            auto_follow = true;
            heal.set_priority(Some(id));
        } else if cli.pocket.is_some() {
            heal.set_priority(None);
        }
        if auto_follow && follow_id.is_none() {
            follow_id = heal.first_target();
        }
        let move_now = Instant::now();
        if let Some(dodge) = heal.take_dodge() {
            move_target = Some(dodge);
            logln!("[{:7.3}] ESQUIVE -> pas de cote", t0.elapsed().as_secs_f64());
        } else if heal.needs_heal(move_now) {
            move_target = None;
        } else if let Some(id) = follow_id {
            if let Some(loc) = heal.ally_loc(id).or_else(|| user_locs.get(&id).copied()) {
                move_target = Some(loc);
            }
        } else if heal.channeling() {
            if combat_driving { move_target = None; combat_driving = false; }
        } else if heal.cfg.attack_enabled || heal.cfg.rotation_enabled {
            match heal.combat_target().or_else(|| heal.farm_roam_target()) {
                Some(spot) => { move_target = Some(spot); combat_driving = true; }
                None if combat_driving => { move_target = None; combat_driving = false; }
                None => {}
            }
        }
        if let (Some(target), Some(pos)) = (move_target, bot_pos) {
            if last_step.elapsed() >= move_step_interval {
                let dt = last_step.elapsed().as_secs_f32();
                last_step = Instant::now();
                let move_speed = heal.run_speed().max(1.0);
                let dx = target[0] - pos[0];
                let dy = target[1] - pos[1];
                let distance = (dx * dx + dy * dy).sqrt();
                let arrive = if pocket_id.is_some() { 90.0 } else if follow_id.is_some() { 200.0 } else { 60.0 };
                let stamp = (t0.elapsed().as_millis() as u64) & 0xffff_ffff;
                if distance <= arrive {
                    if follow_id.is_none() {
                        let stopped = [target[0], target[1], pos[2]];
                        bot_pos = Some(stopped);
                        player_loc = Some(tera_protocol::Value::Vec3(stopped));
                        let stop = tera_protocol::Object::new()
                            .with("loc", tera_protocol::Value::Vec3(stopped))
                            .with("w", tera_protocol::Value::Int(player_w))
                            .with("lookDirection", tera_protocol::Value::Int(0))
                            .with("dest", tera_protocol::Value::Vec3(stopped))
                            .with("type", tera_protocol::Value::Int(7))
                            .with("jumpDistance", tera_protocol::Value::Int(0))
                            .with("inShuttle", tera_protocol::Value::Bool(false))
                            .with("time", tera_protocol::Value::Uint(stamp));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &stop, t0)? {
                            break 'session;
                        }
                        move_target = None;
                        last_keepalive = Instant::now();
                    }
                } else {
                    let step = (move_speed * dt).min(distance);
                    let ux = dx / distance;
                    let uy = dy / distance;
                    let fraction = step / distance;
                    let next_z = pos[2] + (target[2] - pos[2]) * fraction;
                    let next = [pos[0] + ux * step, pos[1] + uy * step, next_z];
                    let lookahead = (distance - step).min(move_dest_lookahead);
                    let dest_point = [next[0] + ux * lookahead, next[1] + uy * lookahead, target[2]];
                    bot_pos = Some(next);
                    player_loc = Some(tera_protocol::Value::Vec3(next));
                    player_w = ((dy.atan2(dx) / (2.0 * std::f32::consts::PI)) * 65536.0) as i32 as i16 as i64;
                    let moving = tera_protocol::Object::new()
                        .with("loc", tera_protocol::Value::Vec3(next))
                        .with("w", tera_protocol::Value::Int(player_w))
                        .with("lookDirection", tera_protocol::Value::Int(0))
                        .with("dest", tera_protocol::Value::Vec3(dest_point))
                        .with("type", tera_protocol::Value::Int(0))
                        .with("jumpDistance", tera_protocol::Value::Int(0))
                        .with("inShuttle", tera_protocol::Value::Bool(false))
                        .with("time", tera_protocol::Value::Uint(stamp));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &moving, t0)? {
                        break 'session;
                    }
                    last_keepalive = Instant::now();
                }
            }
        }
        let read = match stream.read(&mut buf) {
            Ok(0) => { logln!("[{:7.3}] serveur a ferme", t0.elapsed().as_secs_f64()); break }
            Ok(n) => { last_recv = Instant::now(); n }
            Err(e)
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
            {
                let input_rx = ui.as_ref().map(|h| &h.input).or(stdin_rx.as_ref());
                if let Some(rx) = input_rx {
                    while let Ok(text) = rx.try_recv() {
                        if text == tui::QUIT_SENTINEL {
                            logln!("[{:7.3}] fermeture demandee", t0.elapsed().as_secs_f64());
                            user_quit = true;
                            break 'session;
                        }
                        if let Some(rest) = text.strip_prefix('@').or_else(|| text.strip_prefix('/')) {
                            pending_command.push_back((Access::Admin, ReplyTarget::Local, rest.trim().to_string()));
                            continue;
                        }
                        let channel = if cli.tui { 2 } else { cli.chat_channel };
                        let message = tera_protocol::Object::new()
                            .with("channel", tera_protocol::Value::Uint(u64::from(channel)))
                            .with("message", tera_protocol::Value::Str(text));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &message, t0)? {
                            break 'session;
                        }
                    }
                }
                if last_recv.elapsed() > inactivity_limit {
                    logln!("[{:7.3}] serveur muet {}s, on coupe", t0.elapsed().as_secs_f64(), inactivity_limit.as_secs());
                    break 'session;
                }
                if in_world && last_keepalive.elapsed() > keepalive_interval {
                    if let Some(location) = &player_loc {
                        last_keepalive = Instant::now();
                        let stamp = (t0.elapsed().as_millis() as u64) & 0xffff_ffff;
                        let packet = tera_protocol::Object::new()
                            .with("loc", location.clone())
                            .with("w", tera_protocol::Value::Int(player_w))
                            .with("lookDirection", tera_protocol::Value::Int(player_w))
                            .with("dest", location.clone())
                            .with("type", tera_protocol::Value::Int(7))
                            .with("jumpDistance", tera_protocol::Value::Int(0))
                            .with("inShuttle", tera_protocol::Value::Bool(false))
                            .with("time", tera_protocol::Value::Uint(stamp));
                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PLAYER_LOCATION", &packet, t0)? {
                            break 'session;
                        }
                        logln!("[{:7.3}] keep-alive (C_PLAYER_LOCATION)", t0.elapsed().as_secs_f64());
                    }
                }
                continue;
            }
            Err(e) => { logln!("[{:7.3}] lecture: {e}", t0.elapsed().as_secs_f64()); break }
        };
        dec.apply(&mut buf[..read]);
        packets.push(&buf[..read]);
        while let Some(packet) = packets.take_packet() {
            let name = opcodes.name(packet.opcode).unwrap_or("INCONNU").to_string();
            show("<-", t0, &name, packet.opcode, &packet.body, &registry, &packet.encode());
            if cli.auto_gen_defs && registry.get(&name).is_none() {
                let gen_name = if name == "INCONNU" {
                    format!("UNK_{}", packet.opcode)
                } else {
                    name.clone()
                };
                if let Some(path) = defgen.observe(&gen_name, &packet.encode()) {
                    logln!("[{:7.3}] def scaffold generee : {path}", t0.elapsed().as_secs_f64());
                    if let Some(handle) = &ui {
                        handle.act(format!("def auto: {gen_name}"));
                    }
                }
            }
            if cli.salute && name == "S_ACTION_STAGE" && world.player.game_id != 0 {
                if let Some(obj) = registry.get(&name).and_then(|def| tera_protocol::value::read(def, &packet.encode()).ok()) {
                    let actor = obj.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                    let target = obj.get("target").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                    let skill = obj.get("skill").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                    if greet::is_greet(skill) && target == world.player.game_id && actor != world.player.game_id {
                        let fresh = salute_cooldown
                            .get(&actor)
                            .map_or(true, |at| at.elapsed() >= Duration::from_secs(8));
                        if fresh {
                            salute_cooldown.insert(actor, Instant::now());
                            let loc = bot_pos.unwrap_or([world.player.location.x, world.player.location.y, world.player.location.z]);
                            let packet = match heal.ally_loc(actor).or_else(|| user_locs.get(&actor).copied()) {
                                Some(tloc) => greet::build_targeted(loc, greet::face(loc, tloc), actor, tloc),
                                None => greet::build_with(loc, player_w, false),
                            };
                            send_raw(&mut stream, &mut enc, &registry, "C_START_INSTANCE_SKILL", packet, t0)?;
                            logln!("[{:7.3}] salut : joueur (gid {actor}) a salue le bot -> salut rendu", t0.elapsed().as_secs_f64());
                            if let Some(h) = &ui {
                                h.act("salut rendu a un joueur".to_string());
                            }
                        }
                    }
                }
            }
            if let Some(b) = broker.as_mut() {
                let out = match name.as_str() {
                    "S_LOGIN" => Some(b.start()),
                    "S_TRADE_BROKER_WAITING_ITEM_LIST" => registry.get(&name).map(|def| b.on_listings(def, &packet.encode())),
                    "S_NPC_MENU_SELECT" => registry.get(&name).and_then(|def| tera_protocol::value::read(def, &packet.encode()).ok()).map(|obj| {
                        let ctype = obj.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                        let mut o = broker::BrokerOut::default();
                        o.messages.push(format!("S_NPC_MENU_SELECT type={ctype} (28=broker) -> le serveur ouvre un service"));
                        if ctype == 28 {
                            let restart = b.restart_scan();
                            o.packets = restart.packets;
                        }
                        o
                    }),
                    "S_DIALOG" if broker_open_tried && !broker_dialog_sent => registry.get(&name).and_then(|def| tera_protocol::value::read(def, &packet.encode()).ok()).map(|obj| {
                        let id = obj.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0) as u32;
                        broker_dialog_sent = true;
                        let mut o = broker::BrokerOut::default();
                        match (id, opcodes.code("C_DIALOG_EVENT"), opcodes.code("C_DIALOG")) {
                            (id, Some(ev), Some(sel)) if id != 0 => {
                                o.packets.push(broker::OutPacket { name: "C_DIALOG_EVENT", bytes: broker::build_dialog_event(ev, id, 0) });
                                o.packets.push(broker::OutPacket { name: "C_DIALOG", bytes: broker::build_dialog_select(sel, id, 1) });
                                o.packets.push(broker::OutPacket { name: "C_DIALOG_EVENT", bytes: broker::build_dialog_event(ev, id, 2) });
                                o.messages.push(format!("dialogue broker (id {id}) -> selection option broker, scan"));
                                o.packets.extend(b.restart_scan().packets);
                            }
                            _ => o.messages.push(format!("S_DIALOG id={id} mais opcodes dialogue absents")),
                        }
                        o
                    }),
                    "S_REQUEST_CONTRACT" => registry.get(&name).and_then(|def| tera_protocol::value::read(def, &packet.encode()).ok()).map(|obj| {
                        let ctype = obj.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                        let id = obj.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                        let mut o = b.restart_scan();
                        o.messages.push(format!("broker ouvert (contrat type={ctype} id={id}) -> scan lance"));
                        o
                    }),
                    "S_TRADE_BROKER_BUY_IT_NOW" => b.on_buy_result().map(|m| {
                        let mut o = broker::BrokerOut::default();
                        o.messages.push(m);
                        o
                    }),
                    "S_SYSTEM_MESSAGE" => b.on_system_message().map(|m| {
                        let mut o = broker::BrokerOut::default();
                        o.messages.push(m);
                        o
                    }),
                    _ => None,
                };
                if let Some(out) = out {
                    for p in out.packets {
                        send_raw(&mut stream, &mut enc, &registry, p.name, p.bytes, t0)?;
                    }
                    for m in out.messages {
                        logln!("[{:7.3}] broker: {m}", t0.elapsed().as_secs_f64());
                        if let Some(h) = &ui {
                            h.act(format!("broker: {m}"));
                        }
                    }
                }
            }
            let want_world = world_packets.contains(&name.as_str());
            let want_heal = heal_packets.contains(&name.as_str());
            let want_auto = auto_packets.contains(&name.as_str());
            if want_world || want_heal || want_auto {
                if let Some(definition) = registry.get(&name) {
                    if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                        if want_auto {
                            let out = auto.observe(&name, &object);
                            let now = Instant::now();
                            for (delay_ms, opcode_name, packet_object) in out.sends {
                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                            }
                            for line in out.logs {
                                logln!("[{:7.3}] auto: {line}", t0.elapsed().as_secs_f64());
                                if let Some(handle) = &ui { handle.act(line); }
                            }
                        }
                        if want_world {
                            world.apply(&name, &object);
                            if broker.is_some() && name == "S_SPAWN_NPC" {
                                let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                if game_id != 0 {
                                    let npc_name = object.get("npcName").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                                    let template = object.get("templateId").and_then(tera_protocol::Value::as_uint).unwrap_or(0) as u32;
                                    let loc = match object.get("loc") {
                                        Some(tera_protocol::Value::Vec3(v)) => *v,
                                        _ => [0.0; 3],
                                    };
                                    broker_npcs.insert(game_id, (npc_name, template, loc));
                                }
                            }
                            if broker.is_some() && name == "S_DESPAWN_NPC" {
                                let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                broker_npcs.remove(&game_id);
                            }
                            if name == "S_DESPAWN_USER" {
                                let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                nearby_players.remove(&game_id);
                                user_locs.remove(&game_id);
                            }
                            if name == "S_USER_LOCATION" {
                                if let Some(tera_protocol::Value::Vec3(loc)) = object.get("loc") {
                                    let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    if game_id != 0 {
                                        user_locs.insert(game_id, *loc);
                                    }
                                }
                            }
                            if name == "S_SPAWN_NPC" && !boss_db.is_empty() {
                                let hz = object.get("huntingZoneId").and_then(tera_protocol::Value::as_uint).unwrap_or(0) as u32;
                                let tid = object.get("templateId").and_then(tera_protocol::Value::as_uint).unwrap_or(0) as u32;
                                if let Some(boss) = boss_db.lookup(hz, tid) {
                                    let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    if let std::collections::hash_map::Entry::Vacant(slot) = spawned_bosses.entry(game_id) {
                                        let place = if boss.region.is_empty() { zone_names.label(world.player.zone as i64) } else { boss.region.clone() };
                                        let msg = format!("{} ({}) vient d'apparaitre - {}", boss.name, boss.kind, place);
                                        slot.insert(BossSpawn { name: boss.name.clone(), kind: boss.kind.clone(), region: place, since: Instant::now() });
                                        activity.log("boss", &msg);
                                        logln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
                                        if let Some(handle) = &ui { handle.log(tui::LogKind::Boss, msg.clone()); }
                                        if cli.boss_alert {
                                            if let Some(tx) = &discord { let _ = tx.send(format!(":crossed_swords: {msg}")); }
                                            let announce = tera_protocol::Object::new()
                                                .with("channel", tera_protocol::Value::Uint(2))
                                                .with("message", tera_protocol::Value::Str(format!("[boss] {msg}")));
                                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &announce, t0)? {
                                                break 'session;
                                            }
                                        }
                                    }
                                }
                            }
                            if name == "S_DESPAWN_NPC" {
                                let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                let despawn_type = object.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                if let Some(boss) = spawned_bosses.remove(&game_id) {
                                    if despawn_type == 5 {
                                        let lived = human_duration(boss.since.elapsed().as_secs());
                                        let msg = format!("{} ({}) a ete vaincu apres {}", boss.name, boss.kind, lived);
                                        activity.log("boss", &msg);
                                        logln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
                                        if let Some(handle) = &ui { handle.log(tui::LogKind::Boss, msg.clone()); }
                                        if cli.boss_alert {
                                            if let Some(tx) = &discord { let _ = tx.send(format!(":skull: {msg}")); }
                                            let announce = tera_protocol::Object::new()
                                                .with("channel", tera_protocol::Value::Uint(2))
                                                .with("message", tera_protocol::Value::Str(format!("[boss] {msg}")));
                                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &announce, t0)? {
                                                break 'session;
                                            }
                                        }
                                    }
                                }
                            }
                            if name == "S_SPAWN_USER" {
                                if let Some(user) = object.get("name").and_then(tera_protocol::Value::as_str) {
                                    let game_id = object.get("gameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    user_ids.insert(user.to_string(), game_id);
                                    if game_id != 0 {
                                        if let Some(tera_protocol::Value::Vec3(loc)) = object.get("loc") {
                                            user_locs.insert(game_id, *loc);
                                        }
                                    }
                                    let srv = object.get("serverId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    if srv != 0 {
                                        my_server_id = srv;
                                    }
                                    let gm = object.get("gm").and_then(tera_protocol::Value::as_uint).unwrap_or(0) != 0;
                                    let gm_invisible = object.get("gmInvisible").and_then(tera_protocol::Value::as_uint).unwrap_or(0) != 0;
                                    if !user.eq_ignore_ascii_case(&bot_name) {
                                        let template_id = object.get("templateId").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                        let level = object.get("level").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                        let guild = object.get("guildName").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                                        let location = match object.get("loc") {
                                            Some(tera_protocol::Value::Vec3(c)) => tera_world::Vec3 { x: c[0], y: c[1], z: c[2] },
                                            _ => tera_world::Vec3::default(),
                                        };
                                        let newly_seen = !nearby_players.contains_key(&game_id);
                                        nearby_players.insert(game_id, NearbyPlayer {
                                            name: user.to_string(),
                                            class_id: class_from_template(template_id),
                                            level,
                                            gm,
                                            gm_invisible,
                                            guild,
                                            location,
                                        });
                                        if newly_seen {
                                            state.mark_seen(user);
                                            for mail in state.take_mail(user) {
                                                let body = format!("[message de {}] {}", mail.from, mail.text);
                                                let notify = tera_protocol::Object::new()
                                                    .with("targetServer", tera_protocol::Value::Uint(my_server_id))
                                                    .with("target", tera_protocol::Value::Str(user.to_string()))
                                                    .with("message", tera_protocol::Value::Str(body));
                                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &notify, t0)? {
                                                    break 'session;
                                                }
                                            }
                                            if state.is_watched(user) {
                                                let alert = format!("watchlist : {user} est apparu pres du bot (zone {})", world.player.zone);
                                                if send_alert(&mut stream, &mut enc, &registry, &opcodes, t0, &ui, &discord, &admin_names, &online_admins, my_server_id, tui::LogKind::Watch, &alert)? {
                                                    break 'session;
                                                }
                                            }
                                        }
                                    }
                                    if (gm || gm_invisible) && !admins.contains(&user.to_lowercase()) {
                                        let now = Instant::now();
                                        let fresh = gm_alerted
                                            .get(user)
                                            .map(|last| now.duration_since(*last) > Duration::from_secs(120))
                                            .unwrap_or(true);
                                        if fresh {
                                            gm_alerted.insert(user.to_string(), now);
                                            let alert = format!(
                                                "ALERTE GM : {user} a proximite du bot{} (zone {})",
                                                if gm_invisible { " [INVISIBLE]" } else { "" },
                                                world.player.zone
                                            );
                                            if send_alert(&mut stream, &mut enc, &registry, &opcodes, t0, &ui, &discord, &admin_names, &online_admins, my_server_id, tui::LogKind::Gm, &alert)? {
                                                break 'session;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if want_heal {
                            heal.set_self(bot_pos, player_w);
                            heal.set_moving(move_target.is_some() || follow_id.is_some());
                            let now = Instant::now();
                            for (delay_ms, opcode_name, packet_object) in heal.observe(&name, &object, now) {
                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                            }
                        }
                        if name == "S_PARTY_MEMBER_LIST" && !cli.commander.is_empty() {
                            if let Some(tera_protocol::Value::Array(members)) = object.get("members") {
                                for member in members {
                                    let member_name = member.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("");
                                    if cli.commander.eq_ignore_ascii_case(member_name) {
                                        let server_id = member.get("serverId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                        let player_id = member.get("playerId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                        commander_party = Some((server_id, player_id));
                                    }
                                }
                            }
                        }
                        if name == "S_DIALOG" {
                            if let Some(tera_protocol::Value::Array(buttons)) = object.get("buttons") {
                                logln!("[{:7.3}] DIALOGUE ({} boutons) :", t0.elapsed().as_secs_f64(), buttons.len());
                                for (i, button) in buttons.iter().enumerate() {
                                    let text = button.get("text").and_then(tera_protocol::Value::as_str).unwrap_or("");
                                    logln!("           {} -> {text:?}", i + 1);
                                }
                            }
                        }
                    }
                }
            }
            if matches!(
                name.as_str(),
                "S_CHAT" | "S_PRIVATE_CHAT" | "S_DUNGEON_EVENT_MESSAGE" | "S_SYSTEM_MESSAGE"
            ) {
                if cli.chat {
                    print_chat(&registry, &name, &packet);
                }
                if let Some((channel, author, clean)) = parse_chat(&registry, &name, &packet)
                    .filter(|(_, _, text)| !is_system_template(text))
                {
                    if let Some(handle) = &ui {
                        handle.chat(channel as u32, &author, &clean);
                    }
                    chat_log_line(channel, &author, &clean);
                    if let Some(sink) = &discord {
                        if channel == 2 {
                            let _ = sink.send(format!("**{author}**: {clean}"));
                        }
                    }
                    if name == "S_CHAT" && channel == 2 {
                        if let Some(rest) = clean.strip_prefix('@') {
                            let rest = rest.trim().to_string();
                            if !rest.is_empty() && !author.eq_ignore_ascii_case(&bot_name) {
                                let access = if admins.contains(&author.to_lowercase()) {
                                    Access::Admin
                                } else {
                                    Access::Guild
                                };
                                pending_command.push_back((access, ReplyTarget::Guild { author: author.clone() }, rest));
                            }
                        }
                    }
                }
            }
            match name.as_str() {
                "S_PING" => {
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_PONG", &empty, t0)? {
                        break 'session;
                    }
                }
                "S_LIST_PARCEL_EX" if cli.auto_parcel => {
                    if let Some(definition) = registry.get("S_LIST_PARCEL_EX") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            if let Some(tera_protocol::Value::Array(messages)) = object.get("messages") {
                                for parcel in messages {
                                    let id = parcel.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    let cod = parcel.get("codMoney").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    let status = parcel.get("status").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    if id == 0 || cod > 0 || status == 2 {
                                        continue;
                                    }
                                    let sender = parcel.get("sender").and_then(tera_protocol::Value::as_str).unwrap_or("?").to_string();
                                    let subject = parcel.get("subject").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                                    let money = parcel.get("attachedMoney").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                                    let items: Vec<String> = match parcel.get("items") {
                                        Some(tera_protocol::Value::Array(list)) => list
                                            .iter()
                                            .map(|it| {
                                                let item_id = it.get("id").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                                let amount = it.get("amount").and_then(tera_protocol::Value::as_int).unwrap_or(1);
                                                format!("item {item_id} x{amount}")
                                            })
                                            .collect(),
                                        _ => Vec::new(),
                                    };
                                    let mut parts = Vec::new();
                                    if money > 0 { parts.push(format!("{} or", money / 10_000)); }
                                    parts.extend(items);
                                    let content = if parts.is_empty() { "message".to_string() } else { parts.join(", ") };
                                    let recv = tera_protocol::Object::new()
                                        .with("unk", tera_protocol::Value::Uint(0))
                                        .with("id", tera_protocol::Value::Uint(id));
                                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_RECV_PARCEL", &recv, t0)? {
                                        break 'session;
                                    }
                                    let msg = format!("colis recupere de {sender} : {content}{}", if subject.is_empty() { String::new() } else { format!(" ({subject})") });
                                    activity.log("colis", &msg);
                                    logln!("[{:7.3}] {msg}", t0.elapsed().as_secs_f64());
                                    if let Some(handle) = &ui { handle.log(tui::LogKind::Parcel, msg.clone()); }
                                    if let Some(tx) = &discord { let _ = tx.send(format!(":package: {msg}")); }
                                }
                            }
                        }
                    }
                }
                "S_LOGIN_ARBITER" | "S_LOGIN_ACCOUNT_INFO" if !asked_list => {
                    asked_list = true;
                    let range = tera_protocol::Object::new()
                        .with("range", tera_protocol::Value::Uint(2000));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_SET_VISIBLE_RANGE", &range, t0)? {
                        break 'session;
                    }
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_GET_USER_LIST", &empty, t0)? {
                        break 'session;
                    }
                }
                "S_GET_USER_LIST" if !sent_hw => {
                    sent_hw = true;
                    let u = |v: u64| tera_protocol::Value::Uint(v);
                    let hw = tera_protocol::Object::new()
                        .with("systemMemory", u(16383)).with("videoMemory", u(0))
                        .with("resWidth", u(1920)).with("resHeight", u(1080))
                        .with("isFullScreen", tera_protocol::Value::Bool(false))
                        .with("resScreenWidth", u(1920)).with("resScreenHeight", u(1080))
                        .with("numDisplays", u(1))
                        .with("resVirtualWidth", u(1920)).with("resVirtualHeight", u(1080))
                        .with("physicalCores", u(10)).with("logicalCores", u(10))
                        .with("os", tera_protocol::Value::Str("Windows 10".into()))
                        .with("cpu", tera_protocol::Value::Str("VirtualApple @ 2.50GHz".into()))
                        .with("gpu", tera_protocol::Value::Str(String::new()));
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_HARDWARE_INFO", &hw, t0)? {
                        break 'session;
                    }

                    if !selected {
                        let chosen = if cli.pick_character && cli.character.is_none() && !did_prompt {
                            did_prompt = true;
                            let list = character_list(&registry, &packet);
                            let picked = prompt_character(&list);
                            if let Some((_, ref name)) = picked {
                                cli.character = Some(name.clone());
                            }
                            if defer_tui && ui.is_none() {
                                let (handle, thread) = start_tui(tui::tab_index(&cli.tab));
                                ui = Some(handle);
                                tui_thread = Some(thread);
                            }
                            picked
                        } else {
                            pick_character(&registry, &packet, cli.character.as_deref())
                        };
                        if let Some(chosen) = chosen {
                            selected = true;
                            bot_name = chosen.1.clone();
                            logln!("[{:7.3}] selection du perso \"{}\" (id {})", t0.elapsed().as_secs_f64(), chosen.1, chosen.0);
                            let select = tera_protocol::Object::new()
                                .with("id", tera_protocol::Value::Int(chosen.0 as i64))
                                .with("unk", tera_protocol::Value::Uint(0));
                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_SELECT_USER", &select, t0)? {
                                break 'session;
                            }
                        } else {
                            logln!("[{:7.3}] aucun perso a selectionner", t0.elapsed().as_secs_f64());
                        }
                    }
                }
                "S_PLAYER_CHANGE_EXP" => {
                    if let Some(def) = registry.get(&name) {
                        if let Ok(object) = tera_protocol::value::read(def, &packet.encode()) {
                            let gained = object.get("gainedXp").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let level_xp = object.get("levelXp").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let total_level_xp = object.get("totalLevelXp").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let monster = object.get("monsterGameId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            session_stats.on_exp(gained, level_xp, total_level_xp, monster);
                        }
                    }
                }
                "S_SYSTEM_MESSAGE_LOOT_ITEM" => {
                    if let Some(def) = registry.get(&name) {
                        if let Ok(object) = tera_protocol::value::read(def, &packet.encode()) {
                            let amount = object.get("amount").and_then(tera_protocol::Value::as_uint).unwrap_or(1);
                            session_stats.on_loot(amount);
                        }
                    }
                }
                "S_LOAD_TOPO" => {
                    nearby_players.clear();
                    spawned_bosses.clear();
                    let zone = world.player.zone as i64;
                    zone_time.enter(zone);
                    activity.log("zone", &zone_names.label(zone));
                    let empty = tera_protocol::Object::new();
                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LOAD_TOPO_FIN", &empty, t0)? {
                        break 'session;
                    }
                    logln!("[{:7.3}] >>> ENTRE DANS LE MONDE <<<", t0.elapsed().as_secs_f64());
                    let now = Instant::now();
                    for (delay_ms, opcode_name, packet_object) in auto.set_zone(world.player.zone.max(0) as u32).sends {
                        pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                    }
                }
                "S_SPAWN_ME" => {
                    if let Some(definition) = registry.get("S_SPAWN_ME") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            player_loc = object.get("loc").cloned();
                            if let Some(tera_protocol::Value::Vec3(a)) = &player_loc {
                                bot_pos = Some(*a);
                            }
                            player_w = object.get("w").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                            in_world = true;
                            last_keepalive = Instant::now();
                            let now = Instant::now();
                            for (delay_ms, opcode_name, packet_object) in auto.on_enter_world().sends {
                                pending_sends.push((now + Duration::from_millis(delay_ms), opcode_name, packet_object));
                            }
                            let leave = tera_protocol::Object::new();
                            let _ = reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_LEAVE_PARTY", &leave, t0);
                            logln!(
                                "[{:7.3}] ETAT nom={} gameId={} zone={} vie={}/{} -> C_LEAVE_PARTY defensif",
                                t0.elapsed().as_secs_f64(), bot_name, world.player.game_id,
                                world.player.zone, world.player.hp, world.player.max_hp
                            );
                            let zone = world.player.zone;
                            if cli.watchdog_zone && watchdog_zone_prev != -1 && zone != watchdog_zone_prev {
                                let ordered = last_enter.map(|t| t.elapsed() < Duration::from_secs(20)).unwrap_or(false);
                                if !ordered {
                                    let alert = format!("ALERTE : le bot a change de zone sans ordre ({watchdog_zone_prev} -> {zone}) ; teleporte/kick ?");
                                    if send_alert(&mut stream, &mut enc, &registry, &opcodes, t0, &ui, &discord, &admin_names, &online_admins, my_server_id, tui::LogKind::Zone, &alert)? {
                                        break 'session;
                                    }
                                }
                            }
                            watchdog_zone_prev = zone;
                        }
                    }
                }
                "S_BEGIN_THROUGH_ARBITER_CONTRACT" => {
                    if let Some(definition) = registry.get("S_BEGIN_THROUGH_ARBITER_CONTRACT") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let from = object.get("sender").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            let contract_type = object.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let contract_id = object.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let me_name = object
                                .get("recipient")
                                .and_then(tera_protocol::Value::as_str)
                                .map(str::to_string)
                                .filter(|name| !name.is_empty())
                                .unwrap_or_else(|| bot_name.clone());
                            let trusted = cli.commander.is_empty() || cli.commander.eq_ignore_ascii_case(&from);
                            if trusted {
                                let reply = tera_protocol::Object::new()
                                    .with("type", tera_protocol::Value::Uint(contract_type))
                                    .with("id", tera_protocol::Value::Uint(contract_id))
                                    .with("response", tera_protocol::Value::Uint(reply_response))
                                    .with("recipient", tera_protocol::Value::Str(me_name));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REPLY_THROUGH_ARBITER_CONTRACT", &reply, t0)? {
                                    break 'session;
                                }
                                logln!("[{:7.3}] contrat type {contract_type} de {from} -> accepte auto", t0.elapsed().as_secs_f64());
                            }
                        }
                    }
                }
                "S_REQUEST_CONTRACT" => {
                    if let Some(definition) = registry.get("S_REQUEST_CONTRACT") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let contract_type = object.get("type").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let contract_id = object.get("id").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let sender_name = object.get("senderName").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            let is_party = contract_type as i64 == invite_type;
                            if cli.refuse_trade_duel && !is_party {
                                logln!(
                                    "[{:7.3}] S_REQUEST_CONTRACT type={contract_type} de {sender_name:?} -> C_REJECT_CONTRACT (trade/duel refuse)",
                                    t0.elapsed().as_secs_f64()
                                );
                                let reject = tera_protocol::Object::new()
                                    .with("type", tera_protocol::Value::Int(contract_type as i64))
                                    .with("id", tera_protocol::Value::Int(contract_id as i64));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_REJECT_CONTRACT", &reject, t0)? {
                                    break 'session;
                                }
                            } else {
                                logln!(
                                    "[{:7.3}] S_REQUEST_CONTRACT type={contract_type} id={contract_id} sender={sender_name:?} -> C_ACCEPT_CONTRACT",
                                    t0.elapsed().as_secs_f64()
                                );
                                let accept = tera_protocol::Object::new()
                                    .with("type", tera_protocol::Value::Int(contract_type as i64))
                                    .with("id", tera_protocol::Value::Int(contract_id as i64));
                                if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_ACCEPT_CONTRACT", &accept, t0)? {
                                    break 'session;
                                }
                            }
                        }
                    }
                }
                "S_WHISPER" => {
                    if let Some(definition) = registry.get("S_WHISPER") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let sender = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            let server = object.get("senderServerId").and_then(tera_protocol::Value::as_uint).unwrap_or(0);
                            let text = strip_markup(object.get("message").and_then(tera_protocol::Value::as_str).unwrap_or(""));
                            if !sender.is_empty() && !sender.eq_ignore_ascii_case(&bot_name) {
                                if let Some(handle) = &ui {
                                    handle.chat(11, &sender, &text);
                                }
                                chat_log_line(11, &sender, &text);
                                let is_gm = object.get("gm").and_then(tera_protocol::Value::as_uint).unwrap_or(0) != 0;
                                if is_gm {
                                    logln!("[{:7.3}] ALERTE GM : whisper de {sender}: {text}", t0.elapsed().as_secs_f64());
                                    if let Some(handle) = &ui { handle.act(format!("ALERTE GM whisper {sender}: {text}")); }
                                }
                                if let Some(sink) = &discord {
                                    let tag = if is_gm { "@here [GM] " } else { "[DM] " };
                                    let _ = sink.send(format!("{tag}**{sender}**: {text}"));
                                }
                                if admins.contains(&sender.to_lowercase()) {
                                    logln!("[{:7.3}] << {sender}: {text}", t0.elapsed().as_secs_f64());
                                    pending_command.push_back((Access::Admin, ReplyTarget::Whisper { server, name: sender.clone() }, text));
                                } else {
                                    let key = sender.to_lowercase();
                                    let faq_answer = faq.answer(&text).filter(|a| !a.is_empty());
                                    if let Some(answer_text) = faq_answer {
                                        let fresh = faq_replied
                                            .get(&key)
                                            .map(|last| last.elapsed() > Duration::from_secs(15))
                                            .unwrap_or(true);
                                        if fresh {
                                            faq_replied.insert(key.clone(), Instant::now());
                                            logln!("[{:7.3}] FAQ -> {sender}: {answer_text}", t0.elapsed().as_secs_f64());
                                            let answer = tera_protocol::Object::new()
                                                .with("targetServer", tera_protocol::Value::Uint(server))
                                                .with("target", tera_protocol::Value::Str(sender.clone()))
                                                .with("message", tera_protocol::Value::Str(answer_text.to_string()));
                                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &answer, t0)? {
                                                break 'session;
                                            }
                                        }
                                    } else if let Some(afk) = &cli.afk_reply {
                                        let fresh = afk_replied
                                            .get(&key)
                                            .map(|last| last.elapsed() > Duration::from_secs(300))
                                            .unwrap_or(true);
                                        if fresh && !afk.is_empty() {
                                            afk_replied.insert(key.clone(), Instant::now());
                                            let answer = tera_protocol::Object::new()
                                                .with("targetServer", tera_protocol::Value::Uint(server))
                                                .with("target", tera_protocol::Value::Str(sender.clone()))
                                                .with("message", tera_protocol::Value::Str(afk.clone()));
                                            if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &answer, t0)? {
                                                break 'session;
                                            }
                                        }
                                    }
                                    let relayed = format!("[DM de {sender}] {text}");
                                    let online: Vec<&String> = admin_names
                                        .iter()
                                        .filter(|name| online_admins.contains(&name.to_lowercase()))
                                        .collect();
                                    let targets: Vec<&String> = match cli.whisper_relay.as_str() {
                                        "off" => Vec::new(),
                                        "guild" => Vec::new(),
                                        "whisper" => admin_names.iter().collect(),
                                        _ => online.clone(),
                                    };
                                    let to_guild = match cli.whisper_relay.as_str() {
                                        "off" => false,
                                        "guild" => true,
                                        "whisper" => false,
                                        _ => online.is_empty(),
                                    };
                                    for admin in targets {
                                        let notify = tera_protocol::Object::new()
                                            .with("targetServer", tera_protocol::Value::Uint(server))
                                            .with("target", tera_protocol::Value::Str(admin.clone()))
                                            .with("message", tera_protocol::Value::Str(relayed.clone()));
                                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &notify, t0)? {
                                            break 'session;
                                        }
                                    }
                                    if to_guild {
                                        let notify = tera_protocol::Object::new()
                                            .with("channel", tera_protocol::Value::Uint(2))
                                            .with("message", tera_protocol::Value::Str(relayed.clone()));
                                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &notify, t0)? {
                                            break 'session;
                                        }
                                        logln!("[{:7.3}] DM de {sender} relaye en guilde (aucun admin en ligne)", t0.elapsed().as_secs_f64());
                                    } else if cli.whisper_relay != "off" {
                                        logln!("[{:7.3}] DM de {sender} relaye aux admins en ligne", t0.elapsed().as_secs_f64());
                                    }
                                }
                            }
                        }
                    }
                }
                "S_GUILD_MEMBER_LIST" => {
                    if let Some(definition) = registry.get("S_GUILD_MEMBER_LIST") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let gname = object.get("guildName").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            if !gname.is_empty() {
                                guild_info.name = gname;
                            }
                            let gid = object.get("guildId").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                            if gid != 0 {
                                guild_info.id = gid;
                            }
                            if let Some(v) = object.get("guildMaster").and_then(tera_protocol::Value::as_str) {
                                guild_info.master = v.to_string();
                            }
                            guild_info.level = object.get("guildLevel").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.level);
                            guild_info.funds = object.get("guildFunds").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.funds);
                            guild_info.characters = object.get("characters").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.characters);
                            guild_info.accounts = object.get("accounts").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.accounts);
                            if let Some(tera_protocol::Value::Array(members)) = object.get("members") {
                                for member in members {
                                    let member_name = member.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                                    if member_name.is_empty() {
                                        continue;
                                    }
                                    let key = member_name.to_lowercase();
                                    let online = member.get("status").and_then(tera_protocol::Value::as_int).unwrap_or(2) == 0;
                                    let class = member.get("class").and_then(tera_protocol::Value::as_int).unwrap_or(-1);
                                    let level = member.get("level").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                    let rank = member.get("rank").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                    let zone = member.get("location1").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                    let note = member.get("note").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                                    let contribution = member.get("contributionTotal").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                    if admins.contains(&key) {
                                        if online { online_admins.insert(key.clone()); } else { online_admins.remove(&key); }
                                    }
                                    if online {
                                        state.mark_seen(&member_name);
                                    }
                                    guild_members.insert(key, GuildMember { name: member_name, class, level, online, rank, zone, note, contribution });
                                }
                            }
                        }
                    }
                }
                "S_UPDATE_GUILD_MEMBER" => {
                    if let Some(definition) = registry.get("S_UPDATE_GUILD_MEMBER") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            let member_name = object.get("name").and_then(tera_protocol::Value::as_str).unwrap_or("").to_string();
                            if !member_name.is_empty() {
                                let key = member_name.to_lowercase();
                                let online = object.get("status").and_then(tera_protocol::Value::as_int).unwrap_or(2) == 0;
                                let class = object.get("class").and_then(tera_protocol::Value::as_int).unwrap_or(-1);
                                let level = object.get("level").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                let rank = object.get("rank").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                let zone = object.get("location1").and_then(tera_protocol::Value::as_int).unwrap_or(0);
                                let was_online = guild_members.get(&key).map(|m| m.online).unwrap_or(false);
                                let (note, contribution) = guild_members.get(&key).map(|m| (m.note.clone(), m.contribution)).unwrap_or_default();
                                if admins.contains(&key) {
                                    if online { online_admins.insert(key.clone()); } else { online_admins.remove(&key); }
                                }
                                if cli.greet && online && !was_online && !member_name.eq_ignore_ascii_case(&bot_name) && !guild_members.is_empty() {
                                    let text = cli.greet_text.replace("{name}", &member_name);
                                    let announce = tera_protocol::Object::new()
                                        .with("channel", tera_protocol::Value::Uint(2))
                                        .with("message", tera_protocol::Value::Str(text));
                                    if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_CHAT", &announce, t0)? {
                                        break 'session;
                                    }
                                }
                                if online != was_online {
                                    state.mark_seen(&member_name);
                                }
                                if online && !was_online && !member_name.eq_ignore_ascii_case(&bot_name) {
                                    for mail in state.take_mail(&member_name) {
                                        let body = format!("[message de {}] {}", mail.from, mail.text);
                                        let notify = tera_protocol::Object::new()
                                            .with("targetServer", tera_protocol::Value::Uint(my_server_id))
                                            .with("target", tera_protocol::Value::Str(member_name.clone()))
                                            .with("message", tera_protocol::Value::Str(body));
                                        if reactive_send(&mut stream, &mut enc, &registry, &opcodes, "C_WHISPER", &notify, t0)? {
                                            break 'session;
                                        }
                                    }
                                    if state.is_watched(&member_name) {
                                        let alert = format!("watchlist : {member_name} vient de se connecter");
                                        if send_alert(&mut stream, &mut enc, &registry, &opcodes, t0, &ui, &discord, &admin_names, &online_admins, my_server_id, tui::LogKind::Watch, &alert)? {
                                            break 'session;
                                        }
                                    }
                                }
                                guild_members.insert(key, GuildMember { name: member_name, class, level, online, rank, zone, note, contribution });
                            }
                        }
                    }
                }
                "S_GUILD_INFO" => {
                    if let Some(definition) = registry.get("S_GUILD_INFO") {
                        if let Ok(object) = tera_protocol::value::read(definition, &packet.encode()) {
                            if let Some(v) = object.get("name").and_then(tera_protocol::Value::as_str) { if !v.is_empty() { guild_info.name = v.to_string(); } }
                            if let Some(v) = object.get("master").and_then(tera_protocol::Value::as_str) { guild_info.master = v.to_string(); }
                            if let Some(v) = object.get("motd").and_then(tera_protocol::Value::as_str) { guild_info.motd = v.to_string(); }
                            guild_info.level = object.get("level").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.level);
                            guild_info.funds = object.get("funds").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.funds);
                            guild_info.characters = object.get("characters").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.characters);
                            guild_info.accounts = object.get("accounts").and_then(tera_protocol::Value::as_int).unwrap_or(guild_info.accounts);
                        }
                    }
                }
                "S_SHOW_DEAD_UI" => {
                    session_stats.on_death();
                    activity.log("mort", &zone_names.label(world.player.zone as i64));
                    let alert = format!("le bot est mort (zone {})", world.player.zone);
                    if send_alert(&mut stream, &mut enc, &registry, &opcodes, t0, &ui, &discord, &admin_names, &online_admins, my_server_id, tui::LogKind::Death, &alert)? {
                        break 'session;
                    }
                    if cli.auto_respawn && registry.get("C_REVIVE_NOW").is_some() {
                        let revive = tera_protocol::Object::new()
                            .with("type", tera_protocol::Value::Uint(0))
                            .with("id", tera_protocol::Value::Uint(0));
                        pending_sends.push((Instant::now() + Duration::from_millis(3000), "C_REVIVE_NOW", revive));
                        logln!("[{:7.3}] auto-respawn dans 3s", t0.elapsed().as_secs_f64());
                    }
                }
                "S_RETURN_TO_LOBBY" => {
                    auto.on_leave_game();
                    in_world = false;
                    online_admins.clear();
                    nearby_players.clear();
                }
                _ => {}
            }
        }
    }
    if let Some(b) = broker.as_mut() {
        b.flush();
    }
    broker_open_tried = false;
    broker_dialog_sent = false;
    if user_quit || !cli.reconnect {
        break 'reconnect;
    }
    logln!("[{:7.3}] session terminee, reconnexion automatique", t0.elapsed().as_secs_f64());
    if disconnected_since.is_none() {
        disconnected_since = Some(Instant::now());
    }
    reconnect_attempt += 1;
    }
    logln!("[{:7.3}] fin", t0.elapsed().as_secs_f64());
    if let Some(handle) = &ui {
        handle.quit();
    }
    if let Some(thread) = tui_thread.take() {
        let _ = thread.join();
    }
    Ok(())
}
