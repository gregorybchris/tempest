//! Tempest's terminal UI (D-090): a client of the engine's API and event stream, laid out as
//! `specs/interfaces.md` describes.
//!
//! - [`app`]: the state and what keys do to it, with [`commands`] binding keys to commands.
//! - [`model`]: what the engine has said, kept up to date from its events.
//! - [`finding`]: filtering and sorting the list.
//! - [`ui`]: drawing the state.
//! - [`explain`]: transfer states in plain language.
//! - [`columns`], [`format`] and [`theme`]: how the list fits, numbers read and colors look.
//! - [`input`]: one line of text being typed.
//!
//! [`run`] connects them to a terminal and an engine.

pub mod app;
pub mod columns;
pub mod commands;
pub mod explain;
pub mod finding;
pub mod format;
pub mod input;
pub mod model;
pub mod theme;
pub mod ui;

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
use tempest_engine::api::QueuePlace;
use tempest_engine::api::client::{ApiClient, ApiEvent};
use tokio::sync::mpsc;

use crate::app::{Action, App, Origin, Reply, Request};
use crate::model::Connection;
use crate::theme::Theme;

/// How long to wait before reopening a broken event stream.
const RECONNECT_AFTER: Duration = Duration::from_secs(1);

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

    let mut terminal = ratatui::try_init()?;
    // Pasted links arrive whole rather than as keys; not every terminal supports it.
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let mut app = App::new(origin, Theme::default());
    app.local = is_local(client.url());
    tokio::spawn(send(client.clone(), Request::Categories, sender.clone()));
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
        terminal.draw(|frame| ui::draw(frame, &app))?;
        tokio::select! {
            _ = ticks.tick() => app.tick(),
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
    match message {
        Message::Key(key) => {
            if let Some(request) = app.key(key) {
                tokio::spawn(send(client.clone(), request, sender.clone()));
            }
        }
        Message::Paste(text) => app.paste(&text),
        Message::Redraw => {}
        Message::Engine(event) => app.apply(event),
        Message::Connection(connection) => app.model.connection = connection,
        Message::Finished(request, result) => app.finished(&request, result),
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
            };
            result.map(|()| Reply::Done).map_err(|e| e.to_string())
        }
        Request::Add(add) => client
            .add(add)
            .await
            .map(|transfer| Reply::Added(transfer.name))
            .map_err(|e| e.to_string()),
        Request::Categories => client
            .categories()
            .await
            .map(|categories| Reply::Categories(categories.into_keys().collect()))
            .map_err(|e| e.to_string()),
        Request::Open(folder) => {
            let folder = folder.clone();
            tokio::task::spawn_blocking(move || open(folder))
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
                .map(|()| Reply::Done)
        }
    }
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
    fn loopback_apis_are_local() {
        assert!(is_local("http://127.0.0.1:4776"));
        assert!(is_local("http://localhost:4776/"));
        assert!(is_local("http://[::1]:4776"));
        assert!(!is_local("http://192.168.1.20:4776"));
        assert!(!is_local("https://seedbox.example:4776"));
    }
}
