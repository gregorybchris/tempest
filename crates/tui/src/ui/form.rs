//! A form's fields, one per row: a label, then the value.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::modal::{draw_choice, draw_input};
use crate::form::{DAYS, Form, Value};
use crate::theme::Theme;

/// The width of the labels' column.
pub(super) const LABEL: u16 = 16;

/// Draws `form`'s fields in `area`, a row each, with the focused one lit when `focused`.
pub(super) fn draw(frame: &mut Frame, theme: &Theme, area: Rect, form: &Form, focused: bool) {
    let rows = Layout::vertical(vec![Constraint::Length(1); form.fields.len()]).split(area);
    for (index, (field, row)) in form.fields.iter().zip(rows.iter()).enumerate() {
        let lit = focused && index == form.focus;
        let [label, value] =
            Layout::horizontal([Constraint::Length(LABEL), Constraint::Min(1)]).areas(*row);
        let label_style = if lit {
            Style::new().fg(theme.hot).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.dim)
        };
        let width = usize::from(LABEL - 3);
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!(" {:>width$}  ", field.label),
                label_style,
            )),
            label,
        );
        let value = Rect {
            width: value.width.saturating_sub(1),
            ..value
        };
        match &field.value {
            Value::Text(input) => draw_input(frame, theme, value, input, &field.placeholder, lit),
            Value::Choice { options, chosen } => {
                let text = options.get(*chosen).map_or("", String::as_str);
                draw_choice(frame, theme, value, text, lit);
            }
            Value::Days { on, cursor } => {
                let spans: Vec<Span> = DAYS
                    .iter()
                    .enumerate()
                    .flat_map(|(day, name)| {
                        let mut style = if on[day] {
                            Style::new().fg(theme.bg).bg(theme.down)
                        } else {
                            Style::new().fg(theme.dim)
                        };
                        if lit && day == *cursor {
                            style = style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
                        }
                        [Span::styled(format!(" {name} "), style), Span::raw(" ")]
                    })
                    .collect();
                frame.render_widget(Paragraph::new(Line::from(spans)), value);
            }
        }
    }
}
