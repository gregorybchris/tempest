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
use tempest_protocol::tracker::{
    API_PREFIX, AnnounceEvent, AnnounceRequest, BundleStats, ErrorCode, listing_message,
};
use tempest_tracker_api::listing::Listing;
use tempest_tracker_api::{account, codes};
use tokio::net::TcpListener;

use crate::catalog::{self, Catalog};
use crate::fetcher::{self, FetcherConfig};
use crate::limits::{Action, CatalogLimits, Limiter};
use crate::swarms::{Refusal, Swarms, TrackerConfig};
use tempest_http::HttpLimits;

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
    fetcher: FetcherConfig,
    behind_proxy: bool,
    public: Option<PublicUrl>,
    limiter: Arc<Mutex<Limiter>>,
    http_limits: HttpLimits,
    pages: Option<Router>,
}

/// The URL clients reach the tracker at, which sign-ins name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicUrl {
    /// Its authority, as [`sign_in_message`](tempest_tracker_api::account::sign_in_message)
    /// takes it.
    authority: String,
    https: bool,
}

/// A public URL that isn't an `http` or `https` URL with a host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the public URL must be an http or https URL, such as https://tracker.example.org")]
pub struct InvalidPublicUrl;

impl Tracker {
    /// A tracker with no swarms yet.
    pub fn new(config: TrackerConfig) -> io::Result<Self> {
        let seed = getrandom::u64()?;
        Ok(Self {
            swarms: Arc::new(Mutex::new(Swarms::new(config, seed))),
            catalog: None,
            fetcher: FetcherConfig::default(),
            behind_proxy: false,
            public: None,
            limiter: Arc::new(Mutex::new(Limiter::new(CatalogLimits::default()))),
            http_limits: HttpLimits::default(),
            pages: None,
        })
    }

    /// The tracker with a catalog, served under `/tempest/v1/` beside the peer-list service.
    /// While it serves, it fetches pending listings' manifests from their swarms (see
    /// [`fetcher`]).
    pub fn with_catalog(mut self, catalog: Catalog) -> Self {
        self.catalog = Some(Arc::new(Mutex::new(catalog)));
        self
    }

    /// How a tracker with a catalog fetches pending listings' manifests.
    pub fn fetcher(mut self, config: FetcherConfig) -> Self {
        self.fetcher = config;
        self
    }

    /// The catalog's rate limits per client IP. Clones made before this keep the old ones.
    pub fn catalog_limits(mut self, limits: CatalogLimits) -> Self {
        self.limiter = Arc::new(Mutex::new(Limiter::new(limits)));
        self
    }

    /// The limits on HTTP connections and how long their requests may take.
    pub fn http_limits(mut self, limits: HttpLimits) -> Self {
        self.http_limits = limits;
        self
    }

    /// Routes to serve beside the API, such as the catalog's web UI. They must not overlap the
    /// API's paths; requests that match neither get the API's JSON 404.
    pub fn with_pages(mut self, pages: Router) -> Self {
        self.pages = Some(pages);
        self
    }

    /// Whether requests come through a reverse proxy, such as one that terminates HTTPS. The
    /// observed IP is then the last one in `X-Forwarded-For`, which the proxy appends.
    pub fn behind_proxy(mut self, behind_proxy: bool) -> Self {
        self.behind_proxy = behind_proxy;
        self
    }

