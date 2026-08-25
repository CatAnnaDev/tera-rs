use crate::serverlist::{Encoding, Server, ServerList, TextEncoding};
use std::ffi::OsStr;
use std::net::Ipv4Addr;
use std::os::windows::ffi::OsStrExt;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostQuitMessage,
    RegisterClassExW, SendMessageW, TranslateMessage, MSG, WM_COPYDATA, WM_DESTROY, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};

const CLASS_NAME: &str = "LAUNCHER_CLASS";
const WINDOW_NAME: &str = "LAUNCHER_WINDOW";

const EVENT_ACCOUNT_NAME_REQUEST: usize = 1;
const EVENT_ACCOUNT_NAME_REPLY: usize = 2;
const EVENT_TICKET_REQUEST: usize = 3;
const EVENT_TICKET_REPLY: usize = 4;
const EVENT_SERVER_LIST_REQUEST: usize = 5;
const EVENT_SERVER_LIST_REPLY: usize = 6;

pub struct Config {
    pub account: String,
    pub ticket: String,
    pub host: String,
    pub port: u16,
    pub server_name: String,
    pub game: String,
    pub language: String,
    pub serverlist: Option<Vec<u8>>,
    pub language_id: i32,
    pub probe: bool,
}

impl Config {
    fn from_arguments() -> Self {
        let mut config = Self {
            account: "10000".into(),
            ticket: "0123456789abcdef0123456789abcdef".into(),
            host: "127.0.0.1".into(),
            port: 10001,
            server_name: "Local".into(),
            game: String::new(),
            language: "EUR".into(),
            serverlist: None,
            language_id: 1,
            probe: false,
        };
        let arguments: Vec<String> = std::env::args().collect();
        config.probe = arguments.iter().any(|value| value == "--probe");
        for pair in arguments.windows(2) {
            match pair[0].as_str() {
                "--account" => config.account = pair[1].clone(),
                "--ticket" => config.ticket = pair[1].clone(),
                "--host" => config.host = pair[1].clone(),
                "--port" => config.port = pair[1].parse().unwrap_or(10001),
                "--server-name" => config.server_name = pair[1].clone(),
                "--game" => config.game = pair[1].clone(),
                "--language" => config.language = pair[1].clone(),
                "--language-id" => config.language_id = pair[1].parse().unwrap_or(1),
                "--serverlist" => {
                    config.serverlist = std::fs::read(&pair[1])
                        .map_err(|error| println!("cannot read {}: {error}", pair[1]))
                        .ok()
                }
                _ => {}
            }
        }
        config
    }

    fn server_list(&self, language: i32, encoding: Encoding) -> Vec<u8> {
        ServerList {
            servers: vec![Server::local(
                1,
                &self.server_name,
                self.address(),
                self.port,
                language,
            )],
            last_played_id: 1,
            unknown: 0,
        }
        .encode(&encoding)
    }

    fn candidates(&self) -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("A every field emitted, utf-16, lang 1", self.server_list(1, Encoding::TERA)),
            ("B lang 0", self.server_list(0, Encoding::TERA)),
            ("C lang 2", self.server_list(2, Encoding::TERA)),
            (
                "D proto3 defaults, empty popup skipped",
                self.server_list(1, Encoding::PROTO3_DEFAULTS),
            ),
            (
                "E nul-terminated utf-16 strings",
                self.server_list(1, Encoding::TERA.nul_terminated()),
            ),
            (
                "F utf-8 strings",
                self.server_list(1, Encoding::TERA.with_text(TextEncoding::Utf8)),
            ),
        ]
    }

    fn address(&self) -> Ipv4Addr {
        self.host.parse().unwrap_or(Ipv4Addr::LOCALHOST)
    }
}

static CONFIG: std::sync::OnceLock<Config> = std::sync::OnceLock::new();
static ATTEMPT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(Some(0)).collect()
}

