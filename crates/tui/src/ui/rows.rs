//! The Catalog and Settings areas: sections of rows, a label and a value each, with the
//! selected row's meaning below them.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use unicode_width::UnicodeWidthStr;

use super::{fit, pad, rule};
use crate::app::{App, Area, Row};
use crate::theme::Theme;

/// The widest the labels' column gets.
const LABEL_MAX: usize = 32;

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let restart: Vec<String> = match (&app.settings, app.area) {
        (Some(view), Area::Settings) => view.restart_required.clone(),
        _ => Vec::new(),
    };
    let [banner, content, help_rule, help, bottom_rule] = Layout::vertical([
        Constraint::Length(u16::from(!restart.is_empty())),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);
    if !restart.is_empty() {
        let text = format!(
            " Changes to {} apply when the engine restarts.",
            restart.join(", ")
        );
        frame.render_widget(
            Paragraph::new(Line::styled(text, Style::new().fg(theme.warn))),
            banner,
        );
    }
    rule(frame, theme, help_rule);
    rule(frame, theme, bottom_rule);
    if app.settings.is_none() {
        let message = app
            .settings_error
            .clone()
            .unwrap_or_else(|| "Loading the settings…".to_owned());
        let [middle] = Layout::vertical([Constraint::Length(2)])
            .flex(Flex::Center)
            .areas(content);
        frame.render_widget(
            Paragraph::new(Line::styled(message, Style::new().fg(theme.dim)))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            middle,
        );
        return;
    }
    draw_sections(frame, app, content);
    draw_help(frame, app, help);
}

fn draw_sections(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let sections = app.sections();
    let changed = |row: &Row| {
        app.settings
            .as_ref()
            .is_some_and(|view| row.key.is_some_and(|key| view.changed.contains_key(key)))
    };
    let label_width = (usize::from(area.width) / 2).clamp(12, LABEL_MAX);
    let value_width = usize::from(area.width).saturating_sub(label_width + 7);
    let cursor = app.row_cursor();
    let mut lines: Vec<Line> = Vec::new();
    let mut cursor_line = 0;
    let mut index = 0;
    for (number, section) in sections.iter().enumerate() {
        if number > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            format!(" {}", section.title),
            Style::new().fg(theme.down).add_modifier(Modifier::BOLD),
        ));
        if let Some(note) = &section.note {
            let room = usize::from(area.width).saturating_sub(2);
            lines.extend(
                wrap(note, room)
                    .into_iter()
                    .map(|line| Line::styled(format!(" {line}"), Style::new().fg(theme.dim))),
            );
        }
        for row in &section.rows {
            let lit = index == cursor;
            if lit {
                cursor_line = lines.len();
            }
            lines.push(row_line(
                theme,
                row,
                lit,
                changed(row),
                label_width,
                value_width,
            ));
            index += 1;
        }
    }
    let height = usize::from(area.height);
    let offset = (cursor_line + 1).saturating_sub(height);
    let shown: Vec<Line> = lines.into_iter().skip(offset).take(height).collect();
    frame.render_widget(Paragraph::new(shown), area);
}

/// `text` broken into lines of at most `width` columns, between words where it can be.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.width() + 1 + word.width() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(|line| fit(&line, width)).collect()
}

fn row_line(
    theme: &Theme,
    row: &Row,
    lit: bool,
    changed: bool,
    label_width: usize,
    value_width: usize,
) -> Line<'static> {
    let adding = row.label.starts_with('+');
    let label_color = if adding { theme.dim } else { theme.fg };
    let mut spans = vec![
        Span::styled(if lit { "▌" } else { " " }, Style::new().fg(theme.hot)),
        Span::raw("  "),
    ];
    if row.value.is_empty() {
        let room = label_width + value_width + 2;
        spans.push(Span::styled(
            fit(&row.label, room),
            Style::new().fg(label_color),
        ));
    } else {
        spans.push(Span::styled(
            pad(&row.label, label_width),
            Style::new().fg(label_color),
        ));
        spans.push(Span::raw("  "));
        let value_color = if lit { theme.hot } else { theme.fg };
        spans.push(Span::styled(
            fit(&row.value, value_width),
            Style::new().fg(value_color),
        ));
        if changed {
            spans.push(Span::styled(" •", Style::new().fg(theme.hot)));
        }
    }
    let line = Line::from(spans);
    if lit {
        line.style(
            Style::new()
                .bg(Theme::mix(theme.bg, theme.down, 0.14))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        line
    }
}

/// What the selected row means, and what `•` marks.
fn draw_help(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let Some(view) = &app.settings else { return };
    let row = app.current_row();
    let changed = row
        .as_ref()
        .and_then(|row| row.key)
        .is_some_and(|key| view.changed.contains_key(key));
    let any_changed = app
        .rows()
        .iter()
        .any(|row| row.key.is_some_and(|key| view.changed.contains_key(key)));
    let help = row.map(|row| row.help).unwrap_or_default();
    let legend = if changed {
        " • Changed here, over the config file. x gives it the file's value again."
    } else if any_changed {
        " • marks settings changed here, over the config file."
    } else {
        ""
    };
    let room = usize::from(area.width).saturating_sub(1);
    // A sentence may take two lines; the legend has the last.
    let mut lines: Vec<Line> = wrap(&help, room)
        .into_iter()
        .take(2)
        .map(|line| Line::styled(format!(" {line}"), Style::new().fg(theme.fg)))
        .collect();
    lines.resize(2, Line::raw(""));
    lines.push(Line::styled(
        fit(legend, room + 1),
        Style::new().fg(theme.dim),
    ));
    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_wrap_between_words() {
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("one", 10), ["one"]);
        assert_eq!(wrap("unbreakable", 6), ["unbre…"]);
        assert!(wrap("", 5).is_empty());
    }
}
