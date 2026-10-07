//! Drawing. Reads `App`; never mutates it.

use proto_core::{format, Bundle, Path, GRID_COLS, GRID_ROWS};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Axis, Block, BorderType, Cell, Chart, Clear, Dataset, GraphType, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::app::{fake_files, rate_or_dash, App, Command, Mode, SortKey, Status, Tab, Transfer};
use crate::theme::Theme;

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn draw(f: &mut Frame, app: &App) {
    let th = app.theme;
    let area = f.area();
    f.render_widget(Block::default().style(Style::new().bg(th.bg).fg(th.fg)), area);

    let [header, body, footer] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(10), Constraint::Length(1)]).areas(area);
    draw_header(f, app, header);
    draw_footer(f, app, footer);

    let [side, main] = Layout::horizontal([Constraint::Length(24), Constraint::Min(40)]).areas(body);
    draw_sidebar(f, app, side);

    let rows = app.visible.len() as u16;
    let [list, detail] =
        Layout::vertical([Constraint::Length((rows + 3).clamp(6, 16)), Constraint::Min(12)]).areas(main);
    draw_transfers(f, app, list);
    draw_detail(f, app, detail);

    draw_popup(f, app, area);
}

fn block<'a>(th: &Theme, title: Line<'a>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(th.line))
        .title(title)
}

fn title<'a>(th: &Theme, text: &'a str) -> Line<'a> {
    Line::from(Span::styled(format!(" {text} "), Style::new().fg(th.fg).add_modifier(Modifier::BOLD)))
}

// ---------- Header ----------

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let down: Vec<f64> = history(app, |s| s.down_bps);
    let up: Vec<f64> = history(app, |s| s.up_bps);
    let peers: usize = app.transfers.iter().map(|t| app.bundle(t).peers.len()).sum();
    let left = Line::from(vec![
        Span::styled(" T E M P E S T ", Style::new().fg(th.bg).bg(th.down).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled("↓ ", Style::new().fg(th.dim)),
        Span::styled(format!("{:<11}", format::rate(app.total_down())), Style::new().fg(th.down).add_modifier(Modifier::BOLD)),
        Span::styled(sparkline(&down, 24), Style::new().fg(th.down)),
        Span::raw("   "),
        Span::styled("↑ ", Style::new().fg(th.dim)),
        Span::styled(format!("{:<11}", format::rate(app.total_up())), Style::new().fg(th.up).add_modifier(Modifier::BOLD)),
        Span::styled(sparkline(&up, 24), Style::new().fg(th.up)),
        Span::raw("   "),
        Span::styled(format!("{peers}"), Style::new().fg(th.fg).add_modifier(Modifier::BOLD)),
        Span::styled(" peers", Style::new().fg(th.dim)),
    ]);
    let right = Line::from(vec![
        Span::styled("sort ", Style::new().fg(th.dim)),
        Span::styled(
            format!("{} {}", app.sort.label(), if app.descending { "↓" } else { "↑" }),
            Style::new().fg(th.fg),
        ),
        Span::styled("   theme ", Style::new().fg(th.dim)),
        Span::styled(format!("{} ", th.name), Style::new().fg(th.fg)),
    ])
    .right_aligned();
    let [line, _] = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    f.render_widget(Paragraph::new(left), line);
    f.render_widget(Paragraph::new(right), line);
}

fn history(app: &App, f: impl Fn(&proto_core::Sample) -> f64) -> Vec<f64> {
    let active: Vec<&Bundle> = app.transfers.iter().filter(|t| app.down_bps(t) + app.up_bps(t) > 0.0).map(|t| app.bundle(t)).collect();
    let len = active.iter().map(|b| b.history.len()).max().unwrap_or(0);
    (len.saturating_sub(48)..len).map(|i| active.iter().filter_map(|b| b.history.get(i)).map(&f).sum()).collect()
}

/// A text sparkline of the last `width` values, each pair of samples averaged.
fn sparkline(values: &[f64], width: usize) -> String {
    let pairs: Vec<f64> = values.chunks(2).map(|c| c.iter().sum::<f64>() / c.len() as f64).collect();
    let tail = &pairs[pairs.len().saturating_sub(width)..];
    // Scale between the window's min and max, so small changes stay visible.
    let max = tail.iter().copied().fold(f64::MIN, f64::max);
    let min = tail.iter().copied().fold(f64::MAX, f64::min) * 0.9;
    let span = (max - min).max(1.0);
    tail.iter().map(|v| SPARK[(((v - min) / span) * 7.0).round().clamp(0.0, 7.0) as usize]).collect()
}

