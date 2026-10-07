//! Modals drawn over the screen: help, the palette, the add form, confirmations, text prompts
//! and pickers. The filter prompt lives in the footer instead (see `draw_footer`).

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::{fit, form};
use crate::app::{AddField, AddForm, App, FormKind, Modal, PickKind, Target, TextKind, ValueEdit};
use crate::columns::Column;
use crate::commands::{self, Command};
use crate::form::Form;
use crate::input::Input;
use crate::theme::Theme;

/// The most matches the palette lists at once.
const PALETTE_ROWS: usize = 12;

/// Draws the open modal over `area`, the whole screen.
pub(super) fn draw(frame: &mut Frame, app: &App, area: Rect) {
    match &app.modal {
        None | Some(Modal::Filter(_)) => {}
        Some(Modal::Help { scroll }) => draw_help(frame, app, area, *scroll),
        Some(Modal::Palette { query, selected }) => {
            draw_palette(frame, app, area, query, *selected);
        }
        Some(Modal::Add(form)) => draw_add(frame, app, area, form),
        Some(Modal::Remove {
            target,
            delete_files,
        }) => draw_remove(frame, &app.theme, area, target, *delete_files),
        Some(Modal::Text {
            kind,
            target,
            input,
            error,
        }) => draw_text(
            frame,
            &app.theme,
            area,
            *kind,
            target,
            input,
            error.as_deref(),
        ),
        Some(Modal::Pick { kind, selected }) => draw_pick(frame, app, area, kind, *selected),
        Some(Modal::Value(edit)) => draw_value(frame, &app.theme, area, edit),
        Some(Modal::Form(form)) => draw_form(frame, &app.theme, area, &form.0, &form.1),
        Some(Modal::Confirm {
            title,
            text,
            detail,
            ..
        }) => draw_confirm(frame, &app.theme, area, title, text, detail),
        Some(Modal::Columns { chosen, cursor }) => {
            draw_columns(frame, &app.theme, area, chosen, *cursor);
        }
    }
}

