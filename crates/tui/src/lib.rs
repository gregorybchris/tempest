//! Tempest's terminal UI (D-090): a client of the engine's API and event stream, laid out as
//! `specs/interfaces.md` describes.
//!
//! - [`app`]: the state and what keys do to it.
//! - [`model`]: what the engine has said, kept up to date from its events.
//! - [`ui`]: drawing the state.
//! - [`explain`]: transfer states in plain language.
//! - [`columns`], [`format`] and [`theme`]: how the list fits, numbers read and colors look.
//!
//! [`run`] connects them to a terminal and an engine.

pub mod app;
pub mod columns;
pub mod explain;
pub mod format;
pub mod model;
pub mod theme;
pub mod ui;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use tempest_engine::api::client::{ApiClient, ApiEvent};
use tokio::sync::mpsc;

use crate::app::{App, Origin, Request};
use crate::model::Connection;
use crate::theme::Theme;

/// How long to wait before reopening a broken event stream.
const RECONNECT_AFTER: Duration = Duration::from_secs(1);

/// What the main loop reacts to.
enum Message {
    Key(KeyEvent),
    /// The terminal changed size, or something else that needs only a redraw.
    Redraw,
    Engine(ApiEvent),
    Connection(Connection),
    Finished(Request, Result<(), String>),
}

/// Runs the TUI in the terminal against the engine `client` reaches, until the user quits.
/// Must be called within a multi-threaded tokio runtime.
pub async fn run(client: ApiClient, origin: Origin) -> io::Result<()> {
    let (sender, messages) = mpsc::unbounded_channel();
    let stop_input = Arc::new(AtomicBool::new(false));
    let input = read_input(sender.clone(), stop_input.clone());
    let follower = tokio::spawn(follow(client.clone(), sender.clone()));

    let mut terminal = ratatui::try_init()?;
    let app = App::new(origin, Theme::default());
    let result = main_loop(&mut terminal, app, &client, sender, messages).await;
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
        Message::Redraw => {}
        Message::Engine(event) => app.apply(event),
        Message::Connection(connection) => app.model.connection = connection,
        Message::Finished(request, result) => app.finished(&request, result),
    }
}

/// Sends `request` to the engine and reports how it went. The engine's events bring the
/// change itself.
async fn send(client: ApiClient, request: Request, sender: mpsc::UnboundedSender<Message>) {
    let result = match request {
        Request::Start(id) => client.start(id).await,
        Request::ForceStart(id) => client.force_start(id).await,
        Request::Stop(id) => client.stop(id).await,
    };
    let result = result.map(drop).map_err(|error| error.to_string());
    let _ = sender.send(Message::Finished(request, result));
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