// ---------- Sidebar ----------

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let mut lines = Vec::new();
    let mut options: Vec<(Option<Status>, &str)> = vec![(None, "all")];
    options.extend(Status::ALL.iter().map(|s| (Some(*s), s.label())));
    for (i, (status, label)) in options.into_iter().enumerate() {
        let count = app.count(status);
        let selected = app.status_filter == status;
        let color = status.map_or(th.fg, |s| status_color(th, s));
        let style = if selected {
            Style::new().fg(th.bg).bg(color).add_modifier(Modifier::BOLD)
        } else if count == 0 {
            Style::new().fg(th.faint)
        } else {
            Style::new().fg(color)
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", i), Style::new().fg(th.faint)),
            Span::styled(format!("{label:<12}{count:>3} "), style),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(" tags", Style::new().fg(th.dim)));
    let mut tags: Vec<(String, usize)> = Vec::new();
    for t in &app.transfers {
        for tag in &t.tags {
            match tags.iter_mut().find(|(n, _)| n == tag) {
                Some((_, c)) => *c += 1,
                None => tags.push((tag.clone(), 1)),
            }
        }
    }
    tags.sort();
    for (tag, count) in tags {
        let active = !app.filter.is_empty() && tag.contains(&app.filter.to_lowercase());
        let style = if active { Style::new().fg(th.hot) } else { Style::new().fg(th.dim) };
        lines.push(Line::styled(format!("   #{tag:<14}{count:>3}"), style));
    }
    f.render_widget(Paragraph::new(lines).block(block(th, title(th, "status"))), area);
}

pub fn status_color(th: &Theme, s: Status) -> Color {
    match s {
        Status::Downloading => th.down,
        Status::Sharing => th.up,
        Status::Stalled => th.warn,
        Status::Queued => th.dim,
        Status::Stopped => th.faint,
        Status::Completed => th.fg,
        Status::Errored => th.bad,
    }
}

// ---------- Transfers ----------

fn draw_transfers(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let sort_mark = |k: SortKey, label: &'static str| -> Cell<'static> {
        if app.sort == k {
            Cell::from(format!("{label}{}", if app.descending { "↓" } else { "↑" })).style(Style::new().fg(th.fg))
        } else {
            Cell::from(label).style(Style::new().fg(th.dim))
        }
    };
    let header = Row::new(vec![
        sort_mark(SortKey::Name, "name"),
        sort_mark(SortKey::Size, "size"),
        sort_mark(SortKey::Progress, "progress"),
        sort_mark(SortKey::Status, "status"),
        sort_mark(SortKey::Down, "↓"),
        sort_mark(SortKey::Up, "↑"),
        sort_mark(SortKey::Peers, "peers"),
        sort_mark(SortKey::Eta, "left"),
        sort_mark(SortKey::Ratio, "ratio"),
    ])
    .bottom_margin(0);

    let inner_rows = area.height.saturating_sub(3) as usize;
    let offset = app.cursor.saturating_sub(inner_rows.saturating_sub(1));
    let rows: Vec<Row> = app
        .visible
        .iter()
        .enumerate()
        .skip(offset)
        .take(inner_rows)
        .map(|(pos, &i)| transfer_row(app, &app.transfers[i], pos == app.cursor))
        .collect();

    let empty = rows.is_empty();
    let filter_title = if app.filter.is_empty() && app.status_filter.is_none() {
        format!(" transfers {} ", app.visible.len())
    } else {
        format!(
            " transfers {} of {}{}{} ",
            app.visible.len(),
            app.transfers.len(),
            app.status_filter.map(|s| format!(" · {}", s.label())).unwrap_or_default(),
            if app.filter.is_empty() { String::new() } else { format!(" · /{}", app.filter) },
        )
    };
    let table = Table::new(
        rows,
        [
            Constraint::Min(24),
            Constraint::Length(9),
            Constraint::Length(19),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(5),
        ],
    )
    .header(header)
    .column_spacing(2)
    .block(block(th, Line::styled(filter_title, Style::new().fg(th.fg).add_modifier(Modifier::BOLD))));
    f.render_widget(table, area);
    if empty {
        let msg = Paragraph::new(Line::styled("No transfers match. Press esc to clear the filter.", Style::new().fg(th.dim)))
            .centered();
        f.render_widget(msg, area.inner(ratatui::layout::Margin::new(2, 2)));
    }
}

