use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph, Wrap};
use ratatui::DefaultTerminal;
use serde::Deserialize;
use std::time::{Duration, Instant};

const ENDPOINT: &str = "https://stats.tera-europe-classic.com/live-data.php";
const REFRESH: Duration = Duration::from_secs(30);

const ACCENT: Color = Color::Rgb(96, 200, 250);
const GREEN: Color = Color::Rgb(90, 220, 130);
const RED: Color = Color::Rgb(244, 96, 96);
const YELLOW: Color = Color::Rgb(244, 202, 96);
const DIM: Color = Color::Rgb(130, 130, 150);
const FG: Color = Color::Rgb(226, 226, 236);

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Data {
    generated_at: String,
    range: String,
    source: String,
    health: Health,
    current: Current,
    insights: Insights,
    series: Vec<Point>,
    activity: Activity,
    heatmap: Vec<Vec<i64>>,
    network: Network,
    attacks: Attacks,
    release: Release,
    diagnostics: Diagnostics,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Release {
    starts_at: String,
    stats_start_at: String,
    state: String,
    phase: String,
    telemetry_visible: bool,
    seconds_remaining: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Diagnostics {
    collector: String,
    storage: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Health {
    state: String,
    age_seconds: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Current {
    players: i64,
    delta1h: i64,
    typical: i64,
    forecast: i64,
    anomaly: bool,
    maintenance: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Insights {
    peak: i64,
    average: i64,
    low: i64,
    movement: i64,
    stability: i64,
}

#[derive(Deserialize, Default)]
struct Point {
    #[allow(dead_code)]
    ts: i64,
    players: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Activity {
    peak_day: i64,
    peak_hour: i64,
    peak_value: i64,
    quiet_day: i64,
    quiet_hour: i64,
    quiet_value: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Network {
    status: String,
    load: String,
    impact: String,
    summary: String,
    uptime: String,
    locations: Vec<Loc>,
    counts: Counts,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Counts {
    active_incidents: i64,
    active_maintenance: i64,
    scheduled_maintenance: i64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Loc {
    title: String,
    place: String,
    status: String,
    load_percent: i64,
    uptime: String,
    latency_ms: Option<f64>,
    rx_mbps: Option<f64>,
    tx_mbps: Option<f64>,
    throughput_mbps: Option<f64>,
    last_observed: String,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Attacks {
    provider: String,
    servers: Vec<AtkServer>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AtkServer {
    label: String,
    status: String,
    active_attacks: i64,
    total_attacks: i64,
    last30_days: i64,
    current_incident: Option<Incident>,
    analytics: AtkAnalytics,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct AtkAnalytics {
    max_bits_per_second: i64,
    max_packets_per_second: i64,
    ended_attacks: i64,
    total_duration_seconds: i64,
    longest_duration_seconds: i64,
    carpet_bombs: i64,
    average_mitigation_seconds: Option<f64>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Incident {
    id: String,
    status: String,
    last_event: String,
    detected_at: String,
    mitigated_at: Option<String>,
    updated_at: String,
    vector: String,
    peak_bits_per_second: i64,
    peak_packets_per_second: i64,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Dashboard,
    Details,
}

struct App {
    data: Option<Data>,
    err: Option<String>,
    last: Instant,
    range: &'static str,
    view: View,
    scroll: u16,
}

fn fetch(range: &str) -> Result<Data> {
    let mut resp = ureq::get(ENDPOINT)
        .query("range", range)
        .header("User-Agent", "tera-stats-tui")
        .call()
        .context("requete HTTP")?;
    let body = resp.body_mut().read_to_string().context("lecture reponse")?;
    serde_json::from_str(&body).context("parsing JSON")
}

fn main() -> Result<()> {
    let mut term = ratatui::init();
    let res = run(&mut term);
    ratatui::restore();
    res
}

fn run(term: &mut DefaultTerminal) -> Result<()> {
    let mut app = App {
        data: None,
        err: None,
        last: Instant::now(),
        range: "24h",
        view: View::Dashboard,
        scroll: 0,
    };
    refresh(&mut app);
    loop {
        if app.last.elapsed() >= REFRESH {
            refresh(&mut app);
        }
        term.draw(|f| ui(f, &app))?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('r') => refresh(&mut app),
                        KeyCode::Char('1') => set_range(&mut app, "24h"),
                        KeyCode::Char('2') => set_range(&mut app, "7d"),
                        KeyCode::Char('3') => set_range(&mut app, "30d"),
                        KeyCode::Char('d') | KeyCode::Tab => {
                            app.view = if app.view == View::Dashboard { View::Details } else { View::Dashboard };
                            app.scroll = 0;
                        }
                        KeyCode::Down | KeyCode::Char('j') => app.scroll = app.scroll.saturating_add(1),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll = app.scroll.saturating_sub(1),
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(())
}

fn set_range(app: &mut App, range: &'static str) {
    if app.range != range {
        app.range = range;
        refresh(app);
    }
}

fn refresh(app: &mut App) {
    match fetch(app.range) {
        Ok(d) => {
            app.data = Some(d);
            app.err = None;
        }
        Err(e) => app.err = Some(format!("{e:#}")),
    }
    app.last = Instant::now();
}

fn ui(f: &mut Frame, app: &App) {
    let area = f.area();
    let Some(d) = &app.data else {
        let msg = match &app.err {
            Some(e) => format!("Erreur de chargement:\n{e}\n\n[r] reessayer   [q] quitter"),
            None => "Chargement...".into(),
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::new().fg(RED))
                .block(panel("TERA Classic+ Live")),
            area,
        );
        return;
    };

    if app.view == View::Details {
        let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(1)]).split(area);
        header(f, rows[0], d, app);
        details(f, rows[1], d, app);
        footer(f, rows[2], app);
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Min(8),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Length(1),
    ])
    .split(area);

    header(f, rows[0], d, app);
    top(f, rows[1], d);
    chart(f, rows[2], d);
    let mid = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[3]);
    network(f, mid[0], d);
    attacks(f, mid[1], d);
    heatmap(f, rows[4], d);
    footer(f, rows[5], app);
}

fn details(f: &mut Frame, area: Rect, d: &Data, app: &App) {
    let mut l: Vec<Line> = Vec::new();
    let sec = |s: &str| Line::from(Span::styled(format!("\u{25b8} {s}"), Style::new().fg(ACCENT).bold()));
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("   {k:<22}"), Style::new().fg(DIM)),
            Span::styled(v, Style::new().fg(FG)),
        ])
    };

    let r = &d.release;
    l.push(sec("Release"));
    l.push(kv("phase", format!("{} / {}", r.phase, r.state)));
    l.push(kv("lancement", r.starts_at.clone()));
    l.push(kv("stats depuis", r.stats_start_at.clone()));
    l.push(kv("telemetrie visible", yesno(r.telemetry_visible)));
    if r.seconds_remaining > 0 {
        l.push(kv("compte a rebours", fmt_dur(r.seconds_remaining)));
    }
    l.push(kv("collector / storage", format!("{} / {}", d.diagnostics.collector, d.diagnostics.storage)));
    l.push(kv("generatedAt", d.generated_at.clone()));
    l.push(Line::from(""));

    let c = &d.current;
    let i = &d.insights;
    l.push(sec("Joueurs"));
    l.push(kv("actuel", group(c.players)));
    l.push(kv("delta 1h", group(c.delta1h)));
    l.push(kv("typique / prevu", format!("{} / {}", group(c.typical), group(c.forecast))));
    l.push(kv("pic / moyenne / bas", format!("{} / {} / {}", group(i.peak), group(i.average), group(i.low))));
    l.push(kv("stabilite / tendance", format!("{} % / {}", i.stability, group(i.movement))));
    l.push(kv("anomalie / maintenance", format!("{} / {}", yesno(c.anomaly), yesno(c.maintenance))));
    let a = &d.activity;
    let days = ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"];
    l.push(kv("pic affluence", format!("{} {}h = {}", days.get(a.peak_day as usize).copied().unwrap_or("?"), a.peak_hour, group(a.peak_value))));
    l.push(kv("creux affluence", format!("{} {}h = {}", days.get(a.quiet_day as usize).copied().unwrap_or("?"), a.quiet_hour, group(a.quiet_value))));
    l.push(Line::from(""));

    let net = &d.network;
    l.push(sec("Reseau"));
    l.push(kv("statut / charge / impact", format!("{} / {} / {}", net.status, net.load, net.impact)));
    l.push(kv("uptime global", net.uptime.clone()));
    l.push(kv("incidents actifs", net.counts.active_incidents.to_string()));
    l.push(kv("maintenance active/prev", format!("{} / {}", net.counts.active_maintenance, net.counts.scheduled_maintenance)));
    l.push(kv("resume", net.summary.clone()));
    for loc in &net.locations {
        l.push(Line::from(Span::styled(format!("   \u{2022} {} ({})", loc.title, loc.place), Style::new().fg(status_col(&loc.status)).bold())));
        l.push(kv("   statut / charge", format!("{} / {} %", loc.status, loc.load_percent)));
        l.push(kv("   uptime", loc.uptime.clone()));
        l.push(kv("   latence", loc.latency_ms.map(|v| format!("{v:.0} ms")).unwrap_or_else(|| "-".into())));
        l.push(kv("   debit rx/tx/total", format!("{} / {} / {}", optf(loc.rx_mbps, "Mbps"), optf(loc.tx_mbps, "Mbps"), optf(loc.throughput_mbps, "Mbps"))));
        l.push(kv("   observe", loc.last_observed.clone()));
        if !loc.message.is_empty() {
            l.push(kv("   note", loc.message.clone()));
        }
    }
    l.push(Line::from(""));

    let at = &d.attacks;
    l.push(sec("Attaques DDoS"));
    l.push(kv("fournisseur", at.provider.clone()));
    let an = at.servers.first().map(|s| &s.analytics);
    if let Some(an) = an {
        l.push(kv("record debit", fmt_bps(an.max_bits_per_second)));
        l.push(kv("record paquets", fmt_pps(an.max_packets_per_second)));
        l.push(kv("duree totale", fmt_dur(an.total_duration_seconds)));
        l.push(kv("plus longue", fmt_dur(an.longest_duration_seconds)));
        l.push(kv("terminees / carpet", format!("{} / {}", an.ended_attacks, an.carpet_bombs)));
        l.push(kv("mitigation moyenne", an.average_mitigation_seconds.map(|v| format!("{v:.0} s")).unwrap_or_else(|| "-".into())));
    }
    for s in &at.servers {
        l.push(Line::from(Span::styled(format!("   \u{2022} {} [{}]", s.label, s.status), Style::new().fg(if s.status.eq_ignore_ascii_case("under-attack") { RED } else { GREEN }).bold())));
        l.push(kv("   actives/total/30j", format!("{} / {} / {}", s.active_attacks, s.total_attacks, s.last30_days)));
        if let Some(inc) = &s.current_incident {
            l.push(kv("   incident", format!("{} [{}]", inc.id, inc.status)));
            l.push(kv("   vecteur", inc.vector.clone()));
            l.push(kv("   detecte", inc.detected_at.clone()));
            l.push(kv("   maj / event", format!("{} / {}", inc.updated_at, inc.last_event)));
            l.push(kv("   mitige", inc.mitigated_at.clone().unwrap_or_else(|| "non".into())));
            l.push(kv("   pic incident", format!("{} / {}", fmt_bps(inc.peak_bits_per_second), fmt_pps(inc.peak_packets_per_second))));
        }
    }

    let total = l.len() as u16;
    let max_scroll = total.saturating_sub(area.height.saturating_sub(2));
    let scroll = app.scroll.min(max_scroll);
    f.render_widget(
        Paragraph::new(l)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .block(panel(&format!("Details bruts \u{2022} {}  ({}/{})", pretty(&d.range), scroll, max_scroll))),
        area,
    );
}

fn yesno(b: bool) -> String {
    if b { "oui".into() } else { "non".into() }
}

fn optf(v: Option<f64>, unit: &str) -> String {
    v.map(|x| format!("{x:.1} {unit}")).unwrap_or_else(|| "-".into())
}

fn fmt_dur(s: i64) -> String {
    if s <= 0 {
        return "-".into();
    }
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}h {m}m {sec}s")
    } else if m > 0 {
        format!("{m}m {sec}s")
    } else {
        format!("{sec}s")
    }
}


fn panel(title: &str) -> Block<'static> {
    Block::bordered()
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(format!(" {title} "), Style::new().fg(ACCENT).bold()))
        .style(Style::new().fg(FG))
}

fn header(f: &mut Frame, area: Rect, d: &Data, app: &App) {
    let stale = app.last.elapsed().as_secs();
    let live = d.health.state.eq_ignore_ascii_case("live");
    let dot = Span::styled("\u{25cf} ", Style::new().fg(if live { GREEN } else { RED }));
    let line = Line::from(vec![
        Span::styled("TERA Classic+", Style::new().fg(ACCENT).bold()),
        Span::styled("  \u{2502}  ", Style::new().fg(DIM)),
        dot,
        Span::styled(d.health.state.to_uppercase(), Style::new().fg(if live { GREEN } else { RED }).bold()),
        Span::styled("   plage ", Style::new().fg(DIM)),
        Span::styled(pretty(&d.range), Style::new().fg(ACCENT).bold()),
        Span::styled(format!("   source {}", d.source), Style::new().fg(DIM)),
        Span::styled(format!("   donnees +{}s", d.health.age_seconds), Style::new().fg(DIM)),
        Span::styled(format!("   maj il y a {stale}s", ), Style::new().fg(DIM)),
        Span::styled(if app.view == View::Details { "   [DETAILS]" } else { "" }, Style::new().fg(YELLOW).bold()),
    ]);
    f.render_widget(Paragraph::new(line).block(panel("Live")), area);
}

fn top(f: &mut Frame, area: Rect, d: &Data) {
    let cols = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).split(area);

    let c = &d.current;
    let (arrow, dcol) = delta_style(c.delta1h);
    let mut lines = vec![
        Line::from(Span::styled("JOUEURS EN LIGNE", Style::new().fg(DIM).bold())),
        Line::from(Span::styled(group(c.players), Style::new().fg(ACCENT).bold().add_modifier(Modifier::REVERSED))),
        Line::from(vec![
            Span::styled(format!("{arrow} {}", group(c.delta1h.abs())), Style::new().fg(dcol).bold()),
            Span::styled(" / 1h", Style::new().fg(DIM)),
            Span::styled(format!("    typique {}", group(c.typical)), Style::new().fg(DIM)),
            Span::styled(format!("    prevu {}", group(c.forecast)), Style::new().fg(DIM)),
        ]),
    ];
    let mut flags = vec![];
    if c.anomaly {
        flags.push(Span::styled(" ANOMALIE ", Style::new().fg(Color::Black).bg(YELLOW).bold()));
        flags.push(Span::raw(" "));
    }
    if c.maintenance {
        flags.push(Span::styled(" MAINTENANCE ", Style::new().fg(Color::Black).bg(RED).bold()));
    }
    if flags.is_empty() {
        flags.push(Span::styled(" nominal ", Style::new().fg(GREEN)));
    }
    lines.push(Line::from(flags));
    f.render_widget(Paragraph::new(lines).block(panel("Maintenant")), cols[0]);

    let i = &d.insights;
    let (mv_arrow, mv_col) = delta_style(i.movement);
    let grid = vec![
        stat_line("Pic 24h", group(i.peak), YELLOW),
        stat_line("Moyenne", group(i.average), FG),
        stat_line("Plancher", group(i.low), DIM),
        stat_line("Stabilite", format!("{} %", i.stability), stability_col(i.stability)),
        Line::from(vec![
            Span::styled(format!("{:<12}", "Tendance"), Style::new().fg(DIM)),
            Span::styled(format!("{mv_arrow} {}", group(i.movement.abs())), Style::new().fg(mv_col).bold()),
        ]),
    ];
    f.render_widget(Paragraph::new(grid).block(panel("Analyse")), cols[1]);
}

