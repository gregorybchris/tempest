//! `GET /v1/events`: the engine's events as server-sent events, resumable with `Last-Event-ID`.

use std::convert::Infallible;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;
use serde_json::{Value, json};

use super::error::ApiError;
use crate::{Engine, EngineEvent, Numbered, Subscription, SubscriptionError};

/// What a stream sends before the next event.
#[derive(Debug, Clone, Copy)]
enum Intro {
    /// Nothing.
    None,
    /// Every transfer, since the client has no view yet.
    Snapshot,
    /// Every transfer, since events the client asked for are gone.
    Reset,
}

struct EventStream {
    engine: Engine,
    subscription: Subscription,
    intro: Intro,
}

pub(super) async fn events(
    State(engine): State<Engine>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    let after = match headers.get("last-event-id") {
        None => None,
        Some(value) => Some(
            value
                .to_str()
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
                .ok_or_else(|| ApiError::invalid("Last-Event-ID must be an event's number"))?,
        ),
    };
    let subscription = engine.subscribe(after).await?;
    let intro = match (after, subscription.missed()) {
        (None, _) => Intro::Snapshot,
        (Some(_), true) => Intro::Reset,
        (Some(_), false) => Intro::None,
    };
    let state = EventStream {
        engine,
        subscription,
        intro,
    };
    let stream = futures_util::stream::unfold(state, |mut state| async move {
        let event = state.next().await?;
        Some((Ok(event), state))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

impl EventStream {
    /// The next event to send, or `None` once the engine has stopped.
    async fn next(&mut self) -> Option<SseEvent> {
        let intro = std::mem::replace(&mut self.intro, Intro::None);
        match intro {
            Intro::Snapshot => return Some(self.snapshot("snapshot").await),
            Intro::Reset => return Some(self.snapshot("reset").await),
            Intro::None => {}
        }
        match self.subscription.recv().await {
            Ok(event) => Some(numbered(&event)),
            // The client fell behind and events were dropped: start it over.
            Err(SubscriptionError::Lagged(_)) => Some(self.snapshot("reset").await),
            Err(SubscriptionError::Closed) => None,
        }
    }

    /// Every transfer, as an event with no ID, so a client that reconnects resumes from the
    /// last numbered event it saw.
    async fn snapshot(&self, name: &str) -> SseEvent {
        let transfers = self.engine.list().await;
        SseEvent::default()
            .event(name)
            .data(json!({ "transfers": transfers }).to_string())
    }
}

fn numbered(numbered: &Numbered) -> SseEvent {
    let (name, data): (&str, Value) = match &numbered.event {
        EngineEvent::Transfer(info) => ("transfer", json!(info)),
        EngineEvent::Removed { id } => ("removed", json!({ "id": id.to_string() })),
        EngineEvent::Decision { id, event } => (
            "decision",
            json!({
                "id": id.to_string(),
                "time": event.time.since_epoch().as_secs_f64(),
                "kind": event.kind.name(),
                "reason": event.kind.reason(),
                "text": event.kind.to_string(),
            }),
        ),
        EngineEvent::DecisionsMissed { id, count } => (
            "decisions_missed",
            json!({ "id": id.to_string(), "count": count }),
        ),
    };
    SseEvent::default()
        .id(numbered.id.to_string())
        .event(name)
        .data(data.to_string())
}
