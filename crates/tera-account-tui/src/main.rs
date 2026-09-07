use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, Wrap};
use ratatui::DefaultTerminal;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::time::{Duration, Instant};

const BASE: &str = "https://tera-europe-classic.com/api";
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

const ACCENT: Color = Color::Rgb(96, 200, 250);
const GOLD: Color = Color::Rgb(244, 202, 96);
const GREEN: Color = Color::Rgb(90, 220, 130);
const RED: Color = Color::Rgb(244, 96, 96);
const DIM: Color = Color::Rgb(130, 130, 150);
const FG: Color = Color::Rgb(226, 226, 236);

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Account {
    account_id: i64,
    user_name: String,
    email: String,
    language: String,
    play_time_total_hours: String,
    play_count: i64,
    register_time: String,
    last_login_time: Option<String>,
    highest_level: i64,
    leaderboard_consent: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Session {
    authenticated: bool,
    user: SessionUser,
    oauth_providers: Vec<OauthProvider>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SessionUser {
    shop_access: bool,
    patch_notes_access: bool,
    guides_access: bool,
    statistics_access: bool,
    archive_access: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct OauthProvider {
    provider: String,
    label: String,
    enabled: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Discord {
    members: i64,
    online: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct StoreRegion {
    name: String,
    currency_code: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PaymentMethod {
    id: String,
    available: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Profile {
    account: Account,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Summary {
    tera_coins: i64,
    adventure_coins: Option<i64>,
    total_gold: i64,
    total_playtime_sec: i64,
    total_achievements: i64,
    character_count: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Characters {
    characters: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Slots {
    current: i64,
    maximum: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Vip {
    active: bool,
    expires_at: Option<String>,
    recurring: bool,
    daily_tokens: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Product {
    id: String,
    kind: String,
    amount: Option<i64>,
    days: Option<i64>,
    slots: Option<i64>,
    price_cents: i64,
    bonus_percent: Option<i64>,
    saving_percent: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Shop {
    mode: String,
    currency: String,
    token_balance: i64,
    character_slots: Slots,
    vip: Vip,
    vip_plus: Vip,
    founder: bool,
    products: Vec<Product>,
    payment_methods: Vec<PaymentMethod>,
    transactions: Vec<serde_json::Value>,
    checkout_mode: String,
    catalog_source: String,
    store_region: StoreRegion,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Launcher {
    available: bool,
    version: String,
    download_url: String,
    size_bytes: i64,
    published_at: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Item {
    id: i64,
    name: String,
    rarity: String,
    category: String,
    combat_item_type: String,
    level: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Catalog {
    total: i64,
    page: i64,
    page_size: i64,
    items: Vec<Item>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RankPlayer {
    name: String,
    class: String,
    dps: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Ranking {
    area_name: String,
    boss_name: String,
    party_dps: i64,
    fight_duration: i64,
    players: Vec<RankPlayer>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Rankings {
    total: i64,
    items: Vec<Ranking>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Totals {
    characters: i64,
    accounts: i64,
    level65_count: i64,
    dungeon_clears: i64,
    combat_parses: i64,
    total_damage: i64,
    hours_played: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Record {
    kind: String,
    value: String,
    unit: String,
    who: String,
    class_name: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Overview {
    realm: String,
    sample_window_label: String,
    totals: Totals,
    records: Vec<Record>,
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Account,
    Shop,
    Characters,
    Access,
    Community,
    Items,
    Leaderboard,
    Stats,
}

const TABS: [(&str, Tab); 8] = [
    ("Compte", Tab::Account),
    ("Boutique", Tab::Shop),
    ("Persos", Tab::Characters),
    ("Acces", Tab::Access),
    ("Communaute", Tab::Community),
    ("Items", Tab::Items),
    ("Leaderboard", Tab::Leaderboard),
    ("Stats", Tab::Stats),
];

struct App {
    cookies: String,
    tab: Tab,
    profile: Option<Profile>,
    summary: Option<Summary>,
    characters: Option<Characters>,
    shop: Option<Shop>,
    launcher: Option<Launcher>,
    session: Option<Session>,
    discord: Option<Discord>,
    catalog: Option<Catalog>,
    rankings: Option<Rankings>,
    overview: Option<Overview>,
    items_page: i64,
    errors: Vec<String>,
    last: Instant,
}

fn get<T: DeserializeOwned>(cookies: &str, ep: &str) -> Result<T> {
    let mut r = ureq::get(format!("{BASE}/{ep}"))
        .header("Cookie", cookies)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .header("Referer", "https://tera-europe-classic.com/")
        .call()
        .with_context(|| format!("GET {ep}"))?;
    r.body_mut().read_json().with_context(|| format!("JSON {ep}"))
}

fn load_cookies() -> Result<String> {
    if let Ok(c) = std::env::var("TERA_COOKIES") {
        if !c.trim().is_empty() {
            return Ok(c.trim().to_string());
        }
    }
    let path = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            dirs_config()
                .join("tera-classic")
                .join("cookies.txt")
        });
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("lecture des cookies {} (mets ton header Cookie dedans, ou TERA_COOKIES=...)", path.display()))?;
    Ok(raw.trim().to_string())
}

fn dirs_config() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| std::path::PathBuf::from(".config"))
        })
}

fn main() -> Result<()> {
    let cookies = load_cookies()?;
    let mut app = App {
        cookies,
        tab: Tab::Account,
        profile: None,
        summary: None,
        characters: None,
        shop: None,
        launcher: None,
        session: None,
        discord: None,
        catalog: None,
        rankings: None,
        overview: None,
        items_page: 1,
        errors: Vec::new(),
        last: Instant::now(),
    };
    refresh(&mut app);
    let mut term = ratatui::init();
    let res = run(&mut term, &mut app);
    ratatui::restore();
    res
}

fn run(term: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        term.draw(|f| ui(f, app))?;
        if event::poll(Duration::from_millis(300))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('r') => refresh(app),
                        KeyCode::Char('1') => app.tab = Tab::Account,
                        KeyCode::Char('2') => app.tab = Tab::Shop,
                        KeyCode::Char('3') => app.tab = Tab::Characters,
                        KeyCode::Char('4') => app.tab = Tab::Access,
                        KeyCode::Char('5') => app.tab = Tab::Community,
                        KeyCode::Char('6') => app.tab = Tab::Items,
                        KeyCode::Char('7') => app.tab = Tab::Leaderboard,
                        KeyCode::Char('8') => app.tab = Tab::Stats,
                        KeyCode::Char(']') if app.tab == Tab::Items => {
                            app.items_page += 1;
                            refresh_items(app);
                        }
                        KeyCode::Char('[') if app.tab == Tab::Items => {
                            if app.items_page > 1 {
                                app.items_page -= 1;
                                refresh_items(app);
                            }
                        }
                        KeyCode::Tab | KeyCode::Right => app.tab = step_tab(app.tab, 1),
                        KeyCode::Left => app.tab = step_tab(app.tab, -1),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn refresh_items(app: &mut App) {
    match get::<Catalog>(&app.cookies, &format!("items/catalog?page={}", app.items_page)) {
        Ok(v) => app.catalog = Some(v),
        Err(e) => app.errors.push(format!("items p{}: {e:#}", app.items_page)),
    }
}

fn step_tab(t: Tab, d: i32) -> Tab {
    let idx = TABS.iter().position(|(_, x)| *x == t).unwrap_or(0) as i32;
    let n = TABS.len() as i32;
    TABS[(((idx + d) % n + n) % n) as usize].1
}

fn refresh(app: &mut App) {
    app.errors.clear();
    match get::<Profile>(&app.cookies, "auth/profile") {
        Ok(v) => app.profile = Some(v),
        Err(e) => app.errors.push(format!("profile: {e:#}")),
    }
    match get::<Summary>(&app.cookies, "auth/game/account-summary") {
        Ok(v) => app.summary = Some(v),
        Err(e) => app.errors.push(format!("summary: {e:#}")),
    }
    match get::<Characters>(&app.cookies, "auth/game/characters") {
        Ok(v) => app.characters = Some(v),
        Err(e) => app.errors.push(format!("characters: {e:#}")),
    }
    match get::<Shop>(&app.cookies, "shop/status") {
        Ok(v) => app.shop = Some(v),
        Err(e) => app.errors.push(format!("shop: {e:#}")),
    }
    match get::<Launcher>(&app.cookies, "classicplus-launcher-info") {
        Ok(v) => app.launcher = Some(v),
        Err(e) => app.errors.push(format!("launcher: {e:#}")),
    }
    match get::<Session>(&app.cookies, "auth/session") {
        Ok(v) => app.session = Some(v),
        Err(e) => app.errors.push(format!("session: {e:#}")),
    }
    match get::<Discord>(&app.cookies, "discord-members") {
        Ok(v) => app.discord = Some(v),
        Err(e) => app.errors.push(format!("discord: {e:#}")),
    }
    match get::<Catalog>(&app.cookies, &format!("items/catalog?page={}", app.items_page)) {
        Ok(v) => app.catalog = Some(v),
        Err(e) => app.errors.push(format!("items: {e:#}")),
    }
    match get::<Rankings>(&app.cookies, "leaderboard/rankings") {
        Ok(v) => app.rankings = Some(v),
        Err(e) => app.errors.push(format!("rankings: {e:#}")),
    }
    match get::<Overview>(&app.cookies, "statistics/overview") {
        Ok(v) => app.overview = Some(v),
        Err(e) => app.errors.push(format!("stats: {e:#}")),
    }
    app.last = Instant::now();
}

fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()))
        .style(Style::new().fg(FG))
}

fn ui(f: &mut Frame, app: &App) {
    let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(1)]).split(f.area());
    header(f, rows[0], app);
    match app.tab {
        Tab::Account => tab_account(f, rows[1], app),
        Tab::Shop => tab_shop(f, rows[1], app),
        Tab::Characters => tab_characters(f, rows[1], app),
        Tab::Access => tab_access(f, rows[1], app),
        Tab::Community => tab_community(f, rows[1], app),
        Tab::Items => tab_items(f, rows[1], app),
        Tab::Leaderboard => tab_leaderboard(f, rows[1], app),
        Tab::Stats => tab_stats(f, rows[1], app),
    }
    footer(f, rows[2], app);
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let name = app
        .profile
        .as_ref()
        .map(|p| p.account.user_name.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "?".into());
    let id = app.profile.as_ref().map(|p| p.account.account_id).unwrap_or(0);
    let founder = app.shop.as_ref().map(|s| s.founder).unwrap_or(false);
    let mut spans = vec![
        Span::styled("TERA Classic+ ", Style::new().fg(ACCENT).bold()),
        Span::styled(format!("{name} "), Style::new().fg(FG).bold()),
        Span::styled(format!("#{id}", ), Style::new().fg(DIM)),
    ];
    if founder {
        spans.push(Span::styled("  FOUNDER", Style::new().fg(GOLD).bold()));
    }
    spans.push(Span::styled("     ", Style::new().fg(DIM)));
    for (label, t) in TABS {
        let sel = app.tab == t;
        spans.push(Span::styled(
            format!(" {label} "),
            if sel {
                Style::new().fg(Color::Black).bg(ACCENT).bold()
            } else {
                Style::new().fg(DIM)
            },
        ));
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).block(panel("Live")), area);
}

fn kv(k: &str, v: String, col: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {k:<22}"), Style::new().fg(DIM)),
        Span::styled(v, Style::new().fg(col)),
    ])
}

fn tab_account(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).split(area);
    let mut left = Vec::new();
    if let Some(p) = &app.profile {
        let a = &p.account;
        left.push(kv("Compte", format!("{} (#{})", a.user_name, a.account_id), FG));
        left.push(kv("Email", a.email.clone(), FG));
        left.push(kv("Langue", a.language.clone(), FG));
        left.push(kv("Inscrit le", short_date(&a.register_time), FG));
        left.push(kv("Dernier login", a.last_login_time.clone().map(|s| short_date(&s)).unwrap_or_else(|| "jamais".into()),
            if a.last_login_time.is_some() { GREEN } else { RED }));
        left.push(kv("Niveau max", a.highest_level.to_string(), if a.highest_level > 0 { GREEN } else { DIM }));
        left.push(kv("Parties jouees", a.play_count.to_string(), FG));
        left.push(kv("Temps de jeu", format!("{} h", a.play_time_total_hours), FG));
        left.push(kv("Consentement leaderboard", if a.leaderboard_consent { "oui".into() } else { "non".into() }, DIM));
    } else {
        left.push(Line::from(Span::styled("  (profil indisponible)", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(left).block(panel("Profil")), cols[0]);

    let mut right = Vec::new();
    if let Some(s) = &app.summary {
        right.push(kv("Personnages", s.character_count.to_string(), if s.character_count > 0 { GREEN } else { RED }));
        right.push(kv("TERA coins", group(s.tera_coins), GOLD));
        right.push(kv("Adventure coins", s.adventure_coins.map(group).unwrap_or_else(|| "-".into()), GOLD));
        right.push(kv("Or total", group(s.total_gold), GOLD));
        right.push(kv("Succes", s.total_achievements.to_string(), FG));
        right.push(kv("Temps (resume)", fmt_dur(s.total_playtime_sec), FG));
    }
    if let Some(sh) = &app.shop {
        right.push(Line::from(""));
        right.push(kv("Tokens", format!("{} {}", group(sh.token_balance), sh.currency), GOLD));
        right.push(kv("Slots perso", format!("{} / {}", sh.character_slots.current, sh.character_slots.maximum), FG));
        right.push(kv("VIP", vip_str(&sh.vip), if sh.vip.active { GREEN } else { DIM }));
        right.push(kv("VIP+", vip_str(&sh.vip_plus), if sh.vip_plus.active { GREEN } else { DIM }));
    }
    if app.summary.is_none() && app.shop.is_none() {
        right.push(Line::from(Span::styled("  (donnees indisponibles)", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(right).block(panel("Compte / Portefeuille")), cols[1]);
}

fn tab_shop(f: &mut Frame, area: Rect, app: &App) {
    let Some(sh) = &app.shop else {
        f.render_widget(Paragraph::new("  boutique indisponible").style(Style::new().fg(RED)).block(panel("Boutique")), area);
        return;
    };
    let split = Layout::vertical([Constraint::Length(6), Constraint::Min(3)]).split(area);
    let pm: Vec<String> = sh
        .payment_methods
        .iter()
        .filter(|m| m.available)
        .map(|m| m.id.replace("pp_", "").replace("_paypal", "").replace("_stripe", ""))
        .collect();
    let info = vec![
        kv("Mode / checkout", format!("{} / {}", sh.mode, sh.checkout_mode), if sh.mode == "connected" { GREEN } else { RED }),
        kv("Region boutique", format!("{} ({})", sh.store_region.name, sh.store_region.currency_code.to_uppercase()), FG),
        kv("Moyens de paiement", pm.join(", "), FG),
        kv("Transactions", sh.transactions.len().to_string(), if sh.transactions.is_empty() { DIM } else { GREEN }),
        kv("Source catalogue", sh.catalog_source.clone(), DIM),
    ];
    f.render_widget(Paragraph::new(info).block(panel("Boutique")), split[0]);
    let header = Row::new(vec!["id", "type", "quantite", "prix", "bonus"]).style(Style::new().fg(DIM).bold());
    let rows: Vec<Row> = sh
        .products
        .iter()
        .map(|p| {
            let qty = if let Some(a) = p.amount {
                group(a)
            } else if let Some(d) = p.days {
                format!("{d} j")
            } else if let Some(s) = p.slots {
                format!("{s} slot")
            } else {
                "-".into()
            };
            let bonus = p
                .bonus_percent
                .filter(|b| *b > 0)
                .map(|b| format!("+{b}%"))
                .or_else(|| p.saving_percent.filter(|s| *s > 0).map(|s| format!("-{s}%")))
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(p.id.clone()).style(Style::new().fg(FG)),
                Cell::from(p.kind.clone()).style(Style::new().fg(ACCENT)),
                Cell::from(qty).style(Style::new().fg(FG)),
                Cell::from(format!("{:.2} EUR", p.price_cents as f64 / 100.0)).style(Style::new().fg(GOLD)),
                Cell::from(bonus).style(Style::new().fg(GREEN)),
            ])
        })
        .collect();
    let widths = [Constraint::Length(20), Constraint::Length(16), Constraint::Length(12), Constraint::Length(12), Constraint::Length(8)];
    let title = format!("Produits \u{2022} {} tokens \u{2022} {}", group(sh.token_balance), sh.products.len());
    f.render_widget(Table::new(rows, widths).header(header).block(panel(&title)), split[1]);
}

fn tab_characters(f: &mut Frame, area: Rect, app: &App) {
    let Some(c) = &app.characters else {
        f.render_widget(Paragraph::new("  indisponible").style(Style::new().fg(RED)).block(panel("Personnages")), area);
        return;
    };
    if c.characters.is_empty() {
        let msg = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled("  Aucun personnage.", Style::new().fg(RED).bold())),
            Line::from(Span::styled("  Ce compte n'est jamais entre en jeu (connexion bloquee en amont).", Style::new().fg(DIM))),
            Line::from(Span::styled("  Le premier login creera ton premier perso.", Style::new().fg(DIM))),
        ])
        .block(panel("Personnages"));
        f.render_widget(msg, area);
        return;
    }
    let lines: Vec<Line> = c
        .characters
        .iter()
        .map(|v| Line::from(Span::styled(format!("  {v}"), Style::new().fg(FG))))
        .collect();
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("Personnages")), area);
}

fn tab_access(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let mut left = Vec::new();
    if let Some(s) = &app.session {
        let flag = |b: bool| if b { ("autorise", GREEN) } else { ("refuse", RED) };
        left.push(kv("Authentifie", if s.authenticated { "oui".into() } else { "non".into() }, if s.authenticated { GREEN } else { RED }));
        left.push(Line::from(""));
        for (name, v) in [
            ("Boutique", s.user.shop_access),
            ("Patch notes", s.user.patch_notes_access),
            ("Guides", s.user.guides_access),
            ("Statistiques", s.user.statistics_access),
            ("Archive", s.user.archive_access),
        ] {
            let (t, c) = flag(v);
            left.push(kv(name, t.into(), c));
        }
    } else {
        left.push(Line::from(Span::styled("  session indisponible", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(left).block(panel("Droits d'acces")), cols[0]);

    let mut right = Vec::new();
    if let Some(s) = &app.session {
        for p in &s.oauth_providers {
            right.push(Line::from(vec![
                Span::styled(if p.enabled { "\u{25cf} " } else { "\u{25cb} " }, Style::new().fg(if p.enabled { GREEN } else { DIM })),
                Span::styled(format!("{:<12}", p.label), Style::new().fg(FG)),
                Span::styled(if p.enabled { "lie" } else { "non lie" }, Style::new().fg(if p.enabled { GREEN } else { DIM })),
            ]));
        }
        if s.oauth_providers.is_empty() {
            right.push(Line::from(Span::styled("  (aucun provider)", Style::new().fg(DIM))));
        }
    }
    f.render_widget(Paragraph::new(right).block(panel("Connexions OAuth")), cols[1]);
}

fn tab_community(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let mut disc = Vec::new();
    if let Some(d) = &app.discord {
        disc.push(kv("Membres Discord", group(d.members), ACCENT));
        disc.push(kv("En ligne", group(d.online), GREEN));
    } else {
        disc.push(Line::from(Span::styled("  discord indisponible", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(disc).block(panel("Communaute")), cols[0]);

    let mut lines = Vec::new();
    if let Some(l) = &app.launcher {
        lines.push(kv("Disponible", if l.available { "oui".into() } else { "non".into() }, if l.available { GREEN } else { RED }));
        lines.push(kv("Version", l.version.clone(), ACCENT));
        lines.push(kv("Publie le", short_date(&l.published_at), FG));
        lines.push(kv("Taille", format!("{:.1} Mo", l.size_bytes as f64 / 1_048_576.0), FG));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Telechargement:", Style::new().fg(DIM))));
        lines.push(Line::from(Span::styled(format!("  {}", l.download_url), Style::new().fg(ACCENT))));
    } else {
        lines.push(Line::from(Span::styled("  launcher indisponible", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("Launcher officiel")), cols[1]);
}

fn rarity_col(r: &str) -> Color {
    match r {
        "common" => Color::Rgb(200, 200, 210),
        "uncommon" => GREEN,
        "rare" => ACCENT,
        "epic" => Color::Rgb(190, 120, 240),
        "legendary" | "mythic" => GOLD,
        _ => FG,
    }
}

fn tab_items(f: &mut Frame, area: Rect, app: &App) {
    let Some(c) = &app.catalog else {
        f.render_widget(Paragraph::new("  indisponible").style(Style::new().fg(RED)).block(panel("Items")), area);
        return;
    };
    let header = Row::new(vec!["id", "nom", "rarete", "categorie", "type", "niv"]).style(Style::new().fg(DIM).bold());
    let rows: Vec<Row> = c
        .items
        .iter()
        .map(|it| {
            Row::new(vec![
                Cell::from(it.id.to_string()).style(Style::new().fg(DIM)),
                Cell::from(it.name.clone()).style(Style::new().fg(rarity_col(&it.rarity)).bold()),
                Cell::from(it.rarity.clone()).style(Style::new().fg(rarity_col(&it.rarity))),
                Cell::from(it.category.clone()).style(Style::new().fg(FG)),
                Cell::from(it.combat_item_type.clone()).style(Style::new().fg(DIM)),
                Cell::from(it.level.to_string()).style(Style::new().fg(FG)),
            ])
        })
        .collect();
    let widths = [Constraint::Length(6), Constraint::Percentage(34), Constraint::Length(10), Constraint::Length(14), Constraint::Length(12), Constraint::Length(5)];
    let pages = if c.page_size > 0 { (c.total + c.page_size - 1) / c.page_size } else { 1 };
    let title = format!("Items \u{2022} {} au total \u{2022} page {}/{}  ([ ] pour naviguer)", group(c.total), c.page, pages.max(1));
    f.render_widget(Table::new(rows, widths).header(header).block(panel(&title)), area);
}

fn tab_leaderboard(f: &mut Frame, area: Rect, app: &App) {
    let Some(r) = &app.rankings else {
        f.render_widget(Paragraph::new("  indisponible").style(Style::new().fg(RED)).block(panel("Leaderboard")), area);
        return;
    };
    let header = Row::new(vec!["zone", "boss", "party DPS", "duree", "top joueur"]).style(Style::new().fg(DIM).bold());
    let rows: Vec<Row> = r
        .items
        .iter()
        .map(|e| {
            let top = e
                .players
                .iter()
                .max_by_key(|p| p.dps)
                .map(|p| format!("{} ({}) {}", p.name, p.class, fmt_big(p.dps)))
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(e.area_name.clone()).style(Style::new().fg(ACCENT)),
                Cell::from(e.boss_name.clone()).style(Style::new().fg(FG)),
                Cell::from(fmt_big(e.party_dps)).style(Style::new().fg(GOLD)),
                Cell::from(format!("{}s", e.fight_duration)).style(Style::new().fg(DIM)),
                Cell::from(top).style(Style::new().fg(GREEN)),
            ])
        })
        .collect();
    let widths = [Constraint::Percentage(22), Constraint::Percentage(20), Constraint::Length(12), Constraint::Length(8), Constraint::Percentage(34)];
    let title = format!("Leaderboard \u{2022} {} parses \u{2022} top {}", group(r.total), r.items.len());
    f.render_widget(Table::new(rows, widths).header(header).block(panel(&title)), area);
}

fn tab_stats(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).split(area);
    let mut left = Vec::new();
    if let Some(o) = &app.overview {
        left.push(kv("Realm", format!("{} ({})", o.realm, o.sample_window_label), ACCENT));
        left.push(Line::from(""));
        let t = &o.totals;
        left.push(kv("Comptes", group(t.accounts), FG));
        left.push(kv("Personnages", group(t.characters), FG));
        left.push(kv("Niveau 65", group(t.level65_count), GREEN));
        left.push(kv("Donjons clears", group(t.dungeon_clears), FG));
        left.push(kv("Parses combat", group(t.combat_parses), FG));
        left.push(kv("Degats totaux", fmt_big(t.total_damage), GOLD));
        left.push(kv("Heures jouees", group(t.hours_played), FG));
    } else {
        left.push(Line::from(Span::styled("  stats indisponibles", Style::new().fg(RED))));
    }
    f.render_widget(Paragraph::new(left).block(panel("Statistiques serveur")), cols[0]);

    let mut right = Vec::new();
    if let Some(o) = &app.overview {
        for rec in &o.records {
            right.push(Line::from(vec![
                Span::styled(format!("{:<26}", rec.kind), Style::new().fg(DIM)),
                Span::styled(format!("{} {}", rec.value, rec.unit), Style::new().fg(GOLD).bold()),
            ]));
            right.push(Line::from(Span::styled(format!("   {} · {}", rec.who, rec.class_name), Style::new().fg(GREEN))));
        }
        if o.records.is_empty() {
            right.push(Line::from(Span::styled("  (aucun record)", Style::new().fg(DIM))));
        }
    }
    f.render_widget(Paragraph::new(right).block(panel("Records")), cols[1]);
}

fn fmt_big(n: i64) -> String {
    let v = n as f64;
    if v >= 1e12 {
        format!("{:.2} T", v / 1e12)
    } else if v >= 1e9 {
        format!("{:.2} G", v / 1e9)
    } else if v >= 1e6 {
        format!("{:.2} M", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1} k", v / 1e3)
    } else {
        n.to_string()
    }
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let key = |k: &'static str| Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(DIM));
    let mut spans = vec![
        key("q"),
        Span::styled(" quitter  ", Style::new().fg(DIM)),
        key("r"),
        Span::styled(" refresh  ", Style::new().fg(DIM)),
        key("1-8/Tab"),
        Span::styled(" onglets  ", Style::new().fg(DIM)),
        key("[ ]"),
        Span::styled(" page items  ", Style::new().fg(DIM)),
        Span::styled(format!("maj il y a {}s", app.last.elapsed().as_secs()), Style::new().fg(DIM)),
    ];
    if !app.errors.is_empty() {
        spans.push(Span::styled(format!("  \u{26a0} {} erreur(s) (cookies expires? r)", app.errors.len()), Style::new().fg(RED)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn vip_str(v: &Vip) -> String {
    if v.active {
        format!("actif{}", v.expires_at.as_ref().map(|e| format!(" jusqu'au {}", short_date(e))).unwrap_or_default())
    } else {
        "inactif".into()
    }
}

fn short_date(iso: &str) -> String {
    iso.split(['T', ' ']).next().unwrap_or(iso).to_string()
}

fn fmt_dur(s: i64) -> String {
    if s <= 0 {
        return "0".into();
    }
    let h = s / 3600;
    let m = (s % 3600) / 60;
    format!("{h}h {m}m")
}

fn group(n: i64) -> String {
    let neg = n < 0;
    let d = n.abs().to_string();
    let len = d.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in d.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push('\u{202f}');
        }
        out.push(c);
    }
    if neg {
        format!("-{out}")
    } else {
        out
    }
}
