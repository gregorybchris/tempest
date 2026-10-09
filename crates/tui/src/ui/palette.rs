//! The `:` command palette: commands matching what is typed, with their keys.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::fit;
use super::modal::{draw_input, highlight, popup};
use crate::app::App;
use crate::commands::{self, Command};
use crate::input::Input;

/// The most matches the palette lists at once.
pub(super) const PALETTE_ROWS: usize = 12;

pub(super) fn draw_palette(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    query: &Input,
    selected: usize,
) {
    let theme = &app.theme;
    let matches = commands::palette_matches(app.screen(), &query.text());
    let rows = matches.len().clamp(1, PALETTE_ROWS);
    let width = 56.min(area.width.saturating_sub(4));
    let height = rows as u16 + 4;
    // The palette sits near the top, where the eye already is after pressing `:`.
    let top = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(2),
        ..area
    };
    let [top] = Layout::vertical([Constraint::Length(height)]).areas(top);
    let inner = popup(frame, theme, top, (width, height), "Commands");
    let [input_row, rule_row, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    let [prompt, input_area] =
        Layout::horizontal([Constraint::Length(2), Constraint::Min(1)]).areas(input_row);
    frame.render_widget(
        Paragraph::new(Span::styled(":", Style::new().fg(theme.hot))),
        prompt,
    );
    draw_input(
        frame,
        theme,
        input_area,
        query,
        "type to find a command",
        true,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(rule_row.width))).style(Style::new().fg(theme.line)),
        rule_row,
    );

    if matches.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                " No command matches.",
                Style::new().fg(theme.dim),
            )),
            list,
        );
        return;
    }
    // Scroll so the highlighted match shows in the rows the popup has.
    let shown = usize::from(list.height).clamp(1, rows);
    let first = selected.saturating_sub(shown - 1);
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(first)
        .take(shown)
        .map(|(index, command)| palette_row(app, *command, index == selected, list.width))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

pub(super) fn palette_row(
    app: &App,
    command: Command,
    selected: bool,
    width: u16,
) -> Line<'static> {
    let theme = &app.theme;
    let keys = command.key_label(app.screen());
    let label_width = usize::from(width).saturating_sub(keys.width() + 3);
    let label = fit(command.label(), label_width);
    let gap = usize::from(width).saturating_sub(label.width() + keys.width() + 2);
    let color = if app.works(command) {
        theme.fg
    } else {
        theme.faint
    };
    let line = Line::from(vec![
        Span::styled(format!(" {label}"), Style::new().fg(color)),
        Span::raw(" ".repeat(gap)),
        Span::styled(format!("{keys} "), Style::new().fg(theme.dim)),
    ]);
    if selected {
        line.style(highlight(theme))
    } else {
        line
    }
}
