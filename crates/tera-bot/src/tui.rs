use std::collections::VecDeque;
use std::io::stdout;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Terminal;

pub const QUIT_SENTINEL: &str = "\u{0}quit";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    Info,
    Auto,
    Gm,
    Boss,
    Ping,
    Death,
    Watch,
    Zone,
    Parcel,
}

impl LogKind {
    fn is_alert(self) -> bool {
        matches!(self, LogKind::Gm | LogKind::Boss | LogKind::Ping | LogKind::Death | LogKind::Watch | LogKind::Zone)
    }

    fn label(self) -> &'static str {
        match self {
            LogKind::Info => "info",
            LogKind::Auto => "auto",
            LogKind::Gm => "GM",
            LogKind::Boss => "boss",
            LogKind::Ping => "ping",
            LogKind::Death => "death",
            LogKind::Watch => "watch",
            LogKind::Zone => "zone",
            LogKind::Parcel => "parcel",
        }
    }

    fn color(self) -> Color {
        match self {
            LogKind::Info => Color::Gray,
            LogKind::Auto => Color::Cyan,
            LogKind::Gm => Color::Red,
            LogKind::Boss => Color::Magenta,
            LogKind::Ping => Color::Yellow,
            LogKind::Death => Color::Red,
            LogKind::Watch => Color::LightBlue,
            LogKind::Zone => Color::Blue,
            LogKind::Parcel => Color::LightGreen,
        }
    }
}

struct LogEntry {
    when: String,
    kind: LogKind,
    text: String,
}

const FILTERS: &[(&str, Option<LogKind>)] = &[
    ("all", None),
    ("auto", Some(LogKind::Auto)),
    ("boss", Some(LogKind::Boss)),
    ("ping", Some(LogKind::Ping)),
    ("GM", Some(LogKind::Gm)),
    ("death", Some(LogKind::Death)),
    ("parcel", Some(LogKind::Parcel)),
    ("info", Some(LogKind::Info)),
];

#[derive(Clone, Default)]
pub struct NearbyEntry {
    pub name: String,
    pub class: String,
    pub level: i64,
    pub dist: f32,
    pub dx: f32,
    pub dy: f32,
    pub gm: bool,
    pub gm_invisible: bool,
    pub guildmate: bool,
    pub guild: String,
}

#[derive(Clone, Default)]
pub struct GuildEntry {
    pub name: String,
    pub class: String,
    pub level: i64,
    pub admin: bool,
}

#[derive(Clone, Default)]
pub struct MemberEntry {
    pub name: String,
    pub hp_pct: f64,
    pub dead: bool,
}

#[derive(Clone, Default)]
pub struct CooldownEntry {
    pub name: String,
    pub remaining: f32,
}

#[derive(Clone, Default)]
pub struct ItemEntry {
    pub name: String,
    pub amount: i32,
    pub enchant: i32,
}

#[derive(Clone, Default)]
pub struct EntityEntry {
    pub label: String,
    pub level: i32,
    pub hp_pct: f64,
    pub dist: f32,
    pub dx: f32,
    pub dy: f32,
    pub boss: bool,
    pub enraged: bool,
    pub aggro_me: bool,
    pub villager: bool,
    pub alive: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub name: String,
    pub level: u16,
    pub class: String,
    pub zone_label: String,
    pub hp: u64,
    pub max_hp: u64,
    pub mp: u64,
    pub max_mp: u64,
    pub auto: String,
    pub guardian: String,
    pub real_players: usize,
    pub npcs: usize,
    pub gm_near: usize,
    pub server: String,
    pub uptime: String,
    pub reconnects: u32,
    pub ping_rtt: Option<u32>,
    pub ping_jitter: u32,
    pub ping_loss: bool,
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub nearby: Vec<NearbyEntry>,
    pub guild_name: String,
    pub guild_master: String,
    pub guild_level: i64,
    pub guild_funds: i64,
    pub guild_motd: String,
    pub guild_online: Vec<GuildEntry>,
    pub guild_total: usize,
    pub members: Vec<MemberEntry>,
    pub cooldowns: Vec<CooldownEntry>,
    pub farm_xp_hr: u64,
    pub farm_kills: u64,
    pub farm_kills_hr: u64,
    pub farm_loot: u64,
    pub farm_deaths: u64,
    pub farm_eta: String,
    pub farm_level_pct: f64,
    pub money: i64,
    pub inventory_slots: usize,
    pub boss_count: usize,
    pub aggro_on_me: usize,
    pub net_in_rate: f64,
    pub net_out_rate: f64,
    pub net_in_pps: f64,
    pub net_out_pps: f64,
    pub traffic: crate::traffic::TrafficSnapshot,
    pub entities: Vec<EntityEntry>,
    pub items: Vec<ItemEntry>,
}

pub enum UiEvent {
    Chat { channel: u32, author: String, text: String },
    Act(String),
    Log { kind: LogKind, text: String },
    State(Box<Snapshot>),
    Quit,
}

pub struct UiHandle {
    tx: Sender<UiEvent>,
    pub input: Receiver<String>,
}

impl UiHandle {
    pub fn chat(&self, channel: u32, author: &str, text: &str) {
        let _ = self.tx.send(UiEvent::Chat { channel, author: author.to_string(), text: text.to_string() });
    }

    pub fn act(&self, text: impl Into<String>) {
        let _ = self.tx.send(UiEvent::Act(text.into()));
    }

    pub fn log(&self, kind: LogKind, text: impl Into<String>) {
        let _ = self.tx.send(UiEvent::Log { kind, text: text.into() });
    }

    pub fn state(&self, snapshot: Snapshot) {
        let _ = self.tx.send(UiEvent::State(Box::new(snapshot)));
    }

    pub fn quit(&self) {
        let _ = self.tx.send(UiEvent::Quit);
    }
}

pub fn channels() -> (Sender<UiEvent>, UiHandle, Sender<String>, Receiver<UiEvent>) {
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let handle = UiHandle { tx: event_tx.clone(), input: input_rx };
    (event_tx, handle, input_tx, event_rx)
}

fn local_offset() -> i64 {
    static OFFSET: OnceLock<i64> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|raw| parse_offset(raw.trim()))
            .unwrap_or(0)
    })
}

