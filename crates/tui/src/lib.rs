//! Tempest's terminal UI (D-090): a client of the engine's API and event stream, laid out as
//! `specs/interfaces.md` describes.
//!
//! - [`app`]: the state and what keys do to it, with [`commands`] binding keys to commands.
//! - [`model`]: what the engine has said, kept up to date from its events.
//! - [`finding`]: filtering and sorting the list.
//! - [`ui`]: drawing the state.
//! - [`explain`]: transfer states in plain language.
//! - [`columns`], [`format`] and [`theme`]: how the list fits, numbers read and colors look.
//! - [`chunkmap`]: how a transfer's chunk map fits the terminal and shades its cells.
//! - [`input`] and [`form`]: text being typed, and forms of fields.
//! - [`prefs`]: the theme, columns and views kept in the engine's settings.
//!
//! [`run`] connects them to a terminal and an engine.

pub mod app;
pub mod chunkmap;
pub mod columns;
pub mod commands;
pub mod form;
pub mod format;
pub mod input;
pub mod prefs;
pub mod theme;
pub mod ui;

pub use tempest_api::{explain, finding, model};

use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEvent, KeyEventKind,
};
use ratatui::crossterm::execute;
use tempest_api::ApiEvent;
use tempest_engine::api::QueuePlace;
use tempest_engine::api::client::ApiClient;
use tokio::sync::mpsc;

use crate::app::{Action, App, Look, Origin, Reply, Request};
use crate::model::Connection;
use crate::theme::Theme;

/// How long to wait before reopening a broken event stream.
const RECONNECT_AFTER: Duration = Duration::from_secs(1);

/// How long to wait for the settings before drawing, so the saved theme shows from the start.
const SETTINGS_WAIT: Duration = Duration::from_secs(2);

/// What the main loop reacts to.
enum Message {
    Key(KeyEvent),
    /// Text pasted into the terminal.
    Paste(String),
    /// The terminal changed size, or something else that needs only a redraw.
    Redraw,
    Engine(ApiEvent),
    Connection(Connection),
    Finished(Request, Result<Reply, String>),
}

/// Runs the TUI in the terminal against the engine `client` reaches, until the user quits.
/// Must be called within a multi-threaded tokio runtime.
pub async fn run(client: ApiClient, origin: Origin) -> io::Result<()> {
    let (sender, messages) = mpsc::unbounded_channel();
    let stop_input = Arc::new(AtomicBool::new(false));
    let input = read_input(sender.clone(), stop_input.clone());
    let follower = tokio::spawn(follow(client.clone(), sender.clone()));

    let mut app = App::new(origin, Theme::default());
    app.local = is_local(client.url());
    if let Some(request) = app.load_settings() {
        let loaded = tokio::time::timeout(SETTINGS_WAIT, carry_out(&client, &request)).await;
        let result = loaded.unwrap_or_else(|_| Err("the engine took too long".to_owned()));
        app.finished(&request, result);
    }
    let mut terminal = ratatui::try_init()?;
    // Pasted links arrive whole rather than as keys; not every terminal supports it.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let result = main_loop(&mut terminal, app, &client, sender, messages).await;
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();

    follower.abort();
    stop_input.store(true, Ordering::Relaxed);
    let _ = input.join();
    result
}