fn utf16_payload(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

unsafe fn reply(target: HWND, source: HWND, event: usize, payload: &[u8]) {
    let data = COPYDATASTRUCT {
        dwData: event,
        cbData: payload.len() as u32,
        lpData: payload.as_ptr() as *mut _,
    };
    let result = SendMessageW(
        target,
        WM_COPYDATA,
        source as WPARAM,
        &data as *const _ as LPARAM,
    );
    println!("-> event {event}, {} bytes, result {result}", payload.len());
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_COPYDATA => {
            let data = &*(lparam as *const COPYDATASTRUCT);
            let sender = wparam as HWND;
            let payload = if data.cbData == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize).to_vec()
            };
            println!("<- event {}, {} bytes", data.dwData, payload.len());
            if !payload.is_empty() {
                let hex: Vec<String> = payload
                    .iter()
                    .take(64)
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                println!("   hex  {}", hex.join(" "));
                let text: String = payload
                    .iter()
                    .take(160)
                    .map(|byte| {
                        if byte.is_ascii_graphic() || *byte == b' ' {
                            *byte as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                println!("   text {text}");
                if payload.len() >= 2 && payload.len() % 2 == 0 {
                    let units: Vec<u16> = payload
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|pair| u16::from_le_bytes(*pair))
                        .take(80)
                        .collect();
                    let wide = String::from_utf16_lossy(&units);
                    if wide.chars().filter(|c| c.is_ascii_graphic()).count() > units.len() / 2 {
                        println!("   utf16 {wide}");
                    }
                }
            }
            let config = CONFIG.get().expect("config");
            match data.dwData {
                EVENT_ACCOUNT_NAME_REQUEST => {
                    reply(
                        sender,
                        window,
                        EVENT_ACCOUNT_NAME_REPLY,
                        &utf16_payload(&config.account),
                    );
                }
                EVENT_TICKET_REQUEST => {
                    reply(
                        sender,
                        window,
                        EVENT_TICKET_REPLY,
                        &utf16_payload(&config.ticket),
                    );
                }
                EVENT_SERVER_LIST_REQUEST => {
                    let payload = if let Some(custom) = &config.serverlist {
                        println!("   serving {} bytes from --serverlist", custom.len());
                        custom.clone()
                    } else if config.probe {
                        let candidates = config.candidates();
                        let index = ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            % candidates.len();
                        let (label, payload) = &candidates[index];
                        println!("   probing candidate {label} ({} bytes)", payload.len());
                        payload.clone()
                    } else {
                        let payload = config.server_list(config.language_id, Encoding::TERA);
                        println!(
                            "   serving {} at {}:{} ({} bytes)",
                            config.server_name,
                            config.host,
                            config.port,
                            payload.len()
                        );
                        payload
                    };
                    reply(sender, window, EVENT_SERVER_LIST_REPLY, &payload);
                }
                other => println!("   (no reply for event {other})"),
            }
            1
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn spawn_game(config: &Config) -> Result<(), String> {
    if config.game.is_empty() {
        println!("no --game given, waiting for a client that is already running");
        return Ok(());
    }
    let command = format!("\"{}\" -LANGUAGEEXT={}", config.game, config.language);
    let mut command = wide(&command);
    let mut startup: STARTUPINFOW = std::mem::zeroed();
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut information: PROCESS_INFORMATION = std::mem::zeroed();
    let directory = std::path::Path::new(&config.game)
        .parent()
        .map(|value| wide(&value.to_string_lossy()));
    let created = CreateProcessW(
        std::ptr::null(),
        command.as_mut_ptr(),
        std::ptr::null(),
        std::ptr::null(),
        0,
        0,
        std::ptr::null(),
        directory
            .as_ref()
            .map(|value| value.as_ptr())
            .unwrap_or(std::ptr::null()),
        &startup,
        &mut information,
    );
    if created == 0 {
        return Err("CreateProcessW failed".into());
    }
    println!("launched {} (pid {})", config.game, information.dwProcessId);
    Ok(())
}

pub fn run() -> Result<(), String> {
    let config = Config::from_arguments();
    println!(
        "serving {}:{} as \"{}\" for account {}",
        config.host, config.port, config.server_name, config.account
    );
    unsafe {
        let _ = CONFIG.set(config);
        let instance = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS_NAME);
        let title = wide(WINDOW_NAME);
        let mut descriptor: WNDCLASSEXW = std::mem::zeroed();
        descriptor.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        descriptor.lpfnWndProc = Some(window_procedure);
        descriptor.hInstance = instance;
        descriptor.lpszClassName = class.as_ptr();
        if RegisterClassExW(&descriptor) == 0 {
            return Err("RegisterClassExW failed".into());
        }
        let window = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            320,
            200,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if window.is_null() {
            return Err("CreateWindowExW failed".into());
        }
        println!("registered {CLASS_NAME} / {WINDOW_NAME}");
        spawn_game(CONFIG.get().expect("config"))?;

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}