fn transfer_row<'a>(app: &App, t: &'a Transfer, selected: bool) -> Row<'a> {
    let th = &app.theme;
    let b = app.bundle(t);
    let status = app.status(t);
    let color = status_color(th, status);
    let mut name = vec![Span::styled(t.name.clone(), Style::new().fg(th.fg))];
    for tag in &t.tags {
        name.push(Span::styled(format!(" #{tag}"), Style::new().fg(th.faint)));
    }
    let eta = match status {
        Status::Downloading => b.eta_secs().map(format::duration).unwrap_or_else(|| "∞".into()),
        _ => "·".into(),
    };
    let row = Row::new(vec![
        Cell::from(Line::from(name)),
        Cell::from(format::bytes(b.size_bytes)).style(Style::new().fg(th.dim)),
        Cell::from(progress_bar(th, b.progress, color, 12)),
        Cell::from(status.label()).style(Style::new().fg(color)),
        Cell::from(rate_or_dash(app.down_bps(t))).style(Style::new().fg(th.down)),
        Cell::from(rate_or_dash(app.up_bps(t))).style(Style::new().fg(th.up)),
        Cell::from(format!("{}/{}", app.serving_us(t), b.peers.len())).style(Style::new().fg(th.dim)),
        Cell::from(eta).style(Style::new().fg(th.dim)),
        Cell::from(format!("{:.2}", b.ratio())).style(Style::new().fg(th.dim)),
    ]);
    if selected {
        row.style(Style::new().bg(Theme::mix(th.bg, th.down, 0.16)).add_modifier(Modifier::BOLD))
    } else {
        row
    }
}

fn progress_bar(th: &Theme, p: f64, color: Color, width: usize) -> Line<'static> {
    const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let filled = p.clamp(0.0, 1.0) * width as f64;
    let full = filled.floor() as usize;
    let mut bar: String = "█".repeat(full);
    if full < width {
        bar.push(EIGHTHS[((filled - full as f64) * 8.0) as usize]);
        bar.push_str(&" ".repeat(width - full - 1));
    }
    Line::from(vec![
        Span::styled(bar, Style::new().fg(color).bg(th.cell_off)),
        Span::styled(format!(" {:>5.1}%", p * 100.0), Style::new().fg(th.dim)),
    ])
}

// ---------- Detail ----------

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let mut tabs: Vec<Span> = vec![Span::raw(" ")];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        if i > 0 {
            tabs.push(Span::styled(" │ ", Style::new().fg(th.line)));
        }
        let style = if *tab == app.tab {
            Style::new().fg(th.bg).bg(th.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.dim)
        };
        tabs.push(Span::styled(format!(" {} ", tab.label()), style));
    }
    tabs.push(Span::raw(" "));
    let outer = block(th, Line::from(tabs));
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let Some(t) = app.selected() else {
        f.render_widget(Paragraph::new(Line::styled("Nothing selected.", Style::new().fg(th.dim))), inner);
        return;
    };
    let b = app.bundle(t);
    match app.tab {
        Tab::Overview => draw_overview(f, app, t, b, inner),
        Tab::Peers => draw_peers(f, th, b, inner),
        Tab::Files => draw_files(f, th, t, b, inner),
        Tab::Decisions => draw_decisions(f, th, b, inner),
    }
}