/// A box of `width` by `height` centered in `area`, cleared, with a titled border. Returns
/// the inside.
pub(super) fn popup(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    size: (u16, u16),
    title: &str,
) -> Rect {
    let [popup] = Layout::vertical([Constraint::Length(size.1.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::horizontal([Constraint::Length(size.0.min(area.width))])
        .flex(Flex::Center)
        .areas(popup);
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.down))
        .style(Style::new().bg(theme.raise).fg(theme.fg))
        .title(Line::styled(
            format!(" {title} "),
            Style::new().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    inner
}

/// The style of a highlighted row.
pub(super) fn highlight(theme: &Theme) -> Style {
    Style::new()
        .bg(Theme::mix(theme.raise, theme.down, 0.2))
        .add_modifier(Modifier::BOLD)
}

/// `input` drawn in `area` (one row), showing `placeholder` while empty, with the terminal's
/// cursor in it when `focused`.
pub(super) fn draw_input(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    input: &Input,
    placeholder: &str,
    focused: bool,
) {
    let field = Style::new().bg(Theme::mix(theme.raise, theme.bg, 0.6));
    let (text, cursor) = input.view(usize::from(area.width.saturating_sub(1)));
    let line = if input.is_empty() {
        Line::styled(placeholder.to_owned(), field.fg(theme.faint))
    } else {
        Line::styled(text, field.fg(theme.hot))
    };
    frame.render_widget(Paragraph::new(line).style(field), area);
    if focused {
        frame.set_cursor_position(Position::new(area.x + cursor as u16, area.y));
    }
}

/// A line of one row: dim help, or an error.
pub(super) fn note_line<'a>(theme: &Theme, help: &'a str, error: Option<&'a str>) -> Line<'a> {
    match error {
        Some(error) => Line::styled(error, Style::new().fg(theme.bad)),
        None => Line::styled(help, Style::new().fg(theme.dim)),
    }
}

/// The width of a column of help.
const HELP_COLUMN: u16 = 36;

/// The current screen's keys as blocks of lines, a section each.
fn help_blocks(app: &App) -> Vec<Vec<Line<'static>>> {
    const KEYS: usize = 9;
    let theme = &app.theme;
    let screen = app.screen();
    app.help_sections()
        .into_iter()
        .map(|(section, commands)| {
            let mut lines = vec![Line::styled(
                section.title(),
                Style::new().fg(theme.dim).add_modifier(Modifier::BOLD),
            )];
            lines.extend(
                commands
                    .into_iter()
                    .filter_map(|c| c.help_line(screen))
                    .map(|(keys, label)| {
                        Line::from(vec![
                            Span::styled(format!("{keys:>KEYS$}  "), Style::new().fg(theme.hot)),
                            Span::styled(label, Style::new().fg(theme.fg)),
                        ])
                    }),
            );
            lines
        })
        .collect()
}

/// Help's columns for a screen of `area`, and how many rows of them show. As few columns as
/// fit the screen, as even as whole sections allow; when even the most that fit side by side
/// are taller than the screen, they scroll.
fn help_columns(app: &App, area: Rect) -> (Vec<Vec<Line<'static>>>, usize) {
    let blocks = help_blocks(app);
    let rows = usize::from(area.height.saturating_sub(4)).max(1);
    let most = usize::from(area.width.saturating_sub(3) / HELP_COLUMN).max(1);
    let total: usize = blocks.iter().map(Vec::len).sum::<usize>() + blocks.len() - 1;
    let count = pack(&blocks, rows).len();
    if count <= most {
        let even = (total.div_ceil(count)..rows)
            .find(|rows| pack(&blocks, *rows).len() <= count)
            .unwrap_or(rows);
        return (pack(&blocks, even), rows);
    }
    let tall = (rows..=total)
        .find(|rows| pack(&blocks, *rows).len() <= most)
        .unwrap_or(total);
    (pack(&blocks, tall), rows)
}

/// How many rows help can scroll on a screen of `area`.
pub(super) fn help_overflow(app: &App, area: Rect) -> usize {
    let (columns, rows) = help_columns(app, area);
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    tallest.saturating_sub(rows)
}

fn draw_help(frame: &mut Frame, app: &App, area: Rect, scroll: usize) {
    let theme = &app.theme;
    let (columns, rows) = help_columns(app, area);
    let tallest = columns.iter().map(Vec::len).max().unwrap_or(0);
    let scroll = scroll.min(tallest.saturating_sub(rows));
    let footnote = if tallest > rows {
        "j/k scroll. Arrows, home, end and the page keys move too."
    } else {
        "Arrows, home, end and the page keys move too."
    };
    let height = tallest.min(rows) as u16 + 4;
    let width = HELP_COLUMN * columns.len() as u16 + 3;
    let title = format!("{} keys", app.screen_title());
    let inner = popup(frame, theme, area, (width, height), &title);
    let [body, _, footnote_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let areas = Layout::horizontal(vec![Constraint::Length(HELP_COLUMN); columns.len()])
        .horizontal_margin(1)
        .split(body);
    for (column, area) in columns.into_iter().zip(areas.iter()) {
        let lines: Vec<Line> = column.into_iter().skip(scroll).collect();
        frame.render_widget(Paragraph::new(lines), *area);
    }
    frame.render_widget(
        Paragraph::new(Line::styled(footnote, Style::new().fg(theme.dim))).centered(),
        footnote_area,
    );
}

/// `blocks` in columns of at most `rows` lines, a blank line between blocks, splitting none
/// unless it is taller than a column.
fn pack<'a>(blocks: &[Vec<Line<'a>>], rows: usize) -> Vec<Vec<Line<'a>>> {
    let mut columns: Vec<Vec<Line>> = vec![Vec::new()];
    for block in blocks {
        let column = columns.last_mut().expect("there is a column");
        if column.is_empty() {
            column.extend(block.iter().cloned());
        } else if column.len() + 1 + block.len() > rows {
            columns.push(block.clone());
        } else {
            column.push(Line::raw(""));
            column.extend(block.iter().cloned());
        }
    }
    columns
}

fn draw_palette(frame: &mut Frame, app: &App, area: Rect, query: &Input, selected: usize) {
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
    // Scroll so the highlighted match shows.
    let first = selected.saturating_sub(rows - 1);
    let lines: Vec<Line> = matches
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(index, command)| palette_row(app, *command, index == selected, list.width))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

fn palette_row(app: &App, command: Command, selected: bool, width: u16) -> Line<'static> {
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

