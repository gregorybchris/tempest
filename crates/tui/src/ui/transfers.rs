//! The Transfers area: the list, and below it the selected transfer's state in plain language
//! or, once opened, its detail.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Wrap};
use tempest_engine::{Status, TransferInfo};

use super::{detail, fit, rule, status_color, status_label};
use crate::app::{App, View};
use crate::columns::{self, BAR, Column, SPACING};
use crate::explain::{Fix, explain};
use crate::finding::{Finding, Sort, SortKey};
use crate::format;
use crate::model::Connection;
use crate::theme::Theme;

/// The marker column, which shows the cursor.
const MARKER: u16 = 1;

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    if app.view == View::Detail {
        // The detail takes three fifths of what the rules leave, the list the rest.
        let detail = (area.height.saturating_sub(2) * 3 / 5).max(4);
        let [list, tabs_rule, content, bottom_rule] = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length(1),
            Constraint::Length(detail),
            Constraint::Length(1),
        ])
        .areas(area);
        draw_list(frame, app, list);
        rule(frame, &app.theme, tabs_rule);
        detail::draw_tabs(frame, app, tabs_rule);
        detail::draw(frame, app, content);
        rule(frame, &app.theme, bottom_rule);
        return;
    }
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
    let shown = columns::fitting(area.width.saturating_sub(MARKER + SPACING), &app.columns);
    let name_width = name_width(&shown, area.width);

    let sorted = sorted_column(app.finding.sort.key);
    let arrow = if app.finding.sort.ascending() {
        "▲"
    } else {
        "▼"
    };
    let mut titles = vec![Cell::from("")];
    titles.extend(shown.iter().map(|column| {
        let line = if Some(*column) == sorted {
            Line::from(vec![
                Span::raw(column.title()),
                Span::styled(arrow, Style::new().fg(theme.hot)),
            ])
        } else {
            Line::from(column.title())
        };
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
        let filters = Finding {
            sort: Sort::default(),
            ..app.finding.clone()
        };
        let message = match app.model.connection {
            Connection::Connecting => "Connecting to the engine…".to_owned(),
            _ if !app.model.transfers().is_empty() => format!(
                "Nothing matches {}. Press esc to clear the filters.",
                filters.describe()
            ),
            _ => "No transfers yet. Press a to add a link.".to_owned(),
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

/// The column showing what the list is sorted by, if one does.
fn sorted_column(key: SortKey) -> Option<Column> {
    Some(match key {
        SortKey::Queue => return None,
        SortKey::Added => Column::Added,
        SortKey::Name => Column::Name,
        SortKey::Size => Column::Size,
        SortKey::Progress => Column::Progress,
        SortKey::Status => Column::Status,
        SortKey::Down => Column::Down,
        SortKey::Up => Column::Up,
        SortKey::Peers => Column::Peers,
        SortKey::TimeLeft => Column::Eta,
        SortKey::Ratio => Column::Ratio,
    })
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
        Column::Category => (
            fit(transfer.category.as_deref().unwrap_or("–"), 10),
            theme.dim,
        ),
        Column::Tags => {
            let tags: Vec<&str> = transfer.tags.iter().map(String::as_str).collect();
            let text = if tags.is_empty() {
                dash()
            } else {
                tags.join(", ")
            };
            (fit(&text, 12), theme.dim)
        }
        Column::Added => (format::date(transfer.added), theme.dim),
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
pub(super) fn summary_color(theme: &Theme, status: &Status) -> ratatui::style::Color {
    match status {
        Status::Stalled { .. } => theme.warn,
        Status::Errored { .. } => theme.bad,
        _ => theme.fg,
    }
}

/// How to apply a fix in the TUI.
pub(super) fn fix_hint(fix: Fix) -> &'static str {
    match fix {
        Fix::Start => "Press space to start it.",
        Fix::Retry => "Press space to try again.",
        Fix::ShareAgain => "Press space to share it again.",
        Fix::ForceStart => "Press F to start it now.",
    }
}
