//! The help modal: every key of the current screen, in columns that fit the screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::modal::popup;
use crate::app::App;

/// The width of a column of help.
pub(super) const HELP_COLUMN: u16 = 36;

/// The current screen's keys as blocks of lines, a section each.
pub(super) fn help_blocks(app: &App) -> Vec<Vec<Line<'static>>> {
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
pub(super) fn help_columns(app: &App, area: Rect) -> (Vec<Vec<Line<'static>>>, usize) {
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

pub(super) fn draw_help(frame: &mut Frame, app: &App, area: Rect, scroll: usize) {
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
pub(super) fn pack<'a>(blocks: &[Vec<Line<'a>>], rows: usize) -> Vec<Vec<Line<'a>>> {
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
