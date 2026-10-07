//! Dummy Tempest terminal UI, driven by simulated data.
//!
//! `cargo run --release` runs it. `cargo run -- snapshot [width height]`
//! prints one frame as plain text, for review without a terminal.

mod app;
mod theme;
mod ui;

use std::time::{Duration, Instant};

use app::{App, Command, Mode};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

const TICK: Duration = Duration::from_millis(250);

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("snapshot") {
        let w = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(160);
        let h = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(48);
        return snapshot(w, h, args.get(4).map(String::as_str));
    }
    ratatui::run(|terminal| {
        let mut app = App::new();
        let mut last = Instant::now();
        while !app.quit {
            terminal.draw(|f| ui::draw(f, &app))?;
            let timeout = TICK.saturating_sub(last.elapsed());
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        handle_key(&mut app, key);
                    }
                }
            }
            if last.elapsed() >= TICK {
                app.tick(TICK.as_millis() as u64);
                last = Instant::now();
            }
        }
        Ok(())
    })
}

fn handle_key(app: &mut App, key: KeyEvent) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return;
    }
    match app.mode.clone() {
        Mode::Normal => normal_key(app, key),
        Mode::Filter => match key.code {
            KeyCode::Esc => {
                app.filter.clear();
                app.mode = Mode::Normal;
                app.refresh();
            }
            KeyCode::Enter => app.mode = Mode::Normal,
            KeyCode::Backspace => {
                app.filter.pop();
                app.refresh();
            }
            KeyCode::Char(c) => {
                app.filter.push(c);
                app.cursor = 0;
                app.refresh();
            }
            KeyCode::Down => app.move_cursor(1),
            KeyCode::Up => app.move_cursor(-1),
            _ => {}
        },
        Mode::Rename(text) => edit_text(app, key, text, Mode::Rename, App::apply_rename),
        Mode::Tags(text) => edit_text(app, key, text, Mode::Tags, App::apply_tags),
        Mode::Add(text) => edit_text(app, key, text, Mode::Add, App::apply_add),
        Mode::ConfirmRemove => match key.code {
            KeyCode::Char('r') => app.confirm_remove(false),
            KeyCode::Char('d') => app.confirm_remove(true),
            _ => app.mode = Mode::Normal,
        },
        Mode::Palette { mut query, mut selected } => {
            let matches = App::palette_matches(&query);
            match key.code {
                KeyCode::Esc => app.mode = Mode::Normal,
                KeyCode::Enter => {
                    app.mode = Mode::Normal;
                    if let Some(c) = matches.get(selected) {
                        app.run(*c);
                    }
                }
                KeyCode::Down => {
                    selected = (selected + 1).min(matches.len().saturating_sub(1));
                    app.mode = Mode::Palette { query, selected };
                }
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                    app.mode = Mode::Palette { query, selected };
                }
                KeyCode::Backspace => {
                    query.pop();
                    app.mode = Mode::Palette { query, selected: 0 };
                }
                KeyCode::Char(c) => {
                    query.push(c);
                    app.mode = Mode::Palette { query, selected: 0 };
                }
                _ => {}
            }
        }
        Mode::Help => app.mode = Mode::Normal,
    }
}

fn normal_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('q') => app.run(Command::Quit),
        KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1),
        KeyCode::Char('g') | KeyCode::Home => app.cursor = 0,
        KeyCode::Char('G') | KeyCode::End => app.cursor = app.visible.len().saturating_sub(1),
        KeyCode::Char(' ') => app.run(Command::StartStop),
        KeyCode::Char('F') => app.run(Command::ForceStart),
        KeyCode::Char('a') => app.run(Command::Add),
        KeyCode::Char('r') => app.run(Command::Rename),
        KeyCode::Char('t') => app.run(Command::Tags),
        KeyCode::Char('o') => app.run(Command::OpenFolder),
        KeyCode::Char('x') | KeyCode::Delete => app.run(Command::Remove),
        KeyCode::Char('/') => app.run(Command::Filter),
        KeyCode::Char('s') => app.run(Command::NextSort),
        KeyCode::Char('S') => app.run(Command::ReverseSort),
        KeyCode::Char(']') => app.cycle_status(1),
        KeyCode::Char('[') => app.cycle_status(-1),
        KeyCode::Char(c @ '0'..='7') => {
            let i = c as usize - '0' as usize;
            app.set_status_filter(if i == 0 { None } else { Some(app::Status::ALL[i - 1]) });
        }
        KeyCode::Tab => app.run(Command::NextTab),
        KeyCode::BackTab => {
            for _ in 0..3 {
                app.run(Command::NextTab);
            }
        }
        KeyCode::Char('T') => app.run(Command::NextTheme),
        KeyCode::Char('?') => app.run(Command::Help),
        KeyCode::Char(':') => app.mode = Mode::Palette { query: String::new(), selected: 0 },
        KeyCode::Esc => {
            app.filter.clear();
            app.set_status_filter(None);
        }
        _ => {}
    }
}

fn edit_text(app: &mut App, key: KeyEvent, mut text: String, mode: fn(String) -> Mode, apply: fn(&mut App, String)) {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => apply(app, text),
        KeyCode::Backspace => {
            text.pop();
            app.mode = mode(text);
        }
        KeyCode::Char(c) => {
            text.push(c);
            app.mode = mode(text);
        }
        _ => {}
    }
}

/// Renders one frame to text, optionally after a sequence of keys.
fn snapshot(width: u16, height: u16, keys: Option<&str>) -> std::io::Result<()> {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut app = App::new();
    for _ in 0..8 {
        app.tick(250);
    }
    for c in keys.unwrap_or("").chars() {
        let code = match c {
            '\t' => KeyCode::Tab,
            '\n' => KeyCode::Enter,
            '~' => KeyCode::Esc,
            c => KeyCode::Char(c),
        };
        handle_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE));
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|f| ui::draw(f, &app)).expect("draw");
    let buf = terminal.backend().buffer();
    for y in 0..height {
        let line: String = (0..width).map(|x| buf[(x, y)].symbol().to_string()).collect();
        println!("{}", line.trim_end());
    }
    Ok(())
}