    /// The URL clients reach the tracker at, such as `https://tracker.example.org`. A sign-in
    /// must be signed for its host and port (D-239), whatever the request's headers say, so no
    /// other site can pass on this tracker's codes and sign its visitors in here. Without one,
    /// only clients on the tracker's own machine can sign in.
    pub fn public_url(mut self, url: &str) -> Result<Self, InvalidPublicUrl> {
        let scheme = url
            .split_once("://")
            .map(|(scheme, _)| scheme.to_ascii_lowercase());
        let https = match scheme.as_deref() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err(InvalidPublicUrl),
        };
        let authority = account::authority(url).ok_or(InvalidPublicUrl)?;
        self.public = Some(PublicUrl { authority, https });
        Ok(self)
    }

    /// The tracker's routes. Serve them with connect info, as [`Tracker::serve`] does, so the
    /// announce handler can observe the client's address.
    pub fn router(&self) -> Router {
        let mut router = Router::new()
            .route(&format!("{API_PREFIX}announce"), post(announce))
            .route(&format!("{API_PREFIX}bundles/{{bundle}}"), get(stats))
            .layer(DefaultBodyLimit::max(MAX_BODY));
        if self.catalog.is_some() {
            router = router.merge(catalog::routes::router(self));
        }
        let router = router
            .fallback(not_found)
            .method_not_allowed_fallback(method_not_allowed)
            .with_state(self.clone());
        match &self.pages {
            Some(pages) => router.merge(pages.clone()),
            None => router,
        }
    }

    /// Serves the tracker on `listener` until `shutdown` completes, sweeping out silent peers
    /// as it goes, and with a catalog, fetching pending listings' manifests.
    pub async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> io::Result<()> {
        let mut sweeper = tokio::spawn(self.clone().sweep());
        let mut fetching = match &self.catalog {
            Some(_) => {
                let run = fetcher::run(self.clone(), self.fetcher.clone());
                Some(tokio::spawn(run))
            }
            None => None,
        };
        let fetcher_ended = async {
            match &mut fetching {
                Some(fetching) => fetching.await,
                None => std::future::pending().await,
            }
        };
        let served = tempest_http::serve(listener, self.router(), self.http_limits, shutdown);
        // The sweeper and the fetcher run until aborted; either ending early is a fault.
        let result = tokio::select! {
            () = served => Ok(()),
            ended = &mut sweeper => Err(io::Error::other(match ended {
                Err(error) if error.is_panic() => "the sweeper panicked",
                _ => "the sweeper stopped",
            })),
            ended = fetcher_ended => match ended {
                // It ends by itself only if its endpoint can't be bound.
                Ok(Err(error)) => Err(error),
                Err(error) if error.is_panic() => {
                    Err(io::Error::other("the manifest fetcher panicked"))
                }
                _ => Err(io::Error::other("the manifest fetcher stopped")),
            },
        };
        sweeper.abort();
        if let Some(fetching) = fetching {
            fetching.abort();
        }
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

    pub(crate) fn lock(&self) -> MutexGuard<'_, Swarms> {
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

    /// A bundle's swarm counts now.
    ///
    /// The catalog's routes call this while holding the catalog's lock: the catalog's lock is
    /// always taken before the swarms', never while holding them.
    pub(crate) fn swarm_stats(&self, bundle: &BundleId) -> BundleStats {
        self.lock().stats(Instant::now(), bundle)
    }

    /// Fills in the listings' swarm counts.
    pub(crate) fn add_swarms(&self, listings: &mut [Listing]) {
        let now = Instant::now();
        let swarms = self.lock();
        for listing in listings {
            listing.swarm = swarms.stats(now, &listing.bundle);
        }
    }

    /// Takes one request of `action` from `ip` under the catalog's rate limits.
    pub(crate) fn limit(&self, action: Action, ip: IpAddr) -> Result<(), Refusal> {
        // The limiter is plain data that each check leaves consistent.
        self.limiter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .check(Instant::now(), action, ip)
    }

    /// Whether clients reach the tracker over HTTPS: through a proxy, or at an `https` public
    /// URL.
    pub(crate) fn is_https(&self) -> bool {
        self.behind_proxy || self.public.as_ref().is_some_and(|public| public.https)
    }

    /// The authority a sign-in must be signed for: the public URL's. Without one, the `Host`
    /// the request reached, but only if it names this machine: a client elsewhere could have
    /// been sent by another site with a `Host` of its choosing.
    pub(crate) fn sign_in_authority(&self, headers: &HeaderMap) -> Result<String, Refusal> {
        if let Some(public) = &self.public {
            return Ok(public.authority.clone());
        }
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .map(account::normalize_authority);
        match host {
            Some(host) if is_loopback(&host) => Ok(host),
            _ => {
                let message = "this tracker takes sign-ins only from its own machine until its \
                    operator sets its public URL (tempest-tracker --public-url)";
                Err(Refusal {
                    status: 403,
                    ..Refusal::new(ErrorCode::Other(codes::SIGN_IN_FAILED.into()), message)
                })
            }
        }
    }

    /// The IP address the request came from.
    pub(crate) fn observed_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
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

/// Runs `work` on a blocking thread, as all of the catalog's work must: SQLite and the search
/// index block, and so does waiting for the catalog's lock.
pub(crate) async fn blocking<T: Send + 'static>(
    tracker: Tracker,
    work: impl FnOnce(&Tracker) -> T + Send + 'static,
) -> T {
    match tokio::task::spawn_blocking(move || work(&tracker)).await {
        Ok(value) => value,
        // A blocking task is cancelled only as the runtime shuts down.
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// Whether `authority`'s host is this machine: `localhost` or a loopback address.
fn is_loopback(authority: &str) -> bool {
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(rest),
        None => authority.split(':').next().unwrap_or(authority),
    };
    host == "localhost" || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
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
    let (result, downloaded) = {
        let mut swarms = tracker.lock();
        // A download counts once per peer, when it completes, not when it says so again.
        let downloaded = request.event == AnnounceEvent::Completed
            && !swarms.is_complete(Instant::now(), &request.bundle, &request.peer);
        let result = swarms.announce(Instant::now(), &request, observed);
        (result, downloaded)
    };
    let response = match result {
        Ok(response) => response,
        Err(refusal) => return refuse(refusal),
    };
    if tracker.catalog.is_some() && (listing.is_some() || downloaded) {
        let (bundle, peer, listed) = (request.bundle, request.peer, listing.is_some());
        blocking(tracker, move |tracker| {
            let now = catalog::unix_now();
            let mut catalog = tracker.catalog();
            // The announce itself succeeded; the catalog's work is retried by later announces
            // or lost, as a download count is, and neither is worth refusing it for.
            // Asking again for a bundle the catalog knows costs nothing, so only new ones count
            // against the client's limit. Over it, a later announce asks again.
            let publish = listed
                && match catalog.knows_listing(&bundle) {
                    Ok(known) => known || tracker.limit(Action::Publish, observed).is_ok(),
                    Err(error) => {
                        eprintln!("cannot list {bundle}: {error}");
                        false
                    }
                };
            if publish && let Err(error) = catalog.request_listing(now, &bundle, &peer) {
                eprintln!("cannot list {bundle}: {error}");
            }
            // Fresh peer IDs could otherwise inflate the counts.
            let downloaded = downloaded && tracker.limit(Action::Download, observed).is_ok();
            if downloaded && let Err(error) = catalog.record_download(now, &bundle) {
                eprintln!("cannot count a download of {bundle}: {error}");
            }
        })
        .await;
    }
    json(StatusCode::OK, &response)
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
