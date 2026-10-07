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
use tempest_catalog_api::listing::Listing;
use tempest_protocol::BundleId;
use tempest_protocol::tracker::{API_PREFIX, AnnounceRequest, ErrorCode, listing_message};
use tokio::net::TcpListener;

use crate::catalog::{self, Catalog};
use crate::swarms::{Refusal, Swarms, TrackerConfig};

/// The largest request body accepted. An announce with every field at its limit is far smaller.
const MAX_BODY: usize = 16 * 1024;

/// The longest the tracker waits between sweeps for silent peers.
const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// A tracker: its swarms, its catalog if it has one, and how it reads requests. Clones share
/// the swarms and the catalog.
#[derive(Debug, Clone)]
pub struct Tracker {
    swarms: Arc<Mutex<Swarms>>,
    catalog: Option<Arc<Mutex<Catalog>>>,
    behind_proxy: bool,
}

impl Tracker {
    /// A tracker with no swarms yet.
    pub fn new(config: TrackerConfig) -> io::Result<Self> {
        let seed = getrandom::u64()?;
        Ok(Self {
            swarms: Arc::new(Mutex::new(Swarms::new(config, seed))),
            catalog: None,
            behind_proxy: false,
        })
    }

    /// The tracker with a catalog, served under `/catalog/v1/` beside the peer-list service.
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = Some(Arc::new(Mutex::new(catalog)));
        self
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
        let mut router = Router::new()
            .route(&format!("{API_PREFIX}announce"), post(announce))
            .route(&format!("{API_PREFIX}bundles/{{bundle}}"), get(stats))
            .layer(DefaultBodyLimit::max(MAX_BODY));
        if self.catalog.is_some() {
            router = router.merge(catalog::routes::router());
        }
        router
            .fallback(not_found)
            .method_not_allowed_fallback(method_not_allowed)
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

    /// The catalog, shared, if the tracker has one: for work beside the routes, such as
    /// fetching listed bundles' manifests.
    pub fn catalog_handle(&self) -> Option<Arc<Mutex<Catalog>>> {
        self.catalog.clone()
    }

    /// The catalog. Only its routes call this, and they exist only with a catalog.
    pub(crate) fn catalog(&self) -> MutexGuard<'_, Catalog> {
        let catalog = self
            .catalog
            .as_ref()
            .expect("catalog routes need a catalog");
        // The store's writes are transactions, so a panic leaves nothing half done.
        catalog
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Fills in the listings' swarm counts.
    pub(crate) fn add_swarms(&self, listings: &mut [Listing]) {
        let now = Instant::now();
        let swarms = self.lock();
        for listing in listings {
            listing.swarm = swarms.stats(now, &listing.bundle);
        }
    }

    pub(crate) fn is_behind_proxy(&self) -> bool {
        self.behind_proxy
    }

    /// The authority the client reached the tracker at: `Host`, or behind a proxy the last
    /// `X-Forwarded-Host`, which the proxy sets.
    pub(crate) fn authority(&self, headers: &HeaderMap) -> Option<String> {
        let header = |name: &str| {
            let value = headers.get_all(name).iter().next_back()?.to_str().ok()?;
            Some(value.rsplit(',').next()?.trim().to_owned())
        };
        let forwarded = self
            .behind_proxy
            .then(|| header("x-forwarded-host"))
            .flatten();
        forwarded.or_else(|| header("host"))
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
    // Only a tracker with a catalog reads `listing` (`protocol/trackers.md`).
    let listing = request.listing.filter(|_| tracker.catalog.is_some());
    if let Some(listing) = &listing
        && !catalog::verify(
            &request.peer,
            &listing_message(&request.bundle),
            &listing.sig,
        )
    {
        let message = "the listing signature doesn't verify for this peer and bundle";
        return refuse(Refusal::new(ErrorCode::InvalidRequest, message));
    }
    let observed = tracker.observed_ip(peer, &headers);
    let result = tracker.lock().announce(Instant::now(), &request, observed);
    match result {
        Ok(response) => {
            if listing.is_some() {
                let now = catalog::unix_now();
                let listed = tracker
                    .catalog()
                    .request_listing(now, &request.bundle, &request.peer);
                // The announce itself succeeded; the listing can be asked for again next time.
                if let Err(error) = listed {
                    eprintln!("cannot list {}: {error}", request.bundle);
                }
            }
            json(StatusCode::OK, &response)
        }
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

pub(crate) fn refuse(refusal: Refusal) -> Response {
    let status = StatusCode::from_u16(refusal.status).unwrap_or(StatusCode::BAD_REQUEST);
    let mut response = json(status, &refusal.body);
    if let Some(after) = refusal.body.retry_after_secs {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(after));
    }
    response
}

pub(crate) fn json(status: StatusCode, body: &impl serde::Serialize) -> Response {
    let body = serde_json::to_vec(body).expect("the tracker's JSON types serialize");
    let content_type = HeaderValue::from_static("application/json");
    (status, [(header::CONTENT_TYPE, content_type)], body).into_response()
}
