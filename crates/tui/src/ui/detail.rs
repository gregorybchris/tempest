//! The selected transfer's detail, split below the list (D-137): a tab bar on the rule above it,
//! and the tab's content.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use tempest_api::inspect::{FileInfo, PeerInfo};
use tempest_api::{Kind, Status, TransferInfo};

use super::transfers::{fix_hint, summary_color};
use super::{fit, pad, status_color};
use crate::app::{App, Fetched, Tab};
use crate::explain::{connected, explain};
use crate::format;
use crate::theme::Theme;

/// The width of the summary's labels.
const LABEL: usize = 13;

/// The tab bar, drawn over the rule `area` above the detail.
pub(super) fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut spans = vec![Span::raw(" ")];
    for (index, tab) in Tab::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled("│", Style::new().fg(theme.line)));
        }
        let style = if tab == app.tab {
            Style::new()
                .fg(theme.bg)
                .bg(theme.down)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.dim).bg(theme.bg)
        };
        spans.push(Span::styled(format!(" {} ", tab.title()), style));
    }
    spans.push(Span::raw(" "));
    let line = Line::from(spans);
    let width = (line.width() as u16).min(area.width.saturating_sub(1));
    let tabs = Rect {
        x: area.x + 1,
        width,
        ..area
    };
    frame.render_widget(Paragraph::new(line), tabs);
}

/// The selected tab's content.
pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Some(transfer) = app.selected_transfer() else {
        return;
    };
    let area = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    match app.tab {
        Tab::Summary => draw_summary(frame, app, transfer, area),
        Tab::Files => draw_files(frame, app, transfer, area),
        Tab::Peers => draw_peers(frame, app, area),
        Tab::Decisions => draw_decisions(frame, app, transfer, area),
    }
}

/// A line in the middle of `area`, for a tab with nothing to show.
fn note(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    let [middle] = Layout::vertical([Constraint::Length(1)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            fit(text, usize::from(area.width)),
            Style::new().fg(color),
        ))
        .centered(),
        middle,
    );
}

/// What was fetched, or draws why there's nothing to show yet and returns `None`.
fn fetched<'a, T>(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    fetched: Option<&'a Fetched<T>>,
) -> Option<&'a T> {
    match fetched.and_then(Option::as_ref) {
        Some(Ok(value)) => Some(value),
        Some(Err(error)) => {
            note(frame, area, error, theme.bad);
            None
        }
        None => {
            note(frame, area, "Loading…", theme.dim);
            None
        }
    }
}