fn stat_line(label: &str, value: String, col: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<12}"), Style::new().fg(DIM)),
        Span::styled(value, Style::new().fg(col).bold()),
    ])
}

fn chart(f: &mut Frame, area: Rect, d: &Data) {
    if d.series.is_empty() {
        f.render_widget(Paragraph::new("pas de serie").block(panel("Joueurs")), area);
        return;
    }
    let pts: Vec<(f64, f64)> = d.series.iter().enumerate().map(|(i, p)| (i as f64, p.players as f64)).collect();
    let n = pts.len() as f64;
    let maxy = d.series.iter().map(|p| p.players).max().unwrap_or(1).max(1) as f64;
    let miny = d.series.iter().map(|p| p.players).min().unwrap_or(0) as f64;
    let ds = Dataset::default()
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(ACCENT))
        .data(&pts);
    let chart = Chart::new(vec![ds])
        .block(panel(&format!("Joueurs \u{2022} {}", pretty(&d.range))))
        .x_axis(Axis::default().style(Style::new().fg(DIM)).bounds([0.0, (n - 1.0).max(1.0)]))
        .y_axis(
            Axis::default()
                .style(Style::new().fg(DIM))
                .labels([
                    Span::styled(group(miny as i64), Style::new().fg(DIM)),
                    Span::styled(group(((miny + maxy) / 2.0) as i64), Style::new().fg(DIM)),
                    Span::styled(group(maxy as i64), Style::new().fg(DIM)),
                ])
                .bounds([miny * 0.98, maxy * 1.02]),
        );
    f.render_widget(chart, area);
}

