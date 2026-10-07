//! `GET /v1/events`: the engine's events as server-sent events, resumable with `Last-Event-ID`
//! or, for browsers, whose `EventSource` can't set it on a new stream, `?after=`.

use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures_util::Stream;

use super::error::ApiError;
use super::{DecisionInfo, MissedInfo, RemovedInfo, SnapshotInfo};
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

#[derive(Debug, serde::Deserialize)]
pub(super) struct EventsQuery {
    after: Option<u64>,
}

pub(super) async fn events(
    State(engine): State<Engine>,
    headers: HeaderMap,
    query: Result<Query<EventsQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, ApiError> {
    let Query(query) = query.map_err(|_| ApiError::invalid("after must be an event's number"))?;
    // The header, which a browser's own reconnection sends, wins over the query.
    let after = match headers.get("last-event-id") {
        None => query.after,
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
            .data(to_data(&SnapshotInfo { transfers }))
    }
}

fn numbered(numbered: &Numbered) -> SseEvent {
    let (name, data) = match &numbered.event {
        EngineEvent::Transfer(info) => ("transfer", to_data(info)),
        EngineEvent::Removed { id } => ("removed", to_data(&RemovedInfo { id: *id })),
        EngineEvent::Decision { id, event } => (
            "decision",
            to_data(&DecisionInfo {
                id: *id,
                time: event.time.since_epoch().as_secs_f64(),
                kind: event.kind.name().to_owned(),
                reason: event.kind.reason().map(str::to_owned),
                text: event.kind.to_string(),
            }),
        ),
        EngineEvent::DecisionsMissed { id, count } => (
            "decisions_missed",
            to_data(&MissedInfo {
                id: *id,
                count: *count,
            }),
        ),
    };
    SseEvent::default()
        .id(numbered.id.to_string())
        .event(name)
        .data(data)
}

fn to_data(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("event data serializes")
}