async fn main_loop(
    terminal: &mut DefaultTerminal,
    mut app: App,
    client: &ApiClient,
    sender: mpsc::UnboundedSender<Message>,
    mut messages: mpsc::UnboundedReceiver<Message>,
) -> io::Result<()> {
    let mut ticks = tokio::time::interval(Duration::from_secs(1));
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let size = terminal.size()?;
        app.resize(size.width, size.height);
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            _ = ticks.tick() => {
                for request in app.tick() {
                    tokio::spawn(send(client.clone(), request, sender.clone()));
                }
            }
            message = messages.recv() => {
                let Some(message) = message else { return Ok(()) };
                handle(&mut app, client, &sender, message);
                // Take in everything already waiting before drawing again.
                while let Ok(message) = messages.try_recv() {
                    handle(&mut app, client, &sender, message);
                }
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}

fn handle(
    app: &mut App,
    client: &ApiClient,
    sender: &mpsc::UnboundedSender<Message>,
    message: Message,
) {
    let request = match message {
        Message::Key(key) => app.key(key),
        Message::Paste(text) => {
            app.paste(&text);
            None
        }
        Message::Redraw => None,
        Message::Engine(event) => {
            app.apply(event);
            None
        }
        Message::Connection(connection) => {
            app.model.connection = connection;
            None
        }
        Message::Finished(request, result) => app.finished(&request, result),
    };
    match request {
        // Written between frames, so the escape sequence doesn't land inside one.
        Some(Request::Copy { text, what }) => {
            let request = Request::Copy { text, what };
            let result = copy(&request).map(|()| Reply::Done);
            app.finished(&request, result);
        }
        Some(request) => {
            tokio::spawn(send(client.clone(), request, sender.clone()));
        }
        None => {}
    }
}

/// Carries out `request` and reports how it went.
async fn send(client: ApiClient, request: Request, sender: mpsc::UnboundedSender<Message>) {
    let result = carry_out(&client, &request).await;
    let _ = sender.send(Message::Finished(request, result));
}

/// Carries out `request` through the engine's API, or on this machine for opening a folder.
/// Changes to transfers arrive as events, not in the reply.
pub async fn carry_out(client: &ApiClient, request: &Request) -> Result<Reply, String> {
    match request {
        Request::Transfer(target, action) => {
            let id = target.id;
            let result = match action.clone() {
                Action::Start => client.start(id).await.map(drop),
                Action::ForceStart => client.force_start(id).await.map(drop),
                Action::Stop => client.stop(id).await.map(drop),
                Action::Recheck => client.recheck(id).await.map(drop),
                Action::Remove { delete_files } => client.remove(id, delete_files).await,
                Action::Rename(name) => client.rename(id, name).await.map(drop),
                Action::Tags(tags) => client.set_tags(id, tags).await.map(drop),
                Action::Category(category) => client.set_category(id, category).await.map(drop),
                Action::Move(folder) => client.move_storage(id, folder).await.map(drop),
                Action::Queue(step) => client
                    .move_in_queue(id, QueuePlace::Step(step))
                    .await
                    .map(drop),
                Action::Settings(settings) => {
                    client.set_transfer_settings(id, &settings).await.map(drop)
                }
            };
            result.map(|()| Reply::Done).map_err(|e| e.to_string())
        }
        Request::Add(add) => client
            .add(add)
            .await
            .map(|transfer| Reply::Added(transfer.name))
            .map_err(|e| e.to_string()),
        Request::Share(add) => client
            .add(add)
            .await
            .map(|transfer| Reply::Shared(Box::new(transfer)))
            .map_err(|e| e.to_string()),
        Request::Categories => client
            .categories()
            .await
            .map(|categories| Reply::Categories(folders(categories)))
            .map_err(|e| e.to_string()),
        Request::Settings => {
            let (settings, categories) = tokio::join!(client.settings(), client.categories());
            match (settings, categories) {
                (Ok(settings), Ok(categories)) => {
                    Ok(Reply::Loaded(Box::new(settings), folders(categories)))
                }
                (Err(error), _) | (_, Err(error)) => Err(error.to_string()),
            }
        }
        Request::ChangeSettings { changes, .. } => client
            .change_settings(changes)
            .await
            .map(|view| Reply::Settings(Box::new(view)))
            .map_err(|e| e.to_string()),
        Request::ResetSetting { key, .. } => client
            .reset_setting(key)
            .await
            .map(|view| Reply::Settings(Box::new(view)))
            .map_err(|e| e.to_string()),
        Request::SetCategory { name, folder, .. } => client
            .set_category_folder(name, folder.clone())
            .await
            .map(|_| Reply::Done)
            .map_err(|e| e.to_string()),
        Request::RemoveCategory(name) => client
            .remove_category(name)
            .await
            .map(|()| Reply::Done)
            .map_err(|e| e.to_string()),
        Request::Copy { .. } => copy(request).map(|()| Reply::Done),
        Request::Look(id, look) => {
            let reply = match *look {
                Look::Files => client.files(*id).await.map(Reply::Files),
                Look::Peers => client.peers(*id).await.map(Reply::Peers),
                Look::Chunks(cells) => client.chunk_map(*id, cells).await.map(Reply::Chunks),
            };
            reply.map_err(|e| e.to_string())
        }
        Request::Open(folder) => {
            let folder = folder.clone();
            tokio::task::spawn_blocking(move || open(folder))
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                .map(|()| Reply::Done)
        }
    }
}

/// Every category's save folder, by name.
fn folders(
    categories: std::collections::BTreeMap<String, tempest_engine::api::Category>,
) -> std::collections::BTreeMap<String, PathBuf> {
    categories
        .into_iter()
        .map(|(name, category)| (name, category.save_folder))
        .collect()
}

/// Puts a [`Request::Copy`]'s text on the clipboard by asking the terminal (OSC 52), which
/// works over SSH too. Terminals that don't support it ignore the request.
fn copy(request: &Request) -> Result<(), String> {
    use std::io::Write;

    let Request::Copy { text, .. } = request else {
        return Ok(());
    };
    let mut out = io::stdout().lock();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
        .and_then(|()| out.flush())
        .map_err(|error| error.to_string())
}

/// `bytes` in standard base64, with padding.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

/// Opens `folder` in the system's file manager.
fn open(folder: PathBuf) -> Result<(), String> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let status = Command::new(program)
        .arg(&folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    // Explorer exits with 1 even when it opens the folder.
    if status.success() || cfg!(windows) {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

/// Whether the API at `url` is on this machine.
fn is_local(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split('/').next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Follows the engine's event stream, reopening it where it left off whenever it breaks.
async fn follow(client: ApiClient, sender: mpsc::UnboundedSender<Message>) {
    let mut last = None;
    loop {
        let error = match client.events(last).await {
            Err(error) => error.to_string(),
            Ok(mut stream) => {
                let _ = sender.send(Message::Connection(Connection::Live));
                loop {
                    match stream.next().await {
                        Ok(Some((event, number))) => {
                            last = number.or(last);
                            if sender.send(Message::Engine(event)).is_err() {
                                return;
                            }
                        }
                        Ok(None) => break "The engine closed its event stream.".to_owned(),
                        Err(error) => break error.to_string(),
                    }
                }
            }
        };
        if sender
            .send(Message::Connection(Connection::Lost { error }))
            .is_err()
        {
            return;
        }
        tokio::time::sleep(RECONNECT_AFTER).await;
    }
}

/// Reads the terminal's input on a thread of its own, until `stop` is set.
fn read_input(
    sender: mpsc::UnboundedSender<Message>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let message = match event::poll(Duration::from_millis(100)) {
                Ok(false) => continue,
                Err(_) => return,
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => Message::Key(key),
                    Ok(Event::Paste(text)) => Message::Paste(text),
                    Ok(Event::Resize(..)) => Message::Redraw,
                    Ok(_) => continue,
                    Err(_) => return,
                },
            };
            if sender.send(message).is_err() {
                return;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        let vectors = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (text, encoded) in vectors {
            assert_eq!(base64(text.as_bytes()), encoded, "{text}");
        }
    }

    #[test]
    fn loopback_apis_are_local() {
        assert!(is_local("http://127.0.0.1:4776"));
        assert!(is_local("http://localhost:4776/"));
        assert!(is_local("http://[::1]:4776"));
        assert!(!is_local("http://192.168.1.20:4776"));
        assert!(!is_local("https://seedbox.example:4776"));
    }
}
