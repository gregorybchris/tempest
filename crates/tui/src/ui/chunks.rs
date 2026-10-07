//! The selected transfer's chunk map, full screen: every chunk as part of a cell, shaded by
//! whether it is stored, being downloaded or missing, and missing ones by how rare they are.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use tempest_engine::inspect::ChunkMapInfo;

use super::fit;
use crate::app::App;
use crate::chunkmap::{self, COMMON, Shade};
use crate::format;
use crate::theme::Theme;

/// The color a cell is drawn in.
fn color(theme: &Theme, shade: Shade) -> Color {
    let off = theme.cell_off;
    match shade {
        Shade::Empty => theme.bg,
        Shade::Have => theme.down,
        Shade::Partial(share) => Theme::mix(off, theme.down, 0.3 + 0.5 * share),
        Shade::InFlight => theme.hot,
        Shade::Missing(holders) => Theme::mix(off, theme.dim, holders as f32 / COMMON as f32),
        Shade::Nowhere => Theme::mix(off, theme.bad, 0.7),
        Shade::Unknown => Theme::mix(off, theme.faint, 0.5),
    }
}

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let Some(transfer) = app.selected_transfer() else {
        return;
    };
    let [title, legend, _, map] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);

    let fetched = app
        .inside()
        .and_then(|inside| inside.chunks.as_ref())
        .and_then(|chunks| chunks.as_ref().ok());
    let mut heading = vec![Span::styled(
        format!(" {}", transfer.name),
        Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
    )];
    if let Some(chunks) = fetched.filter(|map| map.chunks > 0) {
        let per_cell = f64::from(chunks.chunks) / chunks.cells.len().max(1) as f64;
        let scale = match per_cell <= 1.0 {
            true => "one cell per chunk".to_owned(),
            false => format!("about {per_cell:.0} chunks per cell"),
        };
        heading.push(Span::styled(
            format!(
                "  {}, {} stored · {scale}",
                match chunks.chunks {
                    1 => "1 chunk".to_owned(),
                    n => format!("{} chunks", format::thousands(u64::from(n))),
                },
                format::percent(transfer.progress)
            ),
            Style::new().fg(theme.dim),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(heading)), title);
    let running = fetched.is_none_or(|map| map.running);
    frame.render_widget(Paragraph::new(legend_line(theme, running)), legend);

    let message = match app.inside().and_then(|inside| inside.chunks.as_ref()) {
        None => Some(("Loading the chunk map…".to_owned(), theme.dim)),
        Some(Err(error)) => Some((error.clone(), theme.bad)),
        Some(Ok(map)) if map.chunks == 0 => Some((
            "The chunk map shows once the bundle's description arrives.".to_owned(),
            theme.dim,
        )),
        Some(Ok(_)) => None,
    };
    match (message, fetched) {
        (Some((text, color)), _) => {
            let [middle] = Layout::vertical([Constraint::Length(1)])
                .flex(Flex::Center)
                .areas(map);
            let line = Line::styled(fit(&text, usize::from(map.width)), Style::new().fg(color));
            frame.render_widget(Paragraph::new(line).centered(), middle);
        }
        (None, Some(chunks)) => draw_map(frame, theme, chunks, map),
        (None, None) => {}
    }
}

/// What each color means. Rarity is known only while the transfer runs.
fn legend_line(theme: &Theme, running: bool) -> Line<'static> {
    let block = |shade| Span::styled("█ ", Style::new().fg(color(theme, shade)));
    let text = |text: &'static str| Span::styled(text, Style::new().fg(theme.dim));
    let mut spans = vec![Span::raw(" "), block(Shade::Have), text("stored  ")];
    if running {
        spans.extend([
            block(Shade::InFlight),
            text("downloading  "),
            block(Shade::Missing(1)),
            block(Shade::Missing(COMMON)),
            text("missing, dimmer is rarer  "),
            block(Shade::Nowhere),
            text("no peer has it"),
        ]);
    } else {
        spans.extend([
            block(Shade::Unknown),
            text("missing · Not running, so how rare each chunk is isn't known."),
        ]);
    }
    Line::from(spans)
}

/// Two cells per terminal cell, one above the other, as an upper half block.
fn draw_map(frame: &mut Frame, theme: &Theme, map: &ChunkMapInfo, area: Rect) {
    let rows = chunkmap::rows(map.cells.len(), area.width).min(usize::from(area.height));
    let buffer = frame.buffer_mut();
    for (index, cell) in map.cells.iter().enumerate() {
        let (column, half_row) = chunkmap::place(index, area.width);
        let row = half_row / 2;
        if row >= rows {
            break;
        }
        let Some(target) = buffer.cell_mut((area.x + column, area.y + row as u16)) else {
            continue;
        };
        let color = color(theme, chunkmap::shade(cell, map.running));
        if half_row % 2 == 0 {
            // The lower half stays the background unless a cell fills it.
            target.set_char('▀').set_fg(color).set_bg(theme.bg);
        } else {
            target.set_bg(color);
        }
    }
}