fn network(f: &mut Frame, area: Rect, d: &Data) {
    let net = &d.network;
    let scol = status_col(&net.status);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(scol)),
            Span::styled(net.status.to_uppercase(), Style::new().fg(scol).bold()),
            Span::styled(format!("   charge {}", net.load), Style::new().fg(DIM)),
            Span::styled(format!("   uptime {}", net.uptime), Style::new().fg(DIM)),
        ]),
        Line::from(Span::styled(net.impact.clone(), Style::new().fg(YELLOW))),
        Line::from(Span::styled(net.summary.clone(), Style::new().fg(DIM))),
        Line::from(""),
    ];
    for l in &net.locations {
        let lc = status_col(&l.status);
        lines.push(Line::from(vec![
            Span::styled("\u{25cf} ", Style::new().fg(lc)),
            Span::styled(format!("{:<18}", l.title), Style::new().fg(FG)),
            Span::styled(format!("{:<8}", l.status), Style::new().fg(lc)),
            Span::styled(format!("{:>3}%  ", l.load_percent), Style::new().fg(load_col(l.load_percent))),
            Span::styled(format!("up {}", l.uptime), Style::new().fg(DIM)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("Reseau")), area);
}

fn attacks(f: &mut Frame, area: Rect, d: &Data) {
    let a = &d.attacks;
    let mut lines = vec![Line::from(Span::styled(
        format!("fournisseur mitigation: {}", a.provider),
        Style::new().fg(DIM),
    ))];
    for s in &a.servers {
        let under = s.status.eq_ignore_ascii_case("under-attack");
        let scol = if under { RED } else { GREEN };
        lines.push(Line::from(vec![
            Span::styled(format!("{}  ", s.label), Style::new().fg(FG).bold()),
            Span::styled(
                if under { " SOUS ATTAQUE " } else { " ok " }.to_string(),
                Style::new().fg(Color::Black).bg(scol).bold(),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(format!("actives {}", s.active_attacks), Style::new().fg(if s.active_attacks > 0 { RED } else { GREEN }).bold()),
            Span::styled(format!("   total {}", s.total_attacks), Style::new().fg(DIM)),
            Span::styled(format!("   30j {}", s.last30_days), Style::new().fg(DIM)),
        ]));
        if let Some(inc) = &s.current_incident {
            lines.push(Line::from(vec![
                Span::styled("vecteur ", Style::new().fg(DIM)),
                Span::styled(inc.vector.clone(), Style::new().fg(YELLOW).bold()),
                Span::styled(format!("   pic {} / {}", fmt_bps(inc.peak_bits_per_second), fmt_pps(inc.peak_packets_per_second)), Style::new().fg(DIM)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("record ", Style::new().fg(DIM)),
            Span::styled(fmt_bps(s.analytics.max_bits_per_second), Style::new().fg(RED)),
            Span::styled(" / ", Style::new().fg(DIM)),
            Span::styled(fmt_pps(s.analytics.max_packets_per_second), Style::new().fg(RED)),
            Span::styled(format!("   termine {}", s.analytics.ended_attacks), Style::new().fg(DIM)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(panel("Attaques DDoS")), area);
}

fn heatmap(f: &mut Frame, area: Rect, d: &Data) {
    let days = ["Lun", "Mar", "Mer", "Jeu", "Ven", "Sam", "Dim"];
    let max = d.heatmap.iter().flatten().copied().max().unwrap_or(1).max(1) as f64;
    let mut lines = Vec::new();
    let mut hdr = vec![Span::styled("     ", Style::new().fg(DIM))];
    for h in (0..24).step_by(3) {
        hdr.push(Span::styled(format!("{h:<2}   "), Style::new().fg(DIM)));
    }
    lines.push(Line::from(hdr));
    for (di, row) in d.heatmap.iter().enumerate().take(7) {
        let mut spans = vec![Span::styled(format!("{:<5}", days.get(di).copied().unwrap_or("?")), Style::new().fg(DIM))];
        for &v in row.iter().take(24) {
            let t = v as f64 / max;
            spans.push(Span::styled("\u{2588}\u{2588}", Style::new().fg(heat_col(t))));
        }
        lines.push(Line::from(spans));
    }
    let a = &d.activity;
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("pic ", Style::new().fg(DIM)),
        Span::styled(format!("{} {}h ({})", days.get(a.peak_day as usize).copied().unwrap_or("?"), a.peak_hour, group(a.peak_value)), Style::new().fg(YELLOW)),
        Span::styled("     creux ", Style::new().fg(DIM)),
        Span::styled(format!("{} {}h ({})", days.get(a.quiet_day as usize).copied().unwrap_or("?"), a.quiet_hour, group(a.quiet_value)), Style::new().fg(ACCENT)),
    ]));
    f.render_widget(Paragraph::new(lines).block(panel("Affluence 7j x 24h")), area);
}

fn footer(f: &mut Frame, area: Rect, app: &App) {
    let err = app.err.as_ref().map(|e| format!("  \u{26a0} {e}")).unwrap_or_default();
    let key = |k: &'static str| Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(DIM));
    let line = Line::from(vec![
        key("q"),
        Span::styled(" quitter  ", Style::new().fg(DIM)),
        key("r"),
        Span::styled(" refresh  ", Style::new().fg(DIM)),
        key("1/2/3"),
        Span::styled(" 24h/7j/30j  ", Style::new().fg(DIM)),
        key("d"),
        Span::styled(" details  ", Style::new().fg(DIM)),
        key("j/k"),
        Span::styled(" defiler  ", Style::new().fg(DIM)),
        Span::styled(format!("auto {}s", REFRESH.as_secs()), Style::new().fg(DIM)),
        Span::styled(err, Style::new().fg(RED)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn delta_style(v: i64) -> (&'static str, Color) {
    if v > 0 {
        ("\u{25b2}", GREEN)
    } else if v < 0 {
        ("\u{25bc}", RED)
    } else {
        ("\u{25ac}", DIM)
    }
}

fn stability_col(v: i64) -> Color {
    if v >= 70 {
        GREEN
    } else if v >= 40 {
        YELLOW
    } else {
        RED
    }
}

fn load_col(p: i64) -> Color {
    if p >= 80 {
        RED
    } else if p >= 50 {
        YELLOW
    } else {
        GREEN
    }
}

fn status_col(s: &str) -> Color {
    match s.to_lowercase().as_str() {
        "online" | "live" | "operational" => GREEN,
        "degraded" | "medium" | "partial" => YELLOW,
        "offline" | "down" | "outage" => RED,
        _ => DIM,
    }
}

fn heat_col(t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let stops = [
        (0.0, (24, 28, 48)),
        (0.35, (32, 96, 140)),
        (0.65, (96, 200, 250)),
        (0.85, (244, 202, 96)),
        (1.0, (244, 96, 96)),
    ];
    for w in stops.windows(2) {
        let (t0, c0) = w[0];
        let (t1, c1) = w[1];
        if t <= t1 {
            let k = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            return Color::Rgb(
                lerp(c0.0, c1.0, k),
                lerp(c0.1, c1.1, k),
                lerp(c0.2, c1.2, k),
            );
        }
    }
    Color::Rgb(244, 96, 96)
}

fn lerp(a: i32, b: i32, k: f64) -> u8 {
    (a as f64 + (b - a) as f64 * k).round().clamp(0.0, 255.0) as u8
}

fn group(n: i64) -> String {
    let neg = n < 0;
    let digits = n.abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    for (i, c) in digits.chars().enumerate() {
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

fn fmt_bps(bits: i64) -> String {
    let b = bits as f64;
    if b >= 1e9 {
        format!("{:.1} Gbps", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} Mbps", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.1} Kbps", b / 1e3)
    } else {
        format!("{bits} bps")
    }
}

fn fmt_pps(p: i64) -> String {
    let v = p as f64;
    if v >= 1e6 {
        format!("{:.1} Mpps", v / 1e6)
    } else if v >= 1e3 {
        format!("{:.1} Kpps", v / 1e3)
    } else {
        format!("{p} pps")
    }
}

fn pretty(s: &str) -> String {
    match s {
        "24h" => "24 h".into(),
        "7d" => "7 j".into(),
        other => other.to_string(),
    }
}