fn parse_offset(value: &str) -> Option<i64> {
    if value.len() < 5 {
        return None;
    }
    let sign = if value.starts_with('-') { -1 } else { 1 };
    let hours: i64 = value.get(1..3)?.parse().ok()?;
    let minutes: i64 = value.get(3..5)?.parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

fn clock_now() -> String {
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let day = (epoch + local_offset()).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", day / 3600, (day % 3600) / 60, day % 60)
}

fn channel_label(channel: u32) -> (&'static str, Color) {
    match channel {
        1 => ("party", Color::Cyan),
        2 => ("guild", Color::Green),
        3 => ("area", Color::Yellow),
        21 | 25 => ("notice", Color::Magenta),
        11..=18 => ("whisper", Color::LightMagenta),
        _ => ("say", Color::White),
    }
}

mod theme {
    use ratatui::style::Color;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Palette {
        name: &'static str,
        accent: Color,
        accent2: Color,
        good: Color,
        warn: Color,
        bad: Color,
        text: Color,
        dim: Color,
        muted: Color,
        net_in: Color,
        net_out: Color,
        border: Color,
        sel_bg: Color,
    }

    const PALETTES: &[Palette] = &[
        Palette {
            name: "teal",
            accent: Color::Rgb(0x56, 0xd8, 0xc8),
            accent2: Color::Rgb(0xc9, 0x8b, 0xff),
            good: Color::Rgb(0x6c, 0xd6, 0x7a),
            warn: Color::Rgb(0xf2, 0xc3, 0x4e),
            bad: Color::Rgb(0xef, 0x6a, 0x6a),
            text: Color::Rgb(0xd6, 0xdc, 0xe6),
            dim: Color::Rgb(0x9a, 0xa4, 0xb2),
            muted: Color::Rgb(0x5c, 0x65, 0x73),
            net_in: Color::Rgb(0x6f, 0xb3, 0xff),
            net_out: Color::Rgb(0xf2, 0xa6, 0x5e),
            border: Color::Rgb(0x2c, 0x35, 0x42),
            sel_bg: Color::Rgb(0x1b, 0x2a, 0x30),
        },
        Palette {
            name: "amber",
            accent: Color::Rgb(0xf2, 0xb2, 0x4e),
            accent2: Color::Rgb(0xff, 0x8c, 0x5a),
            good: Color::Rgb(0xa8, 0xcf, 0x5a),
            warn: Color::Rgb(0xf2, 0xd0, 0x6a),
            bad: Color::Rgb(0xe8, 0x5c, 0x4a),
            text: Color::Rgb(0xe8, 0xdd, 0xc8),
            dim: Color::Rgb(0xb0, 0xa0, 0x88),
            muted: Color::Rgb(0x70, 0x64, 0x50),
            net_in: Color::Rgb(0x7a, 0xc8, 0xa8),
            net_out: Color::Rgb(0xf2, 0x9e, 0x5a),
            border: Color::Rgb(0x3a, 0x30, 0x22),
            sel_bg: Color::Rgb(0x2e, 0x24, 0x14),
        },
        Palette {
            name: "mono",
            accent: Color::Rgb(0xc8, 0xd2, 0xdc),
            accent2: Color::Rgb(0x9a, 0xb0, 0xc8),
            good: Color::Rgb(0xa8, 0xc0, 0xa8),
            warn: Color::Rgb(0xd0, 0xc8, 0x98),
            bad: Color::Rgb(0xd0, 0x90, 0x90),
            text: Color::Rgb(0xd8, 0xdc, 0xe0),
            dim: Color::Rgb(0x98, 0xa0, 0xa8),
            muted: Color::Rgb(0x60, 0x66, 0x6c),
            net_in: Color::Rgb(0x9a, 0xb4, 0xcc),
            net_out: Color::Rgb(0xc4, 0xb0, 0x90),
            border: Color::Rgb(0x30, 0x34, 0x38),
            sel_bg: Color::Rgb(0x22, 0x26, 0x2a),
        },
    ];

    static CURRENT: AtomicUsize = AtomicUsize::new(0);

    fn active() -> &'static Palette {
        &PALETTES[CURRENT.load(Ordering::Relaxed) % PALETTES.len()]
    }

    pub fn cycle() {
        CURRENT.fetch_add(1, Ordering::Relaxed);
    }

    pub fn name() -> &'static str {
        active().name
    }

    pub fn accent() -> Color {
        active().accent
    }
    pub fn accent2() -> Color {
        active().accent2
    }
    pub fn good() -> Color {
        active().good
    }
    pub fn warn() -> Color {
        active().warn
    }
    pub fn bad() -> Color {
        active().bad
    }
    pub fn text() -> Color {
        active().text
    }
    pub fn dim() -> Color {
        active().dim
    }
    pub fn muted() -> Color {
        active().muted
    }
    pub fn net_in() -> Color {
        active().net_in
    }
    pub fn net_out() -> Color {
        active().net_out
    }
    pub fn border() -> Color {
        active().border
    }
    pub fn sel_bg() -> Color {
        active().sel_bg
    }
}

const TABS: &[&str] = &["chat", "logs", "map", "group", "network", "world", "bag", "players", "guild"];
const TAB_COUNT: usize = 9;
const TAB_PLAYERS: usize = 7;
const TAB_GUILD: usize = 8;

fn scrollable(tab: usize) -> bool {
    matches!(tab, 0 | 1 | 4 | 5 | 6)
}

fn selectable(tab: usize) -> bool {
    matches!(tab, TAB_PLAYERS | TAB_GUILD)
}

pub fn tab_index(name: &str) -> usize {
    TABS.iter().position(|t| t.eq_ignore_ascii_case(name.trim())).unwrap_or(0)
}

pub fn run(event_rx: Receiver<UiEvent>, input_tx: Sender<String>, start_tab: usize) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let result = render_loop(&event_rx, &input_tx, start_tab.min(TAB_COUNT - 1));
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
    result
}

struct UiState {
    chat: VecDeque<Line<'static>>,
    logs: VecDeque<LogEntry>,
    alerts: VecDeque<LogEntry>,
    snapshot: Snapshot,
    input: String,
    tab: usize,
    scroll: [usize; TAB_COUNT],
    filter: usize,
    sel_player: usize,
    sel_guild: usize,
    net_dir: u8,
    net_sort: u8,
}

impl UiState {
    fn selected_target(&self) -> Option<String> {
        match self.tab {
            TAB_PLAYERS => self.snapshot.nearby.get(self.sel_player).map(|e| e.name.clone()),
            TAB_GUILD => self.snapshot.guild_online.get(self.sel_guild).map(|e| e.name.clone()),
            _ => None,
        }
    }

    fn selection_len(&self) -> usize {
        match self.tab {
            TAB_PLAYERS => self.snapshot.nearby.len(),
            TAB_GUILD => self.snapshot.guild_online.len(),
            _ => 0,
        }
    }

    fn selection_mut(&mut self) -> &mut usize {
        if self.tab == TAB_GUILD {
            &mut self.sel_guild
        } else {
            &mut self.sel_player
        }
    }
}