fn draw_add(frame: &mut Frame, app: &App, area: Rect, form: &AddForm) {
    let theme = &app.theme;
    let width = 64.min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, 9), "Add a link");
    let rows = Layout::vertical([Constraint::Length(1); 7]).split(inner);
    const LABEL: u16 = 10;
    for (field, row) in AddField::ALL.into_iter().zip(rows.iter()) {
        let focused = form.focus == field;
        let [label, value] =
            Layout::horizontal([Constraint::Length(LABEL), Constraint::Min(1)]).areas(*row);
        let label_style = if focused {
            Style::new().fg(theme.hot).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.dim)
        };
        frame.render_widget(
            Paragraph::new(Line::styled(format!("{:>8}  ", field.label()), label_style)),
            label,
        );
        let value = Rect {
            width: value.width.saturating_sub(1),
            ..value
        };
        match field {
            AddField::Link => {
                draw_input(frame, theme, value, &form.link, "tempest:…", focused);
            }
            AddField::SaveFolder => {
                let placeholder = "the category's or the engine's folder";
                draw_input(frame, theme, value, &form.save_folder, placeholder, focused);
            }
            AddField::Tags => {
                draw_input(frame, theme, value, &form.tags, "comma-separated", focused);
            }
            AddField::Category => {
                let text = form.category.as_deref().unwrap_or("none");
                draw_choice(frame, theme, value, text, focused);
            }
            AddField::Stopped => {
                let text = if form.stopped {
                    "later (add it stopped)"
                } else {
                    "right away"
                };
                draw_choice(frame, theme, value, text, focused);
            }
        }
    }
    let note = note_line(
        theme,
        "Paste a link and press enter; the rest is optional.",
        form.error.as_deref(),
    );
    frame.render_widget(Paragraph::new(note), rows[6]);
}

/// A value chosen with ← and →.
pub(super) fn draw_choice(frame: &mut Frame, theme: &Theme, area: Rect, text: &str, focused: bool) {
    let arrows = if focused { theme.hot } else { theme.faint };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("‹ ", Style::new().fg(arrows)),
            Span::styled(text.to_owned(), Style::new().fg(theme.fg)),
            Span::styled(" ›", Style::new().fg(arrows)),
        ])),
        area,
    );
}

fn draw_remove(frame: &mut Frame, theme: &Theme, area: Rect, target: &Target, delete: bool) {
    let width = 56.min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, 7), "Remove");
    let name = fit(&target.name, usize::from(inner.width).saturating_sub(10));
    let button = |text: &str, chosen: bool, danger: bool| {
        let style = match (chosen, danger) {
            (false, _) => Style::new().fg(theme.dim),
            (true, false) => Style::new()
                .fg(theme.bg)
                .bg(theme.down)
                .add_modifier(Modifier::BOLD),
            (true, true) => Style::new()
                .fg(theme.bg)
                .bg(theme.bad)
                .add_modifier(Modifier::BOLD),
        };
        Span::styled(format!(" {text} "), style)
    };
    let explanation = if delete {
        "Its files are deleted from disk too."
    } else {
        "The transfer goes from the list; its files stay."
    };
    let lines = vec![
        Line::from(vec![
            Span::raw(" Remove "),
            Span::styled(name, Style::new().fg(theme.hot)),
            Span::raw("?"),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::raw(" "),
            button("Keep files", !delete, false),
            Span::raw("  "),
            button("Delete files", delete, true),
        ]),
        Line::raw(""),
        Line::styled(format!(" {explanation}"), Style::new().fg(theme.dim)),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_text(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    kind: TextKind,
    target: &Target,
    input: &Input,
    error: Option<&str>,
) {
    let width = 60.min(area.width.saturating_sub(4));
    let title = format!("{} {}", kind.title(), fit(&target.name, 32));
    let inner = popup(frame, theme, area, (width, 5), &title);
    let [field, _, note] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);
    let field = Rect {
        x: field.x + 1,
        width: field.width.saturating_sub(2),
        ..field
    };
    let placeholder = match kind {
        TextKind::Rename => "the bundle's name",
        TextKind::Tags => "no tags",
        TextKind::Move => "/path/to/folder",
    };
    draw_input(frame, theme, field, input, placeholder, true);
    let note = Rect {
        x: note.x + 1,
        width: note.width.saturating_sub(1),
        ..note
    };
    frame.render_widget(Paragraph::new(note_line(theme, kind.help(), error)), note);
}

