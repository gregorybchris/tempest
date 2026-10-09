//! The Share area: the form, what the engine is doing with it, the shares and the selected
//! share's link.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::{fit, form, rule, rule_title, status_color, status_label};
use crate::app::App;
use crate::format;
use crate::theme::Theme;

pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let fields = app.share.form.fields.len() as u16;
    let [
        title,
        _,
        fields_area,
        status,
        list_rule,
        list,
        link_rule,
        link,
        bottom_rule,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(fields),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            " Share a file or folder",
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        title,
    );
    form::draw(
        frame,
        theme,
        fields_area,
        &app.share.form,
        app.share.editing,
    );
    draw_status(frame, app, status);

    let shares = app.shares();
    rule(frame, theme, list_rule);
    rule_title(
        frame,
        theme,
        list_rule,
        &format!("Shares · {}", shares.len()),
    );
    draw_shares(frame, app, list);
    rule(frame, theme, link_rule);
    draw_link(frame, app, link);
    rule(frame, theme, bottom_rule);
}

/// What the form is waiting for, or what happened to it.
fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let share = &app.share;
    let line = if let Some(path) = &share.building {
        Line::styled(
            format!(
                " Building the bundle from {}: reading and hashing every file…",
                path.display()
            ),
            Style::new().fg(theme.hot),
        )
    } else if let Some(error) = &share.form.error {
        Line::styled(format!(" {error}"), Style::new().fg(theme.bad))
    } else if share.editing {
        Line::styled(
            " Only the path is needed. The engine reads it on its own machine.",
            Style::new().fg(theme.dim),
        )
    } else {
        Line::styled(
            " Press a to fill in the form, then enter to share.",
            Style::new().fg(theme.dim),
        )
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_shares(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let shares = app.shares();
    if shares.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::styled(
                " Nothing shared yet. Shares made anywhere show here with their links.",
                Style::new().fg(theme.dim),
            )),
            area,
        );
        return;
    }
    let selected = app.selected_share().map(|t| t.id);
    let cursor = shares.iter().position(|t| Some(t.id) == selected);
    const STATUS: u16 = 11;
    const SIZE: u16 = 5;
    let name_width = usize::from(area.width.saturating_sub(STATUS + SIZE + 6) / 2).max(10);
    let link_width = usize::from(area.width)
        .saturating_sub(name_width + usize::from(STATUS + SIZE) + 5)
        .max(1);
    let rows = shares.iter().map(|transfer| {
        let lit = Some(transfer.id) == selected;
        let row = Row::new(vec![
            Cell::from(if lit { "▌" } else { " " }).style(Style::new().fg(theme.hot)),
            Cell::from(fit(&transfer.name, name_width)).style(Style::new().fg(theme.fg)),
            Cell::from(status_label(&transfer.status))
                .style(Style::new().fg(status_color(theme, &transfer.status))),
            Cell::from(
                Line::from(transfer.size.map_or_else(|| "–".to_owned(), format::bytes))
                    .right_aligned(),
            )
            .style(Style::new().fg(theme.dim)),
            Cell::from(fit(&transfer.link, link_width)).style(Style::new().fg(theme.dim)),
        ]);
        if lit {
            row.style(
                Style::new()
                    .bg(Theme::mix(theme.bg, theme.down, 0.14))
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            row
        }
    });
    let widths = [
        Constraint::Length(1),
        Constraint::Length(name_width as u16),
        Constraint::Length(STATUS),
        Constraint::Length(SIZE),
        Constraint::Min(0),
    ];
    let table = Table::new(rows, widths).column_spacing(1);
    let mut state = TableState::new()
        .with_offset(app.share.offset.get())
        .with_selected(cursor);
    frame.render_stateful_widget(table, area, &mut state);
    app.share.offset.set(state.offset());
}

/// The selected share's link, which the list cuts short, over as many lines as `area` has.
fn draw_link(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let Some(share) = app.selected_share() else {
        return;
    };
    const LABEL: &str = " Link  ";
    let room = usize::from(area.width)
        .saturating_sub(LABEL.len() + 1)
        .max(1);
    let chars: Vec<char> = share.link.chars().collect();
    let mut parts: Vec<String> = chars.chunks(room).map(|c| c.iter().collect()).collect();
    let rows = usize::from(area.height).max(1);
    if parts.len() > rows {
        parts.truncate(rows);
        let last = parts.last_mut().expect("there is a row");
        last.pop();
        last.push('…');
    }
    let lines: Vec<Line> = parts
        .into_iter()
        .enumerate()
        .map(|(row, part)| {
            let label = if row == 0 { LABEL } else { "       " };
            Line::from(vec![
                Span::styled(label, Style::new().fg(theme.dim)),
                Span::styled(part, Style::new().fg(theme.hot)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}