fn draw_summary(frame: &mut Frame, app: &App, transfer: &TransferInfo, area: Rect) {
    let theme = &app.theme;
    let label = |text: &str| Span::styled(format!("{text:<LABEL$}"), Style::new().fg(theme.dim));
    let value = |text: String| Span::styled(text, Style::new().fg(theme.fg));
    let row = |name: &str, text: String| Line::from(vec![label(name), value(text)]);

    let explanation = explain(transfer);
    let detail = explanation
        .detail
        .map(|detail| Line::styled(detail, Style::new().fg(theme.dim)));
    let mut state = vec![Span::styled(
        explanation.text,
        Style::new().fg(summary_color(theme, &transfer.status)),
    )];
    if let Some(fix) = explanation.fix {
        state.push(Span::styled(
            format!(" {}", fix_hint(fix)),
            Style::new().fg(theme.dim),
        ));
    }

    let progress = match transfer.size {
        None => "Waiting for the bundle's description.".to_owned(),
        Some(size) => format!(
            "{} of {} · {} of {} chunks",
            format::percent(transfer.progress),
            format::bytes(size),
            format::thousands(transfer.have),
            format::thousands(transfer.chunks)
        ),
    };
    let rates = Line::from(vec![
        label("Speed"),
        Span::styled("↓ ", Style::new().fg(theme.dim)),
        Span::styled(
            per_second(transfer.download_rate),
            Style::new().fg(theme.down),
        ),
        Span::styled("  ↑ ", Style::new().fg(theme.dim)),
        Span::styled(per_second(transfer.upload_rate), Style::new().fg(theme.up)),
    ]);
    let ratio = match transfer.ratio {
        Some(ratio) => format!(
            "{ratio:.2} · sent {}, received {}",
            format::bytes(transfer.uploaded),
            format::bytes(transfer.downloaded)
        ),
        None => "–".to_owned(),
    };
    let peers = match transfer.connections {
        0 if transfer.peers == 0 => "None found yet".to_owned(),
        0 => format!("None connected of {} known", transfer.peers),
        _ => connected(transfer),
    };
    let tags = match transfer.tags.is_empty() {
        true => "–".to_owned(),
        false => (transfer.tags.iter())
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join(" "),
    };
    let kind = match transfer.kind {
        Kind::Download => "Download",
        Kind::Share => "Share",
    };
    let progress = format!("{kind}: {progress}");
    let mut lines = vec![Line::from(state)];
    lines.extend(detail);
    lines.extend([
        Line::raw(""),
        row("Progress", progress),
        rates,
        row(
            "Time left",
            transfer
                .eta
                .map_or_else(|| "–".to_owned(), format::duration_words),
        ),
        row("Ratio", ratio),
        row("Peers", peers),
    ]);
    if let Some(content) = &transfer.content_dir {
        lines.push(row("Files in", content.display().to_string()));
    }
    lines.extend([
        row("Save folder", transfer.save_folder.display().to_string()),
        row(
            "Category",
            transfer.category.clone().unwrap_or_else(|| "–".to_owned()),
        ),
        row("Tags", tags),
    ]);
    if !transfer.published.is_empty() {
        let trackers: Vec<&str> = transfer.published.iter().map(String::as_str).collect();
        lines.push(row("Published to", trackers.join(", ")));
    }
    lines.push(row("Link", transfer.link.clone()));
    let scroll = u16::try_from(app.scroll).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

/// A rate with its unit, or a dash for none.
fn per_second(rate: u64) -> String {
    match rate {
        0 => "–".to_owned(),
        rate => format!("{}/s", format::bytes(rate)),
    }
}

/// The rows from `scroll` that fit `height`. Scrolled past the end, the last rows still fill
/// the height rather than leaving one row on screen.
fn window<T>(rows: &[T], scroll: usize, height: usize) -> &[T] {
    let start = scroll.min(rows.len().saturating_sub(height.max(1)));
    &rows[start..rows.len().min(start + height)]
}

fn draw_files(frame: &mut Frame, app: &App, transfer: &TransferInfo, area: Rect) {
    let theme = &app.theme;
    if transfer.size.is_none() {
        let text = "The files show once the bundle's description arrives.";
        return note(frame, area, text, theme.dim);
    }
    let inside = app.inside().map(|inside| &inside.files);
    let Some(files) = fetched(frame, theme, area, inside) else {
        return;
    };
    if files.is_empty() {
        return note(frame, area, "The bundle has no files.", theme.dim);
    }
    const SIZE: usize = 5;
    const BAR: usize = 10;
    let name_width = usize::from(area.width).saturating_sub(SIZE + BAR + 5 + 4);
    let header = Line::styled(
        format!(
            "{:<name_width$}  {:>SIZE$}  {}",
            format!("{} files", files.len()),
            "Size",
            "Progress"
        ),
        Style::new().fg(theme.dim),
    );
    let color = status_color(theme, &transfer.status);
    let row = |file: &FileInfo| {
        let filled = ((file.progress.clamp(0.0, 1.0) * BAR as f64).floor() as usize).min(BAR);
        Line::from(vec![
            Span::styled(
                format!("{}  ", pad(&file.path, name_width)),
                Style::new().fg(theme.fg),
            ),
            Span::styled(
                format!("{:>SIZE$}  ", format::bytes(file.size)),
                Style::new().fg(theme.dim),
            ),
            Span::styled("█".repeat(filled), Style::new().fg(color)),
            Span::styled("░".repeat(BAR - filled), Style::new().fg(theme.cell_off)),
            Span::styled(
                format!(" {:>4}", format::percent(file.progress)),
                Style::new().fg(theme.dim),
            ),
        ])
    };
    let height = usize::from(area.height.saturating_sub(1));
    let mut lines = vec![header];
    lines.extend(window(files, app.scroll, height).iter().map(row));
    frame.render_widget(Paragraph::new(lines), area);
}

/// Want or Serving in both directions: `↓` toward this peer, `↑` toward the other.
fn both_ways(toward_us: bool, toward_them: bool) -> &'static str {
    match (toward_us, toward_them) {
        (true, true) => "↓↑",
        (true, false) => "↓",
        (false, true) => "↑",
        (false, false) => "–",
    }
}

