//! Server-rendered Tempest UI prototype: axum + maud + Datastar.
//!
//! - `cargo run --release` serves the live UI on http://127.0.0.1:8083.
//! - `cargo run -- snapshot <theme> <out.html>` writes a static page, used as
//!   the design reference and for screenshots.

mod patch;
mod render;
mod state;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use proto_core::World;
use render::Ui;
use serde::Deserialize;
use state::{Action, AppState, Session};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;

const ADDR: &str = "127.0.0.1:8083";
const DATASTAR_JS: &str = include_str!("../assets/datastar-1.0.4.js");
const TEMPEST_CSS: &str = include_str!("../../shared/tempest.css");
const COOKIE: &str = "tempest_session";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("snapshot") => snapshot(args.get(2).map(String::as_str), args.get(3).map(String::as_str)),
        None | Some("serve") => serve(),
        _ => eprintln!("usage: server-app [serve | snapshot <theme> <out.html>]"),
    }
}

fn snapshot(theme: Option<&str>, out: Option<&str>) {
    let mut world = World::new(42);
    for _ in 0..40 {
        world.tick(250);
    }
    let theme = render::THEMES.iter().find(|(k, _)| Some(*k) == theme).map(|(k, _)| *k).unwrap_or("green");
    let ui = Ui { theme, ..Ui::default() };
    let html = render::page(&world, &ui, maud::html! {}).into_string();
    let html = html.replace("/tempest.css", "../shared/tempest.css");
    std::fs::write(out.unwrap_or("snapshot.html"), html).expect("write snapshot");
}

#[tokio::main]
async fn serve() {
    let state = AppState::start();
    let app = Router::new()
        .route("/", get(page))
        .route("/updates", get(updates))
        .route("/tempest.css", get(|| async { asset("text/css", TEMPEST_CSS) }))
        .route("/datastar.js", get(|| async { asset("text/javascript", DATASTAR_JS) }))
        .route("/select/{i}", post(|s: Ctx, Path(i): Path<usize>| async move { s.apply(Action::Select(i)) }))
        .route("/theme/{t}", post(theme))
        .route("/scanlines", post(|s: Ctx| async move { s.apply(Action::Scanlines) }))
        .route("/series/{key}", post(series))
        .route("/key/{key}", post(key))
        .route("/chunks/hover", post(|s: Ctx, Json(p): Json<CellPayload>| async move { s.apply(Action::ChunkHover(Some(p.cell))) }))
        .route("/chunks/leave", post(|s: Ctx| async move { s.apply(Action::ChunkHover(None)) }))
        .route("/chart/move", post(|s: Ctx, Json(p): Json<XPayload>| async move { s.apply(Action::ChartMove(p.x)) }))
        .route("/chart/down", post(|s: Ctx, Json(p): Json<XPayload>| async move { s.apply(Action::ChartDown(p.x)) }))
        .route("/chart/up", post(|s: Ctx, Json(p): Json<XPayload>| async move { s.apply(Action::ChartUp(p.x)) }))
        .route("/chart/leave", post(|s: Ctx| async move { s.apply(Action::ChartLeave) }))
        .route("/chart/reset", post(|s: Ctx| async move { s.apply(Action::ResetZoom) }))
        // The default predicate skips text/event-stream; the SSE stream compresses ~7× with gzip.
        .layer(CompressionLayer::new().compress_when(SizeAbove::new(256)))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(ADDR).await.expect("bind");
    println!("tempest server-app on http://{ADDR}");
    axum::serve(listener, app).await.expect("serve");
}

fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, content_type)], body)
}

/// A request's app state and session, found by cookie.
struct Ctx {
    app: Arc<AppState>,
    session: Arc<Session>,
}

impl Ctx {
    fn apply(&self, action: Action) -> StatusCode {
        self.session.apply(&self.app.world.read().unwrap(), action);
        StatusCode::NO_CONTENT
    }
}

impl axum::extract::FromRequestParts<Arc<AppState>> for Ctx {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, app: &Arc<AppState>) -> Result<Self, StatusCode> {
        let id = session_id(&parts.headers).ok_or(StatusCode::UNAUTHORIZED)?;
        Ok(Self { app: app.clone(), session: app.session(&id) })
    }
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('=').map(str::to_string))
}

async fn page(State(app): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let (id, new) = match session_id(&headers) {
        Some(id) => (id, false),
        None => (new_session_id(), true),
    };
    let session = app.session(&id);
    let html = {
        let ui = session.ui.lock().unwrap();
        let world = app.world.read().unwrap();
        render::page(&world, &ui, maud::html! { script type="module" src="/datastar.js" {} }).into_string()
    };
    let mut res = Html(html).into_response();
    if new {
        let cookie = format!("{COOKIE}={id}; Path=/; HttpOnly; SameSite=Strict");
        res.headers_mut().insert(header::SET_COOKIE, cookie.parse().unwrap());
    }
    res
}

fn new_session_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!("{nanos:x}")
}

/// The long-lived SSE stream. Wakes on every world tick and every session
/// change, and sends whatever fragments differ from what it last sent.
async fn updates(ctx: Ctx) -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let mut ticks = ctx.app.ticks.clone();
    let mut changed = ctx.session.changed.subscribe();
    let stats = std::env::var_os("TEMPEST_STATS").is_some();
    tokio::spawn(async move {
        let mut patcher = patch::Patcher::default();
        loop {
            let started = Instant::now();
            let events = {
                let tick = *ticks.borrow_and_update();
                let ui = ctx.session.ui.lock().unwrap().clone();
                let world = ctx.app.world.read().unwrap();
                patcher.update(&world, tick, &ui)
            };
            if stats {
                let bytes: usize = events.iter().map(patch::wire_bytes).sum();
                eprintln!("render {:>5} µs, {:>2} events, {:>6} bytes", started.elapsed().as_micros(), events.len(), bytes);
            }
            for e in events {
                if tx.send(Ok(Event::from(e))).await.is_err() {
                    return;
                }
            }
            tokio::select! {
                r = ticks.changed() => if r.is_err() { return },
                r = changed.changed() => if r.is_err() { return },
                _ = tx.closed() => return,
            }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct XPayload {
    x: f64,
}

#[derive(Deserialize)]
struct CellPayload {
    cell: usize,
}

async fn theme(s: Ctx, Path(t): Path<String>) -> StatusCode {
    match render::THEMES.iter().find(|(k, _)| *k == t) {
        Some((k, _)) => s.apply(Action::Theme(k)),
        None => StatusCode::NOT_FOUND,
    }
}

async fn series(s: Ctx, Path(key): Path<String>) -> StatusCode {
    match key.as_str() {
        "down" => s.apply(Action::ToggleSeries("down")),
        "up" => s.apply(Action::ToggleSeries("up")),
        _ => StatusCode::NOT_FOUND,
    }
}

async fn key(s: Ctx, Path(key): Path<String>) -> StatusCode {
    match Action::from_key(&key) {
        Some(a) => s.apply(a),
        None => StatusCode::NOT_FOUND,
    }
}