fn render_loop(event_rx: &Receiver<UiEvent>, input_tx: &Sender<String>, start_tab: usize) -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    let mut ui = UiState {
        chat: VecDeque::new(),
        logs: VecDeque::new(),
        alerts: VecDeque::new(),
        snapshot: Snapshot::default(),
        input: String::new(),
        tab: start_tab,
        scroll: [0; TAB_COUNT],
        filter: 0,
        sel_player: 0,
        sel_guild: 0,
        net_dir: 0,
        net_sort: 0,
    };
    let cap = 1000usize;
    let mut last_draw = Instant::now();

    loop {
        let mut running = true;
        while let Ok(event) = event_rx.try_recv() {
            match event {
                UiEvent::Chat { channel, author, text } => {
                    let (label, color) = channel_label(channel);
                    let mut spans = vec![
                        Span::styled(format!("{} ", clock_now()), Style::default().fg(theme::muted())),
                        Span::styled(format!("{label:>7} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    ];
                    if !author.is_empty() {
                        spans.push(Span::styled(format!("{author}: "), Style::default().fg(color).add_modifier(Modifier::BOLD)));
                    }
                    spans.push(Span::styled(text, Style::default().fg(theme::text())));
                    ui.chat.push_back(Line::from(spans));
                    while ui.chat.len() > cap {
                        ui.chat.pop_front();
                    }
                }
                UiEvent::Act(text) => push_log(&mut ui.logs, &mut ui.alerts, LogKind::Info, text, cap),
                UiEvent::Log { kind, text } => push_log(&mut ui.logs, &mut ui.alerts, kind, text, cap),
                UiEvent::State(next) => ui.snapshot = *next,
                UiEvent::Quit => running = false,
            }
        }

        if last_draw.elapsed() >= Duration::from_millis(33) {
            last_draw = Instant::now();
            terminal.draw(|frame| draw(frame, &ui))?;
        }

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                let scroll = scrollable(ui.tab);
                let select = selectable(ui.tab);
                match key.code {
                    KeyCode::Char('c') if ctrl => {
                        let _ = input_tx.send(QUIT_SENTINEL.to_string());
                        running = false;
                    }
                    KeyCode::Esc => {
                        let _ = input_tx.send(QUIT_SENTINEL.to_string());
                        running = false;
                    }
                    KeyCode::Tab => ui.tab = (ui.tab + 1) % TAB_COUNT,
                    KeyCode::BackTab => ui.tab = (ui.tab + TAB_COUNT - 1) % TAB_COUNT,
                    KeyCode::F(n) if (1..=TAB_COUNT as u8).contains(&n) => ui.tab = (n - 1) as usize,
                    KeyCode::Char('t') if ctrl => theme::cycle(),
                    KeyCode::Char('/') if ui.tab == 1 => ui.filter = (ui.filter + 1) % FILTERS.len(),
                    KeyCode::Char('d') if ui.tab == 4 && ui.input.is_empty() => ui.net_dir = (ui.net_dir + 1) % 3,
                    KeyCode::Char('s') if ui.tab == 4 && ui.input.is_empty() => ui.net_sort = (ui.net_sort + 1) % 2,
                    KeyCode::Char('e') if ui.tab == 4 && ui.input.is_empty() => { let _ = input_tx.send("@netcsv".to_string()); }
                    KeyCode::Char('i') if select && ui.input.is_empty() => {
                        if let Some(name) = ui.selected_target() {
                            let _ = input_tx.send(format!("@invite {name}"));
                        }
                    }
                    KeyCode::Up if select => {
                        let sel = ui.selection_mut();
                        *sel = sel.saturating_sub(1);
                    }
                    KeyCode::Down if select => {
                        let len = ui.selection_len();
                        let sel = ui.selection_mut();
                        if *sel + 1 < len {
                            *sel += 1;
                        }
                    }
                    KeyCode::Enter if select && ui.input.is_empty() => {
                        if let Some(name) = ui.selected_target() {
                            let _ = input_tx.send(format!("@greet {name}"));
                        }
                    }
                    KeyCode::Char(ch) if select && ui.input.is_empty() => {
                        if let Some(name) = ui.selected_target() {
                            match ch {
                                'g' => { let _ = input_tx.send(format!("@greet {name}")); }
                                'f' => { let _ = input_tx.send(format!("@follow {name}")); }
                                'c' => { let _ = input_tx.send(format!("@come {name}")); }
                                'w' => ui.input = format!("@pm {name} "),
                                _ => {}
                            }
                        }
                    }
                    KeyCode::Up if scroll => ui.scroll[ui.tab] += 1,
                    KeyCode::Down if scroll => ui.scroll[ui.tab] = ui.scroll[ui.tab].saturating_sub(1),
                    KeyCode::PageUp if scroll => ui.scroll[ui.tab] += 10,
                    KeyCode::PageDown if scroll => ui.scroll[ui.tab] = ui.scroll[ui.tab].saturating_sub(10),
                    KeyCode::Home if scroll => ui.scroll[ui.tab] = usize::MAX,
                    KeyCode::End if scroll => ui.scroll[ui.tab] = 0,
                    KeyCode::Enter => {
                        let line = std::mem::take(&mut ui.input);
                        let line = line.trim().to_string();
                        if !line.is_empty() {
                            let _ = input_tx.send(line);
                        }
                    }
                    KeyCode::Backspace => {
                        ui.input.pop();
                    }
                    KeyCode::Char(ch) => ui.input.push(ch),
                    _ => {}
                }
            }
        }

        if !running {
            break;
        }
    }
    Ok(())
}

fn push_log(logs: &mut VecDeque<LogEntry>, alerts: &mut VecDeque<LogEntry>, kind: LogKind, text: String, cap: usize) {
    let when = clock_now();
    if kind.is_alert() {
        alerts.push_back(LogEntry { when: when.clone(), kind, text: text.clone() });
        while alerts.len() > cap {
            alerts.pop_front();
        }
    }
    logs.push_back(LogEntry { when, kind, text });
    while logs.len() > cap {
        logs.pop_front();
    }
}

fn panel(title: &str, accent: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()))
        .title(Span::styled(format!(" {title} "), Style::default().fg(accent).add_modifier(Modifier::BOLD)))
}

fn draw(frame: &mut ratatui::Frame, ui: &UiState) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Length(1), Constraint::Min(5), Constraint::Length(3)])
        .split(frame.area());

    draw_header(frame, &ui.snapshot, vertical[0]);
    draw_tabbar(frame, ui.tab, vertical[1]);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(30), Constraint::Length(44)])
        .split(vertical[2]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Percentage(38),
            Constraint::Percentage(32),
            Constraint::Min(4),
        ])
        .split(columns[1]);

    match ui.tab {
        0 => draw_chat(frame, ui, columns[0]),
        1 => draw_logs(frame, ui, columns[0]),
        2 => draw_map(frame, &ui.snapshot, columns[0]),
        3 => draw_group(frame, &ui.snapshot, columns[0]),
        4 => draw_net(frame, ui, columns[0]),
        5 => draw_world(frame, ui, columns[0]),
        6 => draw_bag(frame, ui, columns[0]),
        TAB_PLAYERS => draw_players(frame, ui, columns[0]),
        _ => draw_guild_tab(frame, ui, columns[0]),
    }

    draw_vitals(frame, &ui.snapshot, right[0]);
    draw_nearby(frame, &ui.snapshot, right[1]);
    draw_guild(frame, &ui.snapshot, right[2]);
    draw_alerts(frame, &ui.alerts, right[3]);

    let hints = match ui.tab {
        1 => "/:filter  arrows:scroll",
        0 | 4 | 5 => "arrows/PgUp/PgDn:scroll  End:bottom",
        TAB_PLAYERS | TAB_GUILD => "up/down:select  g:greet f:follow c:come w:whisper",
        _ => "Tab/F1-F9:tabs",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" > ", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
            Span::styled(ui.input.clone(), Style::default().fg(theme::text())),
            Span::styled("\u{2588}", Style::default().fg(theme::accent()).add_modifier(Modifier::SLOW_BLINK)),
        ]))
        .block(
            panel("command", theme::accent()).title(
                Line::from(Span::styled(
                    format!(" {hints}  |  ^t:theme({})  Esc:quit ", theme::name()),
                    Style::default().fg(theme::muted()),
                ))
                .alignment(Alignment::Right),
            ),
        ),
        vertical[3],
    );
}