fn draw_overview(f: &mut Frame, app: &App, t: &Transfer, b: &Bundle, area: Rect) {
    let th = &app.theme;
    let [info, visuals] = Layout::horizontal([Constraint::Length(46), Constraint::Min(40)]).spacing(2).areas(area);
    let status = app.status(t);
    let explain = match (&t.error, status) {
        (Some(e), _) => e.clone(),
        (_, Status::Stopped) => "Stopped. Press space to start.".into(),
        (_, Status::Completed) => "Complete and stopped. Press space to share it again.".into(),
        (_, Status::Queued) => "Queued: waiting for a free download slot. F force-starts it.".into(),
        _ => b.explanation(),
    };
    let kv = |k: &str, v: String| {
        Line::from(vec![Span::styled(format!("{k:<11}"), Style::new().fg(th.dim)), Span::styled(v, Style::new().fg(th.fg))])
    };
    let mut lines = vec![
        Line::styled(t.name.clone(), Style::new().fg(th.fg).add_modifier(Modifier::BOLD)),
        Line::styled(explain, Style::new().fg(status_color(th, status))),
        Line::raw(""),
        kv("progress", format!("{} of {}", format::percent(b.progress), format::bytes(b.size_bytes))),
        kv("download", rate_or_dash(app.down_bps(t))),
        kv("upload", rate_or_dash(app.up_bps(t))),
        kv("time left", b.eta_secs().filter(|_| status == Status::Downloading).map(format::duration).unwrap_or_else(|| "·".into())),
        kv("ratio", format!("{:.2}", b.ratio())),
        kv("peers", format!("{} serving us of {} connected", app.serving_us(t), b.peers.len())),
        kv("files", format::count(b.files as u64)),
        kv("saved to", t.save_path.clone()),
        kv("bundle id", format::short_id(&b.id).to_string()),
    ];
    if !t.tags.is_empty() {
        lines.push(kv("tags", t.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), info);

    let map_rows = (GRID_ROWS / 2) as u16;
    let [map_title, map, _, chart] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(map_rows), Constraint::Length(1), Constraint::Min(6)]).areas(visuals);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("chunks ", Style::new().fg(th.fg).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!("{} chunks · █ have  █ downloading  ░ missing (dimmer = rarer)", format::count(b.chunk_count())),
                Style::new().fg(th.dim),
            ),
        ])),
        map_title,
    );
    draw_chunk_map(f, th, b, map);
    draw_chart(f, th, b, chart);
}

/// Two chunk-map rows per terminal row, using the upper half block.
fn draw_chunk_map(f: &mut Frame, th: &Theme, b: &Bundle, area: Rect) {
    let width = area.width.min(GRID_COLS as u16 * 2).max(1);
    let buf = f.buffer_mut();
    for y in 0..area.height.min((GRID_ROWS / 2) as u16) {
        for x in 0..width {
            let col = x as usize * GRID_COLS / width as usize;
            let top = cell_color(th, b, (y as usize * 2) * GRID_COLS + col);
            let bottom = cell_color(th, b, (y as usize * 2 + 1) * GRID_COLS + col);
            if let Some(c) = buf.cell_mut((area.x + x, area.y + y)) {
                c.set_char('▀').set_fg(top).set_bg(bottom);
            }
        }
    }
}

fn cell_color(th: &Theme, b: &Bundle, i: usize) -> Color {
    let Some(c) = b.cells.get(i) else { return th.bg };
    if c.in_flight {
        th.hot
    } else if c.have >= 1.0 {
        th.down
    } else if c.have > 0.0 {
        Theme::mix(th.cell_off, th.down, c.have * 0.7)
    } else if c.availability == 0 {
        Theme::mix(th.cell_off, th.bad, 0.4)
    } else {
        Theme::mix(th.cell_off, th.faint, c.availability as f32 / 12.0)
    }
}

fn draw_chart(f: &mut Frame, th: &Theme, b: &Bundle, area: Rect) {
    let down: Vec<(f64, f64)> = b.history.iter().map(|s| (s.t_ms as f64 / 1000.0, s.down_bps)).collect();
    let up: Vec<(f64, f64)> = b.history.iter().map(|s| (s.t_ms as f64 / 1000.0, s.up_bps)).collect();
    let (t0, t1) = (down.first().map_or(0.0, |p| p.0), down.last().map_or(1.0, |p| p.0));
    let peak = down.iter().chain(&up).map(|p| p.1).fold(1_000_000.0, f64::max) * 1.1;
    let chart = Chart::new(vec![
        Dataset::default().name("download").marker(Marker::Braille).graph_type(GraphType::Line).style(Style::new().fg(th.down)).data(&down),
        Dataset::default().name("upload").marker(Marker::Braille).graph_type(GraphType::Line).style(Style::new().fg(th.up)).data(&up),
    ])
    .style(Style::new().bg(th.bg))
    .legend_position(None)
    .x_axis(
        Axis::default()
            .bounds([t0, t1])
            .style(Style::new().fg(th.line))
            .labels(vec![
                Span::styled("-10m", Style::new().fg(th.dim)),
                Span::styled("-5m", Style::new().fg(th.dim)),
                Span::styled("now", Style::new().fg(th.dim)),
            ]),
    )
    .y_axis(
        Axis::default()
            .bounds([0.0, peak])
            .style(Style::new().fg(th.line))
            .labels(vec![
                Span::styled("0", Style::new().fg(th.dim)),
                Span::styled(format::rate(peak / 2.0), Style::new().fg(th.dim)),
                Span::styled(format::rate(peak), Style::new().fg(th.dim)),
            ]),
    );
    f.render_widget(chart, area);
}

