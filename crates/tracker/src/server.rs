//! The tracker's HTTP server: the endpoints of `protocol/trackers.md` over [`Swarms`].

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tempest_protocol::BundleId;
use tempest_protocol::tracker::{API_PREFIX, AnnounceRequest, ErrorCode};
use tokio::net::TcpListener;

use crate::swarms::{Refusal, Swarms, TrackerConfig};

/// The largest request body accepted. An announce with every field at its limit is far smaller.
const MAX_BODY: usize = 16 * 1024;

/// The longest the tracker waits between sweeps for silent peers.
const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// A tracker: its swarms and how it reads requests. Clones share the swarms.
#[derive(Debug, Clone)]
pub struct Tracker {
    swarms: Arc<Mutex<Swarms>>,
    behind_proxy: bool,
}

impl Tracker {
    /// A tracker with no swarms yet.
    pub fn new(config: TrackerConfig) -> io::Result<Self> {
        let seed = getrandom::u64()?;
        Ok(Self {
            swarms: Arc::new(Mutex::new(Swarms::new(config, seed))),
            behind_proxy: false,
        })
    }

    /// Whether requests come through a reverse proxy, such as one that terminates HTTPS. The
    /// observed IP is then the last one in `X-Forwarded-For`, which the proxy appends.
    pub fn behind_proxy(mut self, behind_proxy: bool) -> Self {
        self.behind_proxy = behind_proxy;
        self
    }

    /// The tracker's routes. Serve them with connect info, as [`Tracker::serve`] does, so the
    /// announce handler can observe the client's address.
    pub fn router(&self) -> Router {
        Router::new()
            .route(&format!("{API_PREFIX}announce"), post(announce))
            .route(&format!("{API_PREFIX}bundles/{{bundle}}"), get(stats))
            .fallback(not_found)
            .method_not_allowed_fallback(method_not_allowed)
            .layer(DefaultBodyLimit::max(MAX_BODY))
            .with_state(self.clone())
    }

    /// Serves the tracker on `listener` until `shutdown` completes, sweeping out silent peers
    /// as it goes.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> io::Result<()> {
        let sweeper = tokio::spawn(self.clone().sweep());
        let app = self
            .router()
            .into_make_service_with_connect_info::<SocketAddr>();
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await;
        sweeper.abort();
        result
    }

    /// Forgets silent peers every so often, so swarms nobody announces to again are freed.
    async fn sweep(self) {
        let interval = self.lock().config().interval.min(MAX_SWEEP_INTERVAL);
        let mut tick = tokio::time::interval(interval.max(Duration::from_millis(100)));
        loop {
            tick.tick().await;
            self.lock().sweep(Instant::now());
        }
    }

    fn lock(&self) -> MutexGuard<'_, Swarms> {
        // The swarms stay consistent even if a panic interrupted an update: every method
        // finishes its checks before changing anything.
        self.swarms
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The IP address the request came from.
    fn observed_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
        let forwarded = || {
            let value = headers.get_all("x-forwarded-for").iter().next_back()?;
            let last = value.to_str().ok()?.rsplit(',').next()?;
            last.trim().parse().ok()
        };
        match self.behind_proxy {
            true => forwarded().unwrap_or(peer.ip()),
            false => peer.ip(),
        }
    }
}

async fn announce(
    State(tracker): State<Tracker>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let body = match body {
        Ok(body) => body,
        // Such as a body over the limit.
        Err(rejection) => {
            let refusal = Refusal {
                status: rejection.status().as_u16(),
                ..Refusal::new(ErrorCode::InvalidRequest, rejection.body_text())
            };
            return refuse(refusal);
        }
    };
    let request: AnnounceRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => return refuse(Refusal::new(ErrorCode::InvalidRequest, error.to_string())),
    };
    let observed = tracker.observed_ip(peer, &headers);
    let result = tracker.lock().announce(Instant::now(), &request, observed);
    match result {
        Ok(response) => json(StatusCode::OK, &response),
        Err(refusal) => refuse(refusal),
    }
}

async fn stats(State(tracker): State<Tracker>, Path(bundle): Path<String>) -> Response {
    let Ok(bundle) = bundle.parse::<BundleId>() else {
        let message = "the bundle ID must be 64 lowercase hexadecimal characters";
        return refuse(Refusal::new(ErrorCode::InvalidRequest, message));
    };
    let stats = tracker.lock().stats(Instant::now(), &bundle);
    json(StatusCode::OK, &stats)
}

async fn not_found() -> Response {
    let refusal = Refusal {
        status: 404,
        ..Refusal::new(ErrorCode::Other("not_found".into()), "no such endpoint")
    };
    refuse(refusal)
}

async fn method_not_allowed() -> Response {
    let refusal = Refusal {
        status: 405,
        ..Refusal::new(
            ErrorCode::Other("method_not_allowed".into()),
            "the endpoint doesn't take this method",
        )
    };
    refuse(refusal)
}

fn refuse(refusal: Refusal) -> Response {
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::BAD_REQUEST);
    let mut response = json(status, &refusal.body);
    if let Some(after) = refusal.body.retry_after_secs {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(after));
    }
    response
}

fn json(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let body = serde_json::to_vec(body).expect("the tracker's JSON types serialize");
    let content_type = HeaderValue::from_static("application/json");
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}