fn draw_header(frame: &mut ratatui::Frame, s: &Snapshot, area: Rect) {
    let hp_pct = ratio(s.hp, s.max_hp);
    let mp_pct = ratio(s.mp, s.max_mp);
    let identity = if s.name.is_empty() {
        "(hors jeu)".to_string()
    } else if s.class.is_empty() {
        s.name.clone()
    } else {
        format!("{}  {} Lv.{}", s.name, s.class, s.level)
    };
    let mut spans = vec![
        Span::styled("\u{25c6} ", Style::default().fg(theme::accent())),
        Span::styled(identity, Style::default().fg(theme::text()).add_modifier(Modifier::BOLD)),
        sep(),
        Span::styled(mini_gauge(hp_pct as f64), Style::default().fg(hp_color(hp_pct))),
        Span::styled(format!(" {hp_pct:>3}%", ), Style::default().fg(hp_color(hp_pct))),
        Span::styled(" hp ", Style::default().fg(theme::muted())),
        Span::styled(mini_gauge(mp_pct as f64), Style::default().fg(theme::net_in())),
        Span::styled(format!(" {mp_pct:>3}% mp", ), Style::default().fg(theme::net_in())),
        sep(),
        Span::styled(if s.zone_label.is_empty() { "zone ?".to_string() } else { s.zone_label.clone() }, Style::default().fg(theme::accent2())),
        sep(),
        Span::styled(format!("\u{2193}{}/s ", fmt_rate(s.net_in_rate)), Style::default().fg(theme::net_in())),
        Span::styled(format!("\u{2191}{}/s", fmt_rate(s.net_out_rate)), Style::default().fg(theme::net_out())),
        sep(),
        ping_span(s),
    ];
    if s.gm_near > 0 {
        spans.push(Span::styled(format!("  \u{26a0} GM x{}", s.gm_near), Style::default().fg(theme::bad()).add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK)));
    }
    let right = Line::from(vec![
        Span::styled(format!("up {} ", s.uptime), Style::default().fg(theme::muted())),
        Span::styled(clock_now(), Style::default().fg(theme::dim())),
        Span::raw(" "),
    ])
    .alignment(Alignment::Right);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            panel(if s.server.is_empty() { "tera-bot" } else { s.server.as_str() }, theme::accent()).title(right),
        ),
        area,
    );
}

fn sep() -> Span<'static> {
    Span::styled("  \u{2502}  ", Style::default().fg(theme::border()))
}

