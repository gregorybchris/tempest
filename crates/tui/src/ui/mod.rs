//! Drawing. Reads the [`App`]; never changes it.
//!
//! The screen follows the shared client layout (`specs/interfaces.md`): the areas and rates on
//! top, the area's content, and a bar of the keys that work at the bottom. In Transfers, the
//! content is the list with the selected transfer's state or detail below it, or one of its
//! full-screen views (D-137).

mod chunks;
mod detail;
mod modal;
mod rates;
mod transfers;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tempest_engine::Status;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Area, Hint, Modal, Origin, Screen};
use crate::format;
use crate::model::Connection;
use crate::theme::Theme;

/// Draws the whole screen.
pub fn draw(frame: &mut Frame, app: &App) {
    let theme = &app.theme;
    let area = frame.area();
    frame.render_widget(
        Block::new().style(Style::new().bg(theme.bg).fg(theme.fg)),
        area,
    );
    let [header, top_rule, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(frame, app, header);
    rule(frame, theme, top_rule);
    match app.screen() {
        Screen::List | Screen::Detail => {
            draw_finding(frame, app, top_rule);
            transfers::draw(frame, app, body);
        }
        Screen::Chunks => chunks::draw(frame, app, body),
        Screen::Rates => rates::draw(frame, app, body),
        Screen::Other => draw_empty_area(frame, theme, app.area, body),
    }
    draw_footer(frame, app, footer);
    modal::draw(frame, app, area);
}

/// The filters and order on the rule above the list, with how many transfers they leave.
fn draw_finding(frame: &mut Frame, app: &App, area: Rect) {
    let description = app.finding.describe();
    if description.is_empty() {
        return;
    }
    let theme = &app.theme;
    let mut spans = vec![Span::styled(
        format!(" {description} "),
        Style::new().fg(theme.hot),
    )];
    if app.finding.filtering() {
        let shown = app.visible().len();
        let total = app.model.transfers().len();
        spans.push(Span::styled(
            format!("· {shown} of {total} "),
            Style::new().fg(theme.dim),
        ));
    }
    let line = Line::from(spans);
    let width = (line.width() as u16).min(area.width.saturating_sub(2));
    let text = Rect {
        x: area.x + 1,
        width,
        ..area
    };
    frame.render_widget(Paragraph::new(line), text);
}

/// A horizontal rule across `area`.
fn rule(frame: &mut Frame, theme: &Theme, area: Rect) {
    frame.render_widget(
        Block::new()
            .borders(Borders::TOP)
            .border_style(Style::new().fg(theme.line)),
        area,
    );
}

/// The color a status is drawn in.
fn status_color(theme: &Theme, status: &Status) -> Color {
    match status {
        Status::Downloading => theme.down,
        Status::Sharing => theme.up,
        Status::Stalled { .. } => theme.warn,
        Status::Queued => theme.dim,
        Status::Stopped => theme.faint,
        Status::Completed => theme.fg,
        Status::Errored { .. } => theme.bad,
        Status::Checking | Status::Moving => theme.hot,
    }
}

/// A status as the list shows it: "Downloading".
fn status_label(status: &Status) -> String {
    let name = status.name();
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// `text` cut to `width` columns, ending in an ellipsis if it was cut.
fn fit(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut cut = String::new();
    for c in text.chars() {
        if cut.width() + c.to_string().width() + 1 > width {
            break;
        }
        cut.push(c);
    }
    cut.push('…');
    cut
}

/// `text` fit to `width` columns and padded to fill them.
fn pad(text: &str, width: usize) -> String {
    let fitted = fit(text, width);
    let fill = width.saturating_sub(fitted.width());
    fitted + &" ".repeat(fill)
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut nav = vec![Span::raw(" ")];
    for each in Area::ALL {
        let style = if each == app.area {
            Style::new()
                .fg(theme.bg)
                .bg(theme.down)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.dim)
        };
        nav.push(Span::styled(format!(" {} ", each.title()), style));
        nav.push(Span::raw(" "));
    }
    let nav = Line::from(nav);
    let nav_width = nav.width() as u16;
    frame.render_widget(Paragraph::new(nav), area);

    // The rates always show; the rest only while there is room, most important first.
    let (down, up) = app.model.total_rates();
    let rates = vec![
        Span::styled("↓", Style::new().fg(theme.dim)),
        Span::styled(
            format!("{:<5}", format::rate(down)),
            Style::new().fg(theme.down),
        ),
        Span::styled(" ↑", Style::new().fg(theme.dim)),
        Span::styled(
            format!("{:<5}", format::rate(up)),
            Style::new().fg(theme.up),
        ),
    ];
    let connection = match &app.model.connection {
        Connection::Live => None,
        Connection::Connecting => Some(Span::styled("connecting…  ", Style::new().fg(theme.dim))),
        Connection::Lost { .. } => {
            Some(Span::styled("reconnecting…  ", Style::new().fg(theme.warn)))
        }
    };
    let embedded = (app.origin == Origin::Embedded)
        .then(|| Span::styled("embedded  ", Style::new().fg(theme.dim)));
    let theme_name = Span::styled(format!(" [{}]", theme.name), Style::new().fg(theme.dim));
    let room = usize::from(area.width.saturating_sub(nav_width + 1));
    let mut used = Line::from(rates.clone()).width();
    let mut take = |span: Option<Span<'static>>| {
        let span = span?;
        (used + span.width() <= room).then(|| {
            used += span.width();
            span
        })
    };
    let connection = take(connection);
    let theme_name = take(Some(theme_name));
    let embedded = take(embedded);
    let mut spans: Vec<Span> = [connection, embedded].into_iter().flatten().collect();
    spans.extend(rates);
    spans.extend(theme_name);
    spans.push(Span::raw(" "));
    frame.render_widget(Paragraph::new(Line::from(spans).right_aligned()), area);
}