fn draw_peers(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let running = app.selected_transfer().is_some_and(|t| {
        matches!(
            t.status,
            Status::Downloading | Status::Sharing | Status::Stalled { .. }
        )
    });
    let inside = app.inside().map(|inside| &inside.peers);
    let Some(peers) = fetched(frame, theme, area, inside) else {
        return;
    };
    if peers.is_empty() {
        let text = match running {
            true => "Not connected to any peers yet.",
            false => "Not connected to anyone: peers show while it runs.",
        };
        return note(frame, area, text, theme.dim);
    }
    let mut peers: Vec<&PeerInfo> = peers.iter().collect();
    peers.sort_by_key(|p| std::cmp::Reverse(p.download_rate + p.upload_rate));

    // Peer, path, has, want, serving, ↓ and ↑, then the address in what is left.
    const WIDTHS: [usize; 7] = [8, 7, 4, 4, 7, 5, 5];
    let fixed: usize = WIDTHS.iter().sum::<usize>() + 2 * WIDTHS.len();
    let address_width = usize::from(area.width).saturating_sub(fixed).max(7);
    let columns = |cells: [String; 8]| -> String {
        let [peer, address, path, has, want, serving, down, up] = cells;
        format!(
            "{peer:<8}  {address}  {path:<7}  {has:>4}  {want:<4}  {serving:<7}  {down:>5}  {up:>5}",
            address = pad(&address, address_width)
        )
    };
    let header = Line::styled(
        columns(
            [
                "Peer", "Address", "Path", "Has", "Want", "Serving", "↓", "↑",
            ]
            .map(str::to_owned),
        ),
        Style::new().fg(theme.dim),
    );
    let row = |peer: &&PeerInfo| {
        let text = columns([
            peer.peer_id.chars().take(8).collect(),
            peer.address.clone(),
            peer.path.clone(),
            peer.progress
                .map_or_else(|| "–".to_owned(), format::percent),
            both_ways(peer.we_want, peer.they_want).to_owned(),
            both_ways(peer.serving_us, peer.we_serve).to_owned(),
            format::rate(peer.download_rate),
            format::rate(peer.upload_rate),
        ]);
        let color = match (peer.open, peer.path.as_str()) {
            (false, _) => theme.faint,
            (true, "relayed") => theme.warn,
            (true, _) => theme.fg,
        };
        Line::styled(text, Style::new().fg(color))
    };
    let [table, legend] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
    let height = usize::from(table.height.saturating_sub(1));
    let mut lines = vec![header];
    lines.extend(window(&peers, app.scroll, height).iter().map(row));
    frame.render_widget(Paragraph::new(lines), table);
    let text = "Want and Serving: ↓ toward us, ↑ toward them.";
    frame.render_widget(
        Paragraph::new(Line::styled(
            fit(text, usize::from(legend.width)),
            Style::new().fg(theme.dim),
        )),
        legend,
    );
}

fn draw_decisions(frame: &mut Frame, app: &App, transfer: &TransferInfo, area: Rect) {
    let theme = &app.theme;
    let Some(decisions) = app
        .model
        .decisions(&transfer.id)
        .filter(|d| !d.events.is_empty())
    else {
        let text = "No decisions yet. They show here as the transfer runs.";
        return note(frame, area, text, theme.dim);
    };
    let mut heading = vec![Span::styled(
        format::count(decisions.events.len(), "event"),
        Style::new().fg(theme.dim),
    )];
    if decisions.missed > 0 {
        heading.push(Span::styled(
            format!(" · {} missed: they came too fast", decisions.missed),
            Style::new().fg(theme.warn),
        ));
    }
    const KIND: usize = 14;
    let height = usize::from(area.height.saturating_sub(1));
    // Newest at the bottom; scrolling counts back from it.
    let end = decisions.events.len().saturating_sub(app.scroll);
    let start = end.saturating_sub(height);
    let text_width = usize::from(area.width).saturating_sub(10 + KIND + 2);
    let mut lines = vec![Line::from(heading)];
    lines.extend(decisions.events.range(start..end).map(|event| {
        Line::from(vec![
            Span::styled(
                format!("{:>9} ", format::clock(event.time)),
                Style::new().fg(theme.dim),
            ),
            Span::styled(
                format!("{:<KIND$}  ", fit(&event.kind, KIND)),
                Style::new().fg(theme.hot),
            ),
            Span::styled(fit(&event.text, text_width), Style::new().fg(theme.fg)),
        ])
    }));
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn want_and_serving_read_as_arrows() {
        assert_eq!(both_ways(true, true), "↓↑");
        assert_eq!(both_ways(false, true), "↑");
        assert_eq!(both_ways(false, false), "–");
    }

    #[test]
    fn windows_stay_within_the_rows() {
        let rows = [1, 2, 3, 4, 5];
        assert_eq!(window(&rows, 0, 2), [1, 2]);
        assert_eq!(window(&rows, 3, 2), [4, 5]);
        assert_eq!(window(&rows, 4, 2), [4, 5]);
        assert_eq!(window(&rows, 9, 2), [4, 5]);
        assert_eq!(window(&rows, 9, 0), [] as [i32; 0]);
        assert!(window::<u8>(&[], 3, 2).is_empty());
    }
}