fn draw_tabbar(frame: &mut ratatui::Frame, active: usize, area: Rect) {
    let mut spans: Vec<Span> = Vec::with_capacity(TABS.len() * 2 + 1);
    spans.push(Span::raw(" "));
    for (index, name) in TABS.iter().enumerate() {
        if index == active {
            spans.push(Span::styled(format!(" {name} "), Style::default().fg(Color::Rgb(0x12, 0x18, 0x20)).bg(theme::accent()).add_modifier(Modifier::BOLD)));
        } else {
            spans.push(Span::styled(format!(" {name} "), Style::default().fg(theme::dim())));
        }
        spans.push(Span::styled("\u{2009}", Style::default().fg(theme::border())));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn visible_bounds(len: usize, height: usize, scroll: usize) -> (usize, usize) {
    let max_scroll = len.saturating_sub(height);
    let clamped = scroll.min(max_scroll);
    let end = len - clamped;
    let start = end.saturating_sub(height);
    (start, end)
}

fn draw_chat(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let height = area.height.saturating_sub(2) as usize;
    let (start, end) = visible_bounds(ui.chat.len(), height, ui.scroll[0]);
    let lines: Vec<Line> = ui.chat.iter().skip(start).take(end - start).cloned().collect();
    let more = ui.chat.len().saturating_sub(end);
    let title = if more > 0 { format!("chat  +{more} \u{2193}") } else { format!("chat  ({})", ui.chat.len()) };
    frame.render_widget(
        Paragraph::new(lines).block(panel(&title, theme::accent())).wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_logs(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let (fname, fkind) = FILTERS[ui.filter];
    let filtered: Vec<&LogEntry> = ui
        .logs
        .iter()
        .filter(|entry| fkind.map(|k| entry.kind == k).unwrap_or(true))
        .collect();
    let height = area.height.saturating_sub(2) as usize;
    let (start, end) = visible_bounds(filtered.len(), height, ui.scroll[1]);
    let lines: Vec<Line> = filtered[start..end]
        .iter()
        .map(|entry| {
            Line::from(vec![
                Span::styled(format!("{} ", entry.when), Style::default().fg(theme::muted())),
                Span::styled(format!("{:<5} ", entry.kind.label()), Style::default().fg(entry.kind.color()).add_modifier(Modifier::BOLD)),
                Span::styled(entry.text.clone(), Style::default().fg(theme::text())),
            ])
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&format!("logs  [{fname}]  ({})", filtered.len()), theme::accent()))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_map(frame: &mut ratatui::Frame, snapshot: &Snapshot, area: Rect) {
    let block = panel("map  @me  o guild  ! GM  . player  x mob", theme::accent());
    let grid_w = area.width.saturating_sub(2) as usize;
    let grid_h = area.height.saturating_sub(2) as usize;
    if grid_w < 5 || grid_h < 3 {
        frame.render_widget(block, area);
        return;
    }
    let mut grid = vec![vec![(' ', Color::Reset); grid_w]; grid_h];
    let center_x = (grid_w / 2) as i32;
    let center_y = (grid_h / 2) as i32;
    let radius = snapshot
        .nearby
        .iter()
        .map(|e| e.dist)
        .chain(snapshot.entities.iter().map(|e| e.dist))
        .fold(0f32, f32::max)
        .clamp(120.0, 600.0);
    let span_x = grid_w as f32 / 2.0 - 1.0;
    let span_y = (grid_h as f32 / 2.0 - 1.0) * 0.55;
    let priority = |ch: char| match ch {
        '!' => 4,
        'o' => 3,
        'x' => 2,
        '.' => 1,
        _ => 0,
    };
    let plot = |dx: f32, dy: f32, ch: char, color: Color, grid: &mut Vec<Vec<(char, Color)>>| {
        let col = center_x + (dx / radius * span_x).round() as i32;
        let row = center_y - (dy / radius * span_y).round() as i32;
        if row < 0 || col < 0 || row >= grid_h as i32 || col >= grid_w as i32 {
            return;
        }
        let cell = &mut grid[row as usize][col as usize];
        if cell.0 == '@' || priority(ch) < priority(cell.0) {
            return;
        }
        *cell = (ch, color);
    };
    for entry in &snapshot.entities {
        if entry.alive {
            plot(entry.dx, entry.dy, 'x', if entry.boss { theme::bad() } else { theme::warn() }, &mut grid);
        }
    }
    for entry in &snapshot.nearby {
        let (ch, color) = if entry.gm || entry.gm_invisible {
            ('!', theme::bad())
        } else if entry.guildmate {
            ('o', theme::good())
        } else {
            ('.', theme::dim())
        };
        plot(entry.dx, entry.dy, ch, color, &mut grid);
    }
    grid[center_y as usize][center_x as usize] = ('@', theme::accent());
    let lines: Vec<Line> = grid
        .into_iter()
        .map(|row| {
            let mut spans: Vec<Span> = Vec::new();
            let mut run = String::new();
            let mut run_color = Color::Reset;
            for (ch, color) in row {
                if color != run_color && !run.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(run_color)));
                }
                run_color = color;
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(Span::styled(run, Style::default().fg(run_color)));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn bar_string(pct: f64, width: usize) -> String {
    let pct = pct.clamp(0.0, 100.0);
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(width - filled))
}

fn mini_gauge(pct: f64) -> String {
    bar_string(pct, 8)
}

fn member_line(name: &str, hp_pct: f64, dead: bool, width: usize) -> Line<'static> {
    let label = format!("{:<12.12} ", name);
    if dead {
        return Line::from(vec![
            Span::styled(label, Style::default().fg(theme::muted())),
            Span::styled("\u{2620} DEAD", Style::default().fg(theme::bad()).add_modifier(Modifier::BOLD)),
        ]);
    }
    let color = hp_color(hp_pct as u64);
    Line::from(vec![
        Span::styled(label, Style::default().fg(theme::text())),
        Span::styled(bar_string(hp_pct, width), Style::default().fg(color)),
        Span::styled(format!(" {hp_pct:>3.0}%"), Style::default().fg(color)),
    ])
}

fn draw_group(frame: &mut ratatui::Frame, snapshot: &Snapshot, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Min(4)])
        .split(area);

    let hp_width = (rows[0].width as usize).saturating_sub(24).clamp(6, 40);
    let mut hp_lines: Vec<Line> = Vec::new();
    let self_pct = ratio(snapshot.hp, snapshot.max_hp) as f64;
    let self_name = if snapshot.name.is_empty() { "me" } else { snapshot.name.as_str() };
    hp_lines.push(member_line(self_name, self_pct, false, hp_width));
    if snapshot.members.is_empty() {
        hp_lines.push(Line::from(Span::styled("  (no party)", Style::default().fg(theme::muted()))));
    }
    let member_cap = (rows[0].height.saturating_sub(3) as usize).max(1);
    for member in snapshot.members.iter().take(member_cap) {
        hp_lines.push(member_line(&member.name, member.hp_pct, member.dead, hp_width));
    }
    frame.render_widget(
        Paragraph::new(hp_lines).block(panel(&format!("party health ({})", snapshot.members.len() + 1), theme::accent())),
        rows[0],
    );

    let cd_width = (rows[1].width as usize).saturating_sub(26).clamp(6, 30);
    let longest = snapshot.cooldowns.iter().map(|c| c.remaining).fold(1.0f32, f32::max);
    let cd_cap = (rows[1].height.saturating_sub(2) as usize).max(1);
    let mut cd_lines: Vec<Line> = Vec::new();
    if snapshot.cooldowns.is_empty() {
        cd_lines.push(Line::from(Span::styled("all skills ready", Style::default().fg(theme::good()))));
    }
    for cd in snapshot.cooldowns.iter().take(cd_cap) {
        let pct = (cd.remaining / longest * 100.0) as f64;
        let color = if cd.remaining < 2.0 { theme::good() } else if cd.remaining < 8.0 { theme::warn() } else { theme::bad() };
        cd_lines.push(Line::from(vec![
            Span::styled(format!("{:<18.18} ", cd.name), Style::default().fg(theme::dim())),
            Span::styled(bar_string(pct, cd_width), Style::default().fg(color)),
            Span::styled(format!(" {:>4.1}s", cd.remaining), Style::default().fg(color)),
        ]));
    }
    frame.render_widget(
        Paragraph::new(cd_lines).block(panel(&format!("cooldowns ({})", snapshot.cooldowns.len()), theme::accent2())),
        rows[1],
    );
}

fn draw_net(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let t = &ui.snapshot.traffic;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(4)])
        .split(area);

    let total = t.in_packets + t.out_packets;
    let summary = vec![
        Line::from(vec![
            Span::styled("\u{2193} in     ", Style::default().fg(theme::net_in()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:>9} pkt  ", fmt_count(t.in_packets)), Style::default().fg(theme::text())),
            Span::styled(format!("{:>9}  ", fmt_bytes(t.in_bytes)), Style::default().fg(theme::text())),
            Span::styled(format!("{}/s  ", fmt_rate(ui.snapshot.net_in_rate)), Style::default().fg(theme::net_in())),
            Span::styled(format!("{:.0} pkt/s", ui.snapshot.net_in_pps), Style::default().fg(theme::dim())),
        ]),
        Line::from(vec![
            Span::styled("\u{2191} out    ", Style::default().fg(theme::net_out()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:>9} pkt  ", fmt_count(t.out_packets)), Style::default().fg(theme::text())),
            Span::styled(format!("{:>9}  ", fmt_bytes(t.out_bytes)), Style::default().fg(theme::text())),
            Span::styled(format!("{}/s  ", fmt_rate(ui.snapshot.net_out_rate)), Style::default().fg(theme::net_out())),
            Span::styled(format!("{:.0} pkt/s", ui.snapshot.net_out_pps), Style::default().fg(theme::dim())),
        ]),
        Line::from(vec![
            Span::styled("total   ", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:>9} pkt  ", fmt_count(total)), Style::default().fg(theme::text())),
            Span::styled(format!("{:>9}  ", fmt_bytes(t.in_bytes + t.out_bytes)), Style::default().fg(theme::text())),
            Span::styled(format!("{:.0}s  ", t.elapsed_s), Style::default().fg(theme::muted())),
            Span::styled(format!("avg {}/s", fmt_rate((t.in_bytes + t.out_bytes) as f64 / t.elapsed_s)), Style::default().fg(theme::dim())),
        ]),
        Line::from(vec![
            Span::styled(format!("{} distinct opcodes   ", t.distinct_opcodes), Style::default().fg(theme::accent2())),
            Span::styled(
                format!("unknown: {} in / {} out", t.unknown_in, t.unknown_out),
                Style::default().fg(if t.unknown_in + t.unknown_out > 0 { theme::warn() } else { theme::muted() }),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(summary).block(panel("network  \u{2014}  live traffic", theme::accent())), rows[0]);

    let dir_label = match ui.net_dir {
        1 => "in",
        2 => "out",
        _ => "all",
    };
    let mut rows_view: Vec<&crate::traffic::OpcodeRow> = t
        .top
        .iter()
        .filter(|r| match ui.net_dir {
            1 => r.incoming,
            2 => !r.incoming,
            _ => true,
        })
        .collect();
    if ui.net_sort == 1 {
        rows_view.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    }
    let height = rows[1].height.saturating_sub(2) as usize;
    let (start, end) = visible_bounds(rows_view.len(), height, ui.scroll[4]);
    let mut lines: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
    lines.push(Line::from(vec![
        Span::styled(" dir ", Style::default().fg(theme::muted())),
        Span::styled(format!("{:>6} ", "opcode"), Style::default().fg(theme::muted())),
        Span::styled(format!("{:<30}", "name"), Style::default().fg(theme::muted())),
        Span::styled(format!("{:>8} ", "packets"), Style::default().fg(theme::muted())),
        Span::styled(format!("{:>9} ", "bytes"), Style::default().fg(theme::muted())),
        Span::styled(format!("{:>6}", "idle"), Style::default().fg(theme::muted())),
    ]));
    for row in rows_view.iter().skip(start).take(end.saturating_sub(start)) {
        let (arrow, acolor) = if row.incoming { ("\u{2193}", theme::net_in()) } else { ("\u{2191}", theme::net_out()) };
        let unknown = row.name == "INCONNU";
        let name_color = if unknown { theme::warn() } else { theme::text() };
        let display_name = if unknown { "UNKNOWN" } else { row.name.as_str() };
        lines.push(Line::from(vec![
            Span::styled(format!("  {arrow}  "), Style::default().fg(acolor)),
            Span::styled(format!("{:>6} ", row.opcode), Style::default().fg(theme::dim())),
            Span::styled(format!("{:<30.30}", display_name), Style::default().fg(name_color)),
            Span::styled(format!("{:>8} ", fmt_count(row.packets)), Style::default().fg(theme::text())),
            Span::styled(format!("{:>9} ", fmt_bytes(row.bytes)), Style::default().fg(theme::accent())),
            Span::styled(format!("{:>5.0}s", row.idle_ms as f64 / 1000.0), Style::default().fg(theme::muted())),
        ]));
    }
    let sort_label = if ui.net_sort == 1 { "bytes" } else { "packets" };
    frame.render_widget(
        Paragraph::new(lines).block(panel(
            &format!("top opcodes ({})  dir:{dir_label} sort:{sort_label}  [d/s/e]", rows_view.len()),
            theme::accent2(),
        )),
        rows[1],
    );
}

fn draw_world(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let s = &ui.snapshot;
    let height = area.height.saturating_sub(3) as usize;
    let hp_width = (area.width as usize).saturating_sub(40).clamp(6, 24);
    let (start, end) = visible_bounds(s.entities.len(), height.max(1), ui.scroll[5]);
    let mut lines: Vec<Line> = Vec::new();
    if s.entities.is_empty() {
        lines.push(Line::from(Span::styled("no npc/mob nearby", Style::default().fg(theme::muted()))));
    }
    for entry in s.entities.iter().skip(start).take(end.saturating_sub(start)) {
        let hp_color = hp_color(entry.hp_pct as u64);
        let name_color = if !entry.alive {
            theme::muted()
        } else if entry.boss {
            theme::bad()
        } else if entry.villager {
            theme::accent()
        } else {
            theme::text()
        };
        let mut spans = vec![
            Span::styled(format!("{:>5.0}m ", entry.dist), Style::default().fg(theme::muted())),
            Span::styled(format!("{:<16.16}", entry.label), Style::default().fg(name_color).add_modifier(if entry.boss { Modifier::BOLD } else { Modifier::empty() })),
            Span::styled(format!("L{:<3} ", entry.level.max(0)), Style::default().fg(theme::dim())),
        ];
        if entry.alive {
            spans.push(Span::styled(bar_string(entry.hp_pct, hp_width), Style::default().fg(hp_color)));
            spans.push(Span::styled(format!(" {:>3.0}%", entry.hp_pct), Style::default().fg(hp_color)));
        } else {
            spans.push(Span::styled("\u{2620} dead", Style::default().fg(theme::muted())));
        }
        if entry.boss {
            spans.push(Span::styled(" BOSS", Style::default().fg(theme::bad()).add_modifier(Modifier::BOLD)));
        }
        if entry.enraged {
            spans.push(Span::styled(" ENRAGED", Style::default().fg(theme::warn()).add_modifier(Modifier::BOLD | Modifier::RAPID_BLINK)));
        }
        if entry.aggro_me {
            spans.push(Span::styled(" \u{2694}aggro", Style::default().fg(theme::bad())));
        }
        if entry.villager {
            spans.push(Span::styled(" npc", Style::default().fg(theme::accent())));
        }
        lines.push(Line::from(spans));
    }
    let title = format!(
        "world  {} mobs  {} boss  {} aggro",
        s.npcs, s.boss_count, s.aggro_on_me
    );
    frame.render_widget(
        Paragraph::new(lines).block(panel(&title, theme::accent())),
        area,
    );
}

fn draw_bag(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let s = &ui.snapshot;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(4)])
        .split(area);

    let (gold, silver, copper) = split_money(s.money);
    let eco = vec![
        Line::from(vec![
            Span::styled("wealth   ", Style::default().fg(theme::dim())),
            Span::styled(format!("{gold}"), Style::default().fg(theme::warn()).add_modifier(Modifier::BOLD)),
            Span::styled(" gold ", Style::default().fg(theme::muted())),
            Span::styled(format!("{silver}"), Style::default().fg(theme::dim())),
            Span::styled(" silver ", Style::default().fg(theme::muted())),
            Span::styled(format!("{copper}"), Style::default().fg(Color::Rgb(0xc8, 0x8a, 0x5a))),
            Span::styled(" copper", Style::default().fg(theme::muted())),
        ]),
        Line::from(vec![
            Span::styled("loot ", Style::default().fg(theme::dim())),
            Span::styled(format!("{}  ", s.farm_loot), Style::default().fg(theme::good())),
            Span::styled("xp/h ", Style::default().fg(theme::dim())),
            Span::styled(format!("{}  ", fmt_count(s.farm_xp_hr)), Style::default().fg(theme::accent2())),
            Span::styled(format!("lvl {:>3.0}% -> {}  ", s.farm_level_pct, s.farm_eta), Style::default().fg(theme::accent2())),
            Span::styled(format!("{} kills ({}/h)  ", s.farm_kills, s.farm_kills_hr), Style::default().fg(theme::warn())),
            Span::styled(
                format!("{} deaths", s.farm_deaths),
                Style::default().fg(if s.farm_deaths > 0 { theme::bad() } else { theme::muted() }),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(eco).block(panel("economy & farm", theme::warn())), rows[0]);

    let height = rows[1].height.saturating_sub(2) as usize;
    let (start, end) = visible_bounds(s.items.len(), height.max(1), ui.scroll[6]);
    let mut lines: Vec<Line> = Vec::new();
    if s.items.is_empty() {
        lines.push(Line::from(Span::styled("inventory not loaded yet", Style::default().fg(theme::muted()))));
    }
    for item in s.items.iter().skip(start).take(end.saturating_sub(start)) {
        let mut spans = vec![
            Span::styled(format!("{:>4}x ", item.amount), Style::default().fg(theme::dim())),
            Span::styled(item.name.clone(), Style::default().fg(theme::text())),
        ];
        if item.enchant > 0 {
            spans.push(Span::styled(format!(" +{}", item.enchant), Style::default().fg(theme::accent())));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(&format!("bag  ({} items)", s.inventory_slots), theme::good())),
        rows[1],
    );
}

fn action_hint() -> Line<'static> {
    Line::from(vec![
        Span::styled("  [g]", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(" greet ", Style::default().fg(theme::dim())),
        Span::styled("[f]", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(" follow ", Style::default().fg(theme::dim())),
        Span::styled("[c]", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(" come ", Style::default().fg(theme::dim())),
        Span::styled("[w]", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(" whisper ", Style::default().fg(theme::dim())),
        Span::styled("[enter]", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
        Span::styled(" greet", Style::default().fg(theme::dim())),
    ])
}

fn selection_row(selected: bool, spans: Vec<Span<'static>>) -> Line<'static> {
    let mut row = Vec::with_capacity(spans.len() + 1);
    if selected {
        row.push(Span::styled("\u{25b8} ", Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)));
    } else {
        row.push(Span::raw("  "));
    }
    let style = if selected {
        Style::default().bg(theme::sel_bg())
    } else {
        Style::default()
    };
    for mut span in spans {
        span.style = span.style.patch(style);
        row.push(span);
    }
    Line::from(row)
}

fn draw_players(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let s = &ui.snapshot;
    let len = s.nearby.len();
    let sel = ui.sel_player.min(len.saturating_sub(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let height = rows[0].height.saturating_sub(2) as usize;
    let start = sel.saturating_sub(height.saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    if len == 0 {
        lines.push(Line::from(Span::styled("  nobody nearby", Style::default().fg(theme::muted()))));
    }
    for (index, entry) in s.nearby.iter().enumerate().skip(start).take(height.max(1)) {
        let (tag, name_color) = if entry.gm || entry.gm_invisible {
            (if entry.gm_invisible { "[GM-INV] " } else { "[GM] " }, theme::bad())
        } else if entry.guildmate {
            ("[guild] ", theme::good())
        } else {
            ("", theme::text())
        };
        let mut spans = vec![
            Span::styled(format!("{:>5.0}m ", entry.dist), Style::default().fg(theme::muted())),
            Span::styled(format!("{:<18.18}", entry.name), Style::default().fg(name_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<11} L{:<3}", entry.class, entry.level), Style::default().fg(theme::dim())),
        ];
        if !tag.is_empty() {
            spans.insert(2, Span::styled(tag, Style::default().fg(name_color).add_modifier(Modifier::BOLD)));
        }
        if !entry.guildmate && !entry.gm && !entry.gm_invisible && !entry.guild.is_empty() {
            spans.push(Span::styled(format!(" <{}>", entry.guild), Style::default().fg(theme::muted())));
        }
        lines.push(selection_row(index == sel, spans));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(&format!("players  ({len})  {}/{}", sel.min(len.saturating_sub(1)) + (len > 0) as usize, len), theme::accent())),
        rows[0],
    );
    frame.render_widget(Paragraph::new(action_hint()), rows[1]);
}

fn draw_guild_tab(frame: &mut ratatui::Frame, ui: &UiState, area: Rect) {
    let s = &ui.snapshot;
    let len = s.guild_online.len();
    let sel = ui.sel_guild.min(len.saturating_sub(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);
    let height = rows[0].height.saturating_sub(2) as usize;
    let start = sel.saturating_sub(height.saturating_sub(1));
    let mut lines: Vec<Line> = Vec::new();
    if len == 0 {
        lines.push(Line::from(Span::styled("  no guildmate online", Style::default().fg(theme::muted()))));
    }
    for (index, member) in s.guild_online.iter().enumerate().skip(start).take(height.max(1)) {
        let marker = if member.admin { "\u{2605} " } else { "  " };
        let spans = vec![
            Span::styled(marker, Style::default().fg(theme::warn())),
            Span::styled(format!("{:<18.18}", member.name), Style::default().fg(if member.admin { theme::warn() } else { theme::good() }).add_modifier(Modifier::BOLD)),
            Span::styled(format!("{:<11} L{:<3}", member.class, member.level), Style::default().fg(theme::dim())),
            if member.admin { Span::styled("admin", Style::default().fg(theme::warn())) } else { Span::raw("") },
        ];
        lines.push(selection_row(index == sel, spans));
    }
    let name = if s.guild_name.is_empty() { "guild".to_string() } else { s.guild_name.clone() };
    frame.render_widget(
        Paragraph::new(lines).block(panel(&format!("{name}  ({}/{} online)", len, s.guild_total), theme::good())),
        rows[0],
    );
    frame.render_widget(Paragraph::new(action_hint()), rows[1]);
}

fn draw_alerts(frame: &mut ratatui::Frame, alerts: &VecDeque<LogEntry>, area: Rect) {
    let height = (area.height.saturating_sub(2) as usize).max(1);
    let mut lines: Vec<Line> = Vec::with_capacity(height);
    if alerts.is_empty() {
        lines.push(Line::from(Span::styled("no alert", Style::default().fg(theme::muted()))));
    }
    for entry in alerts.iter().rev().take(height).rev() {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", entry.when), Style::default().fg(theme::muted())),
            Span::styled(format!("{} ", entry.kind.label()), Style::default().fg(entry.kind.color()).add_modifier(Modifier::BOLD)),
            Span::styled(entry.text.clone(), Style::default().fg(entry.kind.color())),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel(&format!("alerts ({})", alerts.len()), theme::bad()))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_vitals(frame: &mut ratatui::Frame, s: &Snapshot, area: Rect) {
    let hp_pct = ratio(s.hp, s.max_hp);
    let mp_pct = ratio(s.mp, s.max_mp);
    let width = (area.width as usize).saturating_sub(18).clamp(6, 26);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("hp   ", Style::default().fg(theme::dim())),
            Span::styled(bar_string(hp_pct as f64, width), Style::default().fg(hp_color(hp_pct))),
            Span::styled(format!(" {hp_pct:>3}%"), Style::default().fg(hp_color(hp_pct))),
        ]),
        Line::from(vec![
            Span::styled("mp   ", Style::default().fg(theme::dim())),
            Span::styled(bar_string(mp_pct as f64, width), Style::default().fg(theme::net_in())),
            Span::styled(format!(" {mp_pct:>3}%"), Style::default().fg(theme::net_in())),
        ]),
        Line::from(Span::styled(
            format!("pos {:.0}, {:.0}, {:.0}", s.pos_x, s.pos_y, s.pos_z),
            Style::default().fg(theme::muted()),
        )),
        Line::from(vec![
            Span::styled(format!("{} players  ", s.real_players), Style::default().fg(theme::warn())),
            Span::styled(format!("{} mobs", s.npcs), Style::default().fg(theme::dim())),
            if s.aggro_on_me > 0 {
                Span::styled(format!("  \u{2694}{}", s.aggro_on_me), Style::default().fg(theme::bad()).add_modifier(Modifier::BOLD))
            } else {
                Span::raw("")
            },
        ]),
    ];
    if !s.auto.is_empty() {
        lines.push(Line::from(Span::styled(s.auto.clone(), Style::default().fg(theme::accent()))));
    }
    if !s.guardian.is_empty() {
        lines.push(Line::from(Span::styled(s.guardian.clone(), Style::default().fg(theme::accent()))));
    }
    if s.reconnects > 0 {
        lines.push(Line::from(Span::styled(format!("reconnects: {}", s.reconnects), Style::default().fg(theme::muted()))));
    }
    frame.render_widget(Paragraph::new(lines).block(panel("vitals", theme::accent())), area);
}

fn draw_nearby(frame: &mut ratatui::Frame, snapshot: &Snapshot, area: Rect) {
    let cap = area.height.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(cap);
    if snapshot.nearby.is_empty() {
        lines.push(Line::from(Span::styled("nobody nearby", Style::default().fg(theme::muted()))));
    }
    for entry in snapshot.nearby.iter().take(cap) {
        let dist = format!("{:>4.0}m ", entry.dist);
        let (tag, name_color) = if entry.gm || entry.gm_invisible {
            (if entry.gm_invisible { "[GM-INV] " } else { "[GM] " }, theme::bad())
        } else if entry.guildmate {
            ("[guild] ", theme::good())
        } else {
            ("", theme::text())
        };
        let mut spans = vec![
            Span::styled(dist, Style::default().fg(theme::muted())),
            Span::styled(entry.name.clone(), Style::default().fg(name_color).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {} {}", entry.class, entry.level), Style::default().fg(theme::dim())),
        ];
        if !tag.is_empty() {
            spans.insert(1, Span::styled(tag, Style::default().fg(name_color).add_modifier(Modifier::BOLD)));
        }
        if !entry.guildmate && !entry.gm && !entry.gm_invisible && !entry.guild.is_empty() {
            spans.push(Span::styled(format!(" <{}>", entry.guild), Style::default().fg(theme::muted())));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines).block(panel(&format!("players nearby ({})", snapshot.nearby.len()), theme::accent())),
        area,
    );
}

fn ping_span(snapshot: &Snapshot) -> Span<'static> {
    match snapshot.ping_rtt {
        Some(rtt) => {
            let color = if snapshot.ping_loss {
                theme::bad()
            } else if rtt < 80 {
                theme::good()
            } else if rtt < 160 {
                theme::warn()
            } else {
                theme::bad()
            };
            let loss = if snapshot.ping_loss { " perte!" } else { "" };
            Span::styled(format!("{rtt}ms ~{}ms{loss}", snapshot.ping_jitter), Style::default().fg(color))
        }
        None => Span::styled("ping -", Style::default().fg(theme::muted())),
    }
}

fn draw_guild(frame: &mut ratatui::Frame, snapshot: &Snapshot, area: Rect) {
    let cap = (area.height.saturating_sub(2) as usize).max(1);
    let mut lines: Vec<Line> = Vec::with_capacity(cap);
    if !snapshot.guild_master.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("master ", Style::default().fg(theme::muted())),
            Span::styled(snapshot.guild_master.clone(), Style::default().fg(theme::warn())),
            Span::styled(format!("  Lv.{}  ", snapshot.guild_level), Style::default().fg(theme::dim())),
            Span::styled(format!("{} gold", fmt_money(snapshot.guild_funds)), Style::default().fg(theme::muted())),
        ]));
    }
    if !snapshot.guild_motd.is_empty() {
        lines.push(Line::from(Span::styled(format!("\"{}\"", snapshot.guild_motd), Style::default().fg(theme::accent()))));
    }
    if snapshot.guild_online.is_empty() {
        lines.push(Line::from(Span::styled("no member online", Style::default().fg(theme::muted()))));
    }
    let room = cap.saturating_sub(lines.len());
    for member in snapshot.guild_online.iter().take(room) {
        let marker = if member.admin { "\u{2605} " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme::warn())),
            Span::styled(member.name.clone(), Style::default().fg(if member.admin { theme::warn() } else { theme::good() }).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {} {}", member.class, member.level), Style::default().fg(theme::muted())),
        ]));
    }
    let name = if snapshot.guild_name.is_empty() { "guild".to_string() } else { snapshot.guild_name.clone() };
    let title = format!("{name} ({}/{} online)", snapshot.guild_online.len(), snapshot.guild_total);
    frame.render_widget(
        Paragraph::new(lines).block(panel(&title, theme::good())),
        area,
    );
}

fn split_money(money: i64) -> (i64, i64, i64) {
    let money = money.max(0);
    (money / 10_000, (money % 10_000) / 100, money % 100)
}

fn fmt_money(gold: i64) -> String {
    if gold >= 10_000 {
        format!("{}g", gold / 10_000)
    } else {
        gold.to_string()
    }
}

fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn fmt_bytes(n: u64) -> String {
    let n = n as f64;
    if n >= 1_073_741_824.0 {
        format!("{:.2} Go", n / 1_073_741_824.0)
    } else if n >= 1_048_576.0 {
        format!("{:.2} Mo", n / 1_048_576.0)
    } else if n >= 1024.0 {
        format!("{:.1} Ko", n / 1024.0)
    } else {
        format!("{n:.0} o")
    }
}

fn fmt_rate(bytes_per_s: f64) -> String {
    if bytes_per_s >= 1_048_576.0 {
        format!("{:.1} Mo", bytes_per_s / 1_048_576.0)
    } else if bytes_per_s >= 1024.0 {
        format!("{:.1} Ko", bytes_per_s / 1024.0)
    } else {
        format!("{bytes_per_s:.0} o")
    }
}

fn ratio(value: u64, max: u64) -> u64 {
    value.saturating_mul(100).checked_div(max).unwrap_or(0)
}

fn hp_color(pct: u64) -> Color {
    if pct >= 60 {
        theme::good()
    } else if pct >= 30 {
        theme::warn()
    } else {
        theme::bad()
    }
}
