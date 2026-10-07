//! The Transfers area: the list, and below it the selected transfer's state in plain language.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use tempest_engine::{Status, TransferInfo};

use super::{fit, rule, status_color, status_label};
use crate::app::App;
use crate::columns::{self, BAR, Column, SPACING};
use crate::explain::{Fix, explain};
use crate::format;
use crate::model::Connection;
use crate::theme::Theme;

/// The marker column, which shows the cursor.
const MARKER: u16 = 1;

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let [list, middle_rule, summary, bottom_rule] = Layout::vertical([
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_list(frame, app, list);
    rule(frame, &app.theme, middle_rule);
    draw_summary(frame, app, summary);
    rule(frame, &app.theme, bottom_rule);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let shown = columns::fitting(area.width.saturating_sub(MARKER + SPACING));
    let name_width = name_width(&shown, area.width);

    let mut titles = vec![Cell::from("")];
    titles.extend(shown.iter().map(|column| {
        let line = Line::from(column.title());
        let line = if column.numeric() {
            line.right_aligned()
        } else {
            line
        };
        Cell::from(line)
    }));
    let header = Row::new(titles).style(Style::new().fg(theme.dim));

    let visible = app.visible();
    let cursor = app.cursor();
    let rows = visible.iter().enumerate().map(|(index, transfer)| {
        let selected = cursor == Some(index);
        let mut cells =
            vec![Cell::from(if selected { "▌" } else { " " }).style(Style::new().fg(theme.hot))];
        cells.extend(
            shown
                .iter()
                .map(|column| cell(theme, transfer, *column, name_width)),
        );
        let row = Row::new(cells);
        if selected {
            row.style(
                Style::new()
                    .bg(Theme::mix(theme.bg, theme.down, 0.14))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            row
        }
    });

    let mut widths = vec![Constraint::Length(MARKER)];
    widths.extend(shown.iter().map(|column| match column {
        Column::Name => Constraint::Min(column.width()),
        other => Constraint::Length(other.width()),
    }));
    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(SPACING);
    let mut state = TableState::new().with_selected(cursor);
    frame.render_stateful_widget(table, area, &mut state);

    if visible.is_empty() {
        let message = match app.model.connection {
            Connection::Connecting => "Connecting to the engine…",
            _ => "No transfers yet. `tempest get <link>` or `tempest share <path>` adds one.",
        };
        let [middle] = Layout::vertical([Constraint::Length(2)])
            .flex(Flex::Center)
            .areas(area);
        frame.render_widget(
            Paragraph::new(Line::styled(message, Style::new().fg(theme.dim)))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            middle,
        );
    }
}

/// The name column's width: what the other columns leave.
fn name_width(shown: &[Column], width: u16) -> usize {
    let others: u16 = shown
        .iter()
        .filter(|column| **column != Column::Name)
        .map(|column| column.width() + SPACING)
        .sum();
    usize::from(width.saturating_sub(MARKER + SPACING + others))
}

fn cell<'a>(theme: &Theme, transfer: &TransferInfo, column: Column, name_width: usize) -> Cell<'a> {
    let dash = || "–".to_owned();
    let (text, color) = match column {
        Column::Name => (fit(&transfer.name, name_width), theme.fg),
        Column::Size => (transfer.size.map_or_else(dash, format::bytes), theme.dim),
        Column::Progress => return Cell::from(progress(theme, transfer)),
        Column::Down => (format::rate(transfer.download_rate), theme.down),
        Column::Up => (format::rate(transfer.upload_rate), theme.up),
        Column::Eta => (transfer.eta.map_or_else(dash, format::duration), theme.dim),
        Column::Status => (
            status_label(&transfer.status),
            status_color(theme, &transfer.status),
        ),
        Column::Peers => match transfer.peers {
            0 => (dash(), theme.dim),
            peers => (format!("{}/{peers}", transfer.connections), theme.dim),
        },
        Column::Ratio => (
            transfer
                .ratio
                .map_or_else(dash, |ratio| format!("{ratio:.2}")),
            theme.dim,
        ),
    };
    let line = Line::styled(text, Style::new().fg(color));
    Cell::from(if column.numeric() {
        line.right_aligned()
    } else {
        line
    })
}

/// A bar of [`BAR`] cells and the percentage.
fn progress<'a>(theme: &Theme, transfer: &TransferInfo) -> Line<'a> {
    let width = usize::from(BAR);
    let filled = ((transfer.progress.clamp(0.0, 1.0) * width as f64).floor() as usize).min(width);
    let color = status_color(theme, &transfer.status);
    Line::from(vec![
        Span::styled("█".repeat(filled), Style::new().fg(color)),
        Span::styled("░".repeat(width - filled), Style::new().fg(theme.cell_off)),
        Span::styled(
            format!(" {:>4}", format::percent(transfer.progress)),
            Style::new().fg(theme.dim),
        ),
    ])
}

fn draw_summary(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let text = match (&app.model.connection, app.selected_transfer()) {
        (Connection::Lost { error }, _) => Text::from(vec![
            Line::styled(
                " Lost touch with the engine. Trying again…",
                Style::new().fg(theme.warn),
            ),
            Line::styled(format!(" {error}"), Style::new().fg(theme.dim)),
        ]),
        (_, None) => Text::default(),
        (_, Some(transfer)) => {
            let explanation = explain(transfer);
            let mut spans = vec![Span::styled(
                format!(" {}", explanation.text),
                Style::new().fg(summary_color(theme, &transfer.status)),
            )];
            if let Some(fix) = explanation.fix {
                spans.push(Span::styled(
                    format!(" {}", fix_hint(fix)),
                    Style::new().fg(theme.dim),
                ));
            }
            Text::from(Line::from(spans))
        }
    };
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

/// The summary is body text, so it keeps the readable colors and only stuck states stand out.
fn summary_color(theme: &Theme, status: &Status) -> ratatui::style::Color {
    match status {
        Status::Stalled { .. } => theme.warn,
        Status::Errored { .. } => theme.bad,
        _ => theme.fg,
    }
}

/// How to apply a fix in the TUI.
fn fix_hint(fix: Fix) -> &'static str {
    match fix {
        Fix::Start => "Press space to start it.",
        Fix::Retry => "Press space to try again.",
        Fix::ShareAgain => "Press space to share it again.",
        Fix::ForceStart => "Press F to start it now.",
    }
}