fn draw_peers(f: &mut Frame, th: &Theme, b: &Bundle, area: Rect) {
    if b.peers.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("Not connected to any peers.", Style::new().fg(th.dim))), area);
        return;
    }
    let header = Row::new(["peer", "address", "path", "has", "wants ours", "we serve", "client", "↓ from them", "↑ to them"])
        .style(Style::new().fg(th.dim));
    let rows = b.peers.iter().map(|p| {
        Row::new(vec![
            Cell::from(format::short_id(&p.id).to_string()),
            Cell::from(p.addr.clone()).style(Style::new().fg(th.dim)),
            Cell::from(p.path.label()).style(Style::new().fg(if p.path == Path::Relayed { th.warn } else { th.fg })),
            Cell::from(format::percent(p.progress)).style(Style::new().fg(th.dim)),
            Cell::from(if p.wants_from_us { "yes" } else { "no" }).style(Style::new().fg(if p.wants_from_us { th.fg } else { th.faint })),
            Cell::from(if p.newcomer_slot { "newcomer" } else if p.we_serve { "yes" } else { "no" })
                .style(Style::new().fg(if p.we_serve { th.up } else { th.faint })),
            Cell::from(p.client.clone()).style(Style::new().fg(th.dim)),
            Cell::from(rate_or_dash(p.down_bps)).style(Style::new().fg(th.down)),
            Cell::from(rate_or_dash(p.up_bps)).style(Style::new().fg(th.up)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(30),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(11),
        ],
    )
    .header(header)
    .column_spacing(2);
    f.render_widget(table, area);
}

fn draw_files(f: &mut Frame, th: &Theme, t: &Transfer, b: &Bundle, area: Rect) {
    let files = fake_files(b, &t.name);
    let header = Row::new(["file", "size", "progress", "priority"]).style(Style::new().fg(th.dim));
    let rows = files.iter().take(area.height as usize).map(|(name, size, p)| {
        Row::new(vec![
            Cell::from(name.clone()),
            Cell::from(format::bytes(*size)).style(Style::new().fg(th.dim)),
            Cell::from(progress_bar(th, *p, th.down, 10)),
            Cell::from("normal").style(Style::new().fg(th.dim)),
        ])
    });
    let table = Table::new(rows, [Constraint::Min(30), Constraint::Length(9), Constraint::Length(18), Constraint::Length(9)])
        .header(header)
        .column_spacing(2);
    f.render_widget(table, area);
}

fn draw_decisions(f: &mut Frame, th: &Theme, b: &Bundle, area: Rect) {
    if b.decisions.is_empty() {
        f.render_widget(Paragraph::new(Line::styled("Nothing to decide until a peer is found.", Style::new().fg(th.dim))), area);
        return;
    }
    let lines: Vec<Line> = b
        .decisions
        .iter()
        .take(area.height as usize)
        .map(|d| {
            let kind_color = match d.kind.label() {
                "request" => th.down,
                "serve" => th.up,
                "stop" => th.bad,
                "endgame" => th.hot,
                _ => th.dim,
            };
            Line::from(vec![
                Span::styled(format!("{} ", format::clock(d.t_ms)), Style::new().fg(th.faint)),
                Span::styled(format!("{:<8}", d.kind.label()), Style::new().fg(kind_color)),
                Span::styled(d.what.clone(), Style::new().fg(th.fg)),
                Span::styled(format!("  — {}", d.why), Style::new().fg(th.dim)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

// ---------- Footer ----------

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let key = |k: &'static str| Span::styled(format!(" {k} "), Style::new().fg(th.bg).bg(th.dim));
    let label = |l: &'static str| Span::styled(format!(" {l}  "), Style::new().fg(th.dim));
    let line = match &app.mode {
        Mode::Filter => Line::from(vec![
            Span::styled(" filter /", Style::new().fg(th.hot)),
            Span::styled(app.filter.clone(), Style::new().fg(th.fg)),
            Span::styled("▏", Style::new().fg(th.hot)),
            Span::styled("   enter keep · esc clear", Style::new().fg(th.dim)),
        ]),
        _ => {
            if let Some((msg, _)) = &app.toast {
                Line::from(Span::styled(format!(" {msg}"), Style::new().fg(th.hot)))
            } else {
                Line::from(vec![
                    key("j/k"), label("move"),
                    key("space"), label("start/stop"),
                    key("/"), label("filter"),
                    key("s"), label("sort"),
                    key("0-7"), label("status"),
                    key("tab"), label("details"),
                    key(":"), label("commands"),
                    key("?"), label("help"),
                    key("q"), label("quit"),
                ])
            }
        }
    };
    f.render_widget(Paragraph::new(line), area);
}

// ---------- Popups ----------

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [v] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
    let [h] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(v);
    h
}

fn popup_block<'a>(th: &Theme, text: &'a str) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(th.down))
        .style(Style::new().bg(th.raise).fg(th.fg))
        .title(title(th, text))
}

fn draw_popup(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    match &app.mode {
        Mode::Rename(text) => input(f, th, area, "rename", "New name", text),
        Mode::Tags(text) => input(f, th, area, "tags", "Comma-separated tags", text),
        Mode::Add(text) => input(f, th, area, "add bundle", "Paste a tempest: link", text),
        Mode::ConfirmRemove => {
            let name = app.selected().map(|t| t.name.as_str()).unwrap_or("");
            let r = centered(area, 64, 7);
            f.render_widget(Clear, r);
            let text = Text::from(vec![
                Line::from(format!("Remove {name}?")),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(" r ", Style::new().fg(th.bg).bg(th.fg)),
                    Span::raw(" remove, keep files    "),
                    Span::styled(" d ", Style::new().fg(th.bg).bg(th.bad)),
                    Span::raw(" remove and delete files    "),
                    Span::styled(" esc ", Style::new().fg(th.bg).bg(th.dim)),
                    Span::raw(" cancel"),
                ]),
            ]);
            f.render_widget(Paragraph::new(text).block(popup_block(th, "remove")).wrap(Wrap { trim: true }), r);
        }
        Mode::Palette { query, selected } => {
            let matches = App::palette_matches(query);
            let r = centered(area, 60, (matches.len() as u16 + 4).clamp(6, 20));
            f.render_widget(Clear, r);
            let mut lines = vec![Line::from(vec![
                Span::styled(":", Style::new().fg(th.hot)),
                Span::raw(query.clone()),
                Span::styled("▏", Style::new().fg(th.hot)),
            ])];
            lines.push(Line::raw(""));
            for (i, c) in matches.iter().enumerate() {
                let style = if i == *selected { Style::new().fg(th.bg).bg(th.down) } else { Style::new().fg(th.fg) };
                lines.push(Line::from(vec![
                    Span::styled(format!(" {:<44}", c.label()), style),
                    Span::styled(format!("{:>8} ", c.key()), Style::new().fg(th.dim)),
                ]));
            }
            f.render_widget(Paragraph::new(lines).block(popup_block(th, "commands")), r);
        }
        Mode::Help => {
            let r = centered(area, 60, Command::ALL.len() as u16 + 6);
            f.render_widget(Clear, r);
            let mut lines: Vec<Line> = Command::ALL
                .iter()
                .map(|c| {
                    Line::from(vec![
                        Span::styled(format!(" {:>7}  ", c.key()), Style::new().fg(th.hot)),
                        Span::raw(c.label()),
                    ])
                })
                .collect();
            lines.push(Line::raw(""));
            lines.push(Line::styled(" j/k or ↑/↓ move · g/G top/bottom · 0–7 status · esc close", Style::new().fg(th.dim)));
            f.render_widget(Paragraph::new(lines).block(popup_block(th, "keys")), r);
        }
        Mode::Normal | Mode::Filter => {}
    }
}

fn input(f: &mut Frame, th: &Theme, area: Rect, heading: &str, prompt: &str, text: &str) {
    let r = centered(area, 64, 6);
    f.render_widget(Clear, r);
    let lines = vec![
        Line::styled(prompt.to_string(), Style::new().fg(th.dim)),
        Line::from(vec![Span::raw(text.to_string()), Span::styled("▏", Style::new().fg(th.hot))]),
        Line::raw(""),
        Line::styled("enter save · esc cancel", Style::new().fg(th.dim)),
    ];
    f.render_widget(Paragraph::new(lines).block(popup_block(th, heading)), r);
}