fn draw_pick(frame: &mut Frame, app: &App, area: Rect, kind: &PickKind, selected: usize) {
    let theme = &app.theme;
    let items = app.pick_items(kind);
    let note = match (kind, &app.categories) {
        (PickKind::Category(_), None) => Some(" Loading categories…"),
        (PickKind::Category(_), Some(categories)) if categories.is_empty() => {
            Some(" No categories yet.")
        }
        (PickKind::View, _) if app.views.is_empty() => Some(" No saved views yet. V saves one."),
        _ => None,
    };
    let extra = u16::from(note.is_some());
    let rows = (items.len() as u16 + extra).min(area.height.saturating_sub(4));
    let title = match kind {
        PickKind::Category(target) => format!("Category of {}", fit(&target.name, 24)),
        other => other.title().to_owned(),
    };
    let width = (title.width() as u16 + 6)
        .max(34)
        .min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, rows + 2), &title);
    let first = selected.saturating_sub(usize::from(rows.saturating_sub(1)));
    let mut lines: Vec<Line> = items
        .iter()
        .enumerate()
        .skip(first)
        .map(|(index, item)| {
            let room = usize::from(inner.width).saturating_sub(item.note.width() + 3);
            let label = fit(&item.label, room);
            let gap =
                usize::from(inner.width).saturating_sub(label.width() + item.note.width() + 2);
            let line = Line::from(vec![
                Span::styled(format!(" {label}"), Style::new().fg(theme.fg)),
                Span::raw(" ".repeat(gap)),
                Span::styled(format!("{} ", item.note), Style::new().fg(theme.dim)),
            ]);
            if index == selected {
                line.style(highlight(theme))
            } else {
                line
            }
        })
        .collect();
    if let Some(note) = note {
        lines.push(Line::styled(note, Style::new().fg(theme.dim)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_value(frame: &mut Frame, theme: &Theme, area: Rect, edit: &ValueEdit) {
    let width = 64.min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, 5), &fit(&edit.title, 48));
    let [field, _, note] = Layout::vertical([Constraint::Length(1); 3]).areas(inner);
    let field = Rect {
        x: field.x + 1,
        width: field.width.saturating_sub(2),
        ..field
    };
    draw_input(frame, theme, field, &edit.input, &edit.placeholder, true);
    let note = Rect {
        x: note.x + 1,
        width: note.width.saturating_sub(1),
        ..note
    };
    let line = note_line(theme, edit.help(), edit.error.as_deref());
    frame.render_widget(Paragraph::new(line), note);
}

fn draw_form(frame: &mut Frame, theme: &Theme, area: Rect, kind: &FormKind, form: &Form) {
    let width = 64.min(area.width.saturating_sub(4));
    let height = form.fields.len() as u16 + 4;
    let inner = popup(frame, theme, area, (width, height), &fit(&kind.title(), 48));
    let [fields, _, note] = Layout::vertical([
        Constraint::Length(form.fields.len() as u16),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    form::draw(frame, theme, fields, form, true);
    let note = Rect {
        x: note.x + 1,
        width: note.width.saturating_sub(1),
        ..note
    };
    let line = note_line(theme, kind.help(), form.error.as_deref());
    frame.render_widget(Paragraph::new(line), note);
}

fn draw_confirm(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    text: &str,
    detail: &str,
) {
    let width = 60.min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, 7), title);
    let room = usize::from(inner.width).saturating_sub(2);
    let lines = vec![
        Line::styled(format!(" {}", fit(text, room)), Style::new().fg(theme.fg)),
        Line::raw(""),
        Line::styled(
            format!(" {}", fit(detail, room)),
            Style::new().fg(theme.dim),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::styled(
                " enter",
                Style::new().fg(theme.hot).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" yes   ", Style::new().fg(theme.dim)),
            Span::styled(
                "esc",
                Style::new().fg(theme.hot).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" no", Style::new().fg(theme.dim)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_columns(frame: &mut Frame, theme: &Theme, area: Rect, chosen: &[Column], cursor: usize) {
    let rows = Column::OPTIONAL.len() as u16;
    let width = 44.min(area.width.saturating_sub(4));
    let inner = popup(frame, theme, area, (width, rows + 4), "Columns");
    let [list, _, note] = Layout::vertical([
        Constraint::Length(rows),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let lines: Vec<Line> = Column::OPTIONAL
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let on = chosen.contains(column);
            let mark = if on { "[x]" } else { "[ ]" };
            let mark_color = if on { theme.down } else { theme.dim };
            let line = Line::from(vec![
                Span::styled(format!(" {mark} "), Style::new().fg(mark_color)),
                Span::styled(column.description(), Style::new().fg(theme.fg)),
            ]);
            if index == cursor {
                line.style(highlight(theme))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    frame.render_widget(
        Paragraph::new(Line::styled(
            " Name, progress and status always show.",
            Style::new().fg(theme.dim),
        )),
        note,
    );
}