fn draw_empty_area(frame: &mut Frame, theme: &Theme, which: Area, area: Rect) {
    let [middle] = Layout::vertical([Constraint::Length(1)])
        .flex(Flex::Center)
        .areas(area);
    let text = format!("{} is not built yet.", which.title());
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::new().fg(theme.dim))).centered(),
        middle,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let hint = |hint: &Hint| {
        vec![
            Span::styled(
                hint.keys,
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {}  ", hint.action), Style::new().fg(theme.dim)),
        ]
    };
    // While filtering, the footer is the prompt, with its keys on the right.
    if let Some(Modal::Filter(input)) = &app.modal {
        let keys: Vec<Span> = app.hints().iter().flat_map(hint).collect();
        let keys = Line::from(keys);
        let keys_width = (keys.width() as u16).min(area.width / 2);
        let [prompt, input_area, keys_area] = Layout::horizontal([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(keys_width),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(Span::styled(" /", Style::new().fg(theme.hot))),
            prompt,
        );
        let (text, cursor) = input.view(usize::from(input_area.width.saturating_sub(1)));
        frame.render_widget(
            Paragraph::new(Span::styled(text, Style::new().fg(theme.hot))),
            input_area,
        );
        frame.set_cursor_position(Position::new(input_area.x + cursor as u16, area.y));
        frame.render_widget(Paragraph::new(keys), keys_area);
        return;
    }
    let line = match &app.notice {
        Some(notice) => {
            let color = if notice.error { theme.bad } else { theme.hot };
            Line::styled(format!(" {}", notice.text), Style::new().fg(color))
        }
        None => {
            // Hints that don't fit are left out, except the last (help, or how to close).
            let hints = app.hints();
            let (last, others) = hints.split_last().expect("there is always a hint");
            let room = usize::from(area.width).saturating_sub(1 + Line::from(hint(last)).width());
            let mut spans = vec![Span::raw(" ")];
            let mut used = 0;
            for each in others {
                let spans_of = hint(each);
                let width = Line::from(spans_of.clone()).width();
                if used + width > room {
                    break;
                }
                used += width;
                spans.extend(spans_of);
            }
            spans.extend(hint(last));
            Line::from(spans)
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_names_end_in_an_ellipsis() {
        assert_eq!(fit("ensemble.tar", 12), "ensemble.tar");
        assert_eq!(fit("ensemble-2026.tar", 10), "ensemble-…");
        assert_eq!(fit("日本語のファイル", 7), "日本語…");
        assert_eq!(pad("日本", 6), "日本  ");
        assert_eq!(pad("ensemble-2026.tar", 10), "ensemble-…");
    }

    #[test]
    fn statuses_are_capitalized() {
        assert_eq!(status_label(&Status::Downloading), "Downloading");
    }
}
