//! The catalog's HTTP endpoints, under [`PREFIX`]. `tempest_catalog_api` defines their bodies.
//!
//! Requests authenticate with a session token: a bearer token in `Authorization`, or the
//! [`SESSION_COOKIE`] a browser gets when it collects a session. Every body must be JSON with a
//! JSON `Content-Type`, which a cross-site form can't send, and the cookie is `SameSite=Strict`;
//! together they keep other sites from acting with a browser's session.

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tempest_catalog_api::account::{AccountEdit, Poll, PollResponse, SignIn, normalize_authority};
use tempest_catalog_api::listing::{ListingEdit, ListingQuery, NewComment, VoteRequest};
use tempest_catalog_api::{PREFIX, SESSION_COOKIE};
use tempest_protocol::tracker::ErrorCode;
use tempest_protocol::{BundleId, PeerId};

use super::unix_now as now;
use crate::server::{Tracker, json, refuse};
use crate::swarms::Refusal;

/// The largest request body accepted: room for the longest description.
const MAX_BODY: usize = 64 * 1024;

type Answer = Result<Response, Refusal>;

/// The catalog's routes, for a tracker that has a catalog.
pub(crate) fn router() -> Router<Tracker> {
    let route = |path: &str| format!("{PREFIX}{path}");
    Router::new()
        .route(&route("info"), get(info))
        .route(&route("challenges"), post(challenge))
        .route(&route("challenges/poll"), post(poll))
        .route(&route("sessions"), post(sign_in))
        .route(&route("session"), delete(sign_out))
        .route(&route("me"), get(me).put(edit_me))
        .route(&route("me/listings"), get(my_listings))
        .route(&route("listings"), get(listings))
        .route(
            &route("listings/{bundle}"),
            get(listing).put(edit_listing).delete(withdraw),
        )
        .route(&route("listings/{bundle}/vote"), put(vote))
        .route(
            &route("listings/{bundle}/comments"),
            get(comments).post(comment),
        )
        .route(&route("comments/{id}"), delete(delete_comment))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}

fn answer(result: Answer) -> Response {
    result.unwrap_or_else(refuse)
}

fn ok(body: &impl serde::Serialize) -> Answer {
    Ok(json(StatusCode::OK, body))
}

/// A JSON body, refused unless its `Content-Type` says JSON.
fn body<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<T, Refusal> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        let message = "the body must be JSON, sent as application/json";
        return Err(Refusal {
            status: 415,
            ..Refusal::new(ErrorCode::InvalidRequest, message)
        });
    }
    let body = body.map_err(|rejection| Refusal {
        status: rejection.status().as_u16(),
        ..Refusal::new(ErrorCode::InvalidRequest, rejection.body_text())
    })?;
    serde_json::from_slice(&body)
        .map_err(|error| Refusal::new(ErrorCode::InvalidRequest, error.to_string()))
}

fn bundle(text: &str) -> Result<BundleId, Refusal> {
    text.parse().map_err(|_| {
        let message = "the bundle ID must be 64 lowercase hexadecimal characters";
        Refusal::new(ErrorCode::InvalidRequest, message)
    })
}

/// The request's session token, from `Authorization` or else the session cookie.
fn token(headers: &HeaderMap) -> Option<&str> {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let cookie = || {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .find_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                (name == SESSION_COOKIE).then_some(value)
            })
    };
    bearer.or_else(cookie).map(str::trim)
}

/// The key of the request's session, which it must have.
fn signed_in(tracker: &Tracker, headers: &HeaderMap) -> Result<PeerId, Refusal> {
    let token = token(headers).ok_or_else(|| Refusal {
        status: 401,
        ..Refusal::new(
            ErrorCode::Other(tempest_catalog_api::codes::UNAUTHORIZED.into()),
            "sign in first",
        )
    })?;
    tracker.catalog().authenticate(now(), token)
}

/// The key of the request's session if it has a valid one. Browsing works signed out.
fn viewer(tracker: &Tracker, headers: &HeaderMap) -> Option<PeerId> {
    let token = token(headers)?;
    tracker.catalog().authenticate(now(), token).ok()
}

/// A `Set-Cookie` value that stores `token` for `max_age` seconds, or clears the cookie.
fn session_cookie(tracker: &Tracker, token: &str, max_age: u64) -> HeaderValue {
    // Behind a proxy, the tracker is reached over HTTPS.
    let secure = if tracker.is_behind_proxy() {
        "; Secure"
    } else {
        ""
    };
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Max-Age={max_age}; Path=/; HttpOnly; SameSite=Strict{secure}"
    );
    HeaderValue::from_str(&cookie).expect("tokens are hexadecimal")
}

async fn info(State(tracker): State<Tracker>) -> Response {
    json(StatusCode::OK, &tracker.catalog().info())
}

async fn challenge(State(tracker): State<Tracker>) -> Response {
    answer(tracker.catalog().challenge(now()).and_then(|c| ok(&c)))
}

async fn sign_in(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let request: SignIn = body(&headers, raw)?;
        let authority = tracker.authority(&headers).ok_or_else(|| {
            Refusal::new(ErrorCode::InvalidRequest, "the request has no Host header")
        })?;
        let authority = normalize_authority(&authority);
        ok(&tracker.catalog().sign_in(now(), &authority, &request)?)
    })())
}

async fn poll(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let request: Poll = body(&headers, raw)?;
        let polled: PollResponse = tracker.catalog().poll(now(), &request)?;
        let mut response = json(StatusCode::OK, &polled);
        if let Some(session) = &polled.session {
            let cookie = session_cookie(&tracker, &session.token, session.expires_in_secs);
            response.headers_mut().insert(header::SET_COOKIE, cookie);
        }
        Ok(response)
    })())
}

async fn sign_out(State(tracker): State<Tracker>, headers: HeaderMap) -> Response {
    if let Some(token) = token(&headers)
        && let Err(refusal) = tracker.catalog().sign_out(token)
    {
        return refuse(refusal);
    }
    let mut response = json(StatusCode::OK, &serde_json::json!({}));
    let cookie = session_cookie(&tracker, "", 0);
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
}

async fn me(State(tracker): State<Tracker>, headers: HeaderMap) -> Response {
    answer((|| {
        let key = signed_in(&tracker, &headers)?;
        ok(&tracker.catalog().account(&key)?)
    })())
}

async fn edit_me(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let key = signed_in(&tracker, &headers)?;
        let edit: AccountEdit = body(&headers, raw)?;
        ok(&tracker.catalog().edit_account(&key, &edit)?)
    })())
}

/// The query string of `GET /catalog/v1/me/listings`.
#[derive(Debug, Deserialize)]
struct PageQuery {
    #[serde(default)]
    offset: u64,
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    ListingQuery::default().limit
}

fn query<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, Refusal> {
    query
        .map(|Query(query)| query)
        .map_err(|rejection| Refusal::new(ErrorCode::InvalidRequest, rejection.body_text()))
}

async fn my_listings(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    page: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    answer((|| {
        let key = signed_in(&tracker, &headers)?;
        let page = query(page)?;
        let mut page = tracker
            .catalog()
            .published_by(&key, page.offset, page.limit)?;
        tracker.add_swarms(&mut page.listings);
        ok(&page)
    })())
}

async fn listings(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    listing_query: Result<Query<ListingQuery>, QueryRejection>,
) -> Response {
    answer((|| {
        let listing_query = query(listing_query)?;
        let viewer = viewer(&tracker, &headers);
        let mut page = tracker
            .catalog()
            .listings(&listing_query, viewer.as_ref())?;
        tracker.add_swarms(&mut page.listings);
        ok(&page)
    })())
}

async fn listing(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let viewer = viewer(&tracker, &headers);
        let mut listing = tracker.catalog().listing(&id, viewer.as_ref())?;
        tracker.add_swarms(std::slice::from_mut(&mut listing));
        ok(&listing)
    })())
}

async fn edit_listing(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let key = signed_in(&tracker, &headers)?;
        let edit: ListingEdit = body(&headers, raw)?;
        let mut listing = tracker.catalog().edit_listing(now(), &id, &key, &edit)?;
        tracker.add_swarms(std::slice::from_mut(&mut listing));
        ok(&listing)
    })())
}

async fn withdraw(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let key = signed_in(&tracker, &headers)?;
        tracker.catalog().withdraw(now(), &id, &key)?;
        ok(&serde_json::json!({}))
    })())
}

async fn vote(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let key = signed_in(&tracker, &headers)?;
        let request: VoteRequest = body(&headers, raw)?;
        let mut listing = tracker.catalog().vote(now(), &id, &key, request.vote)?;
        tracker.add_swarms(std::slice::from_mut(&mut listing));
        ok(&listing)
    })())
}

async fn comments(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let viewer = viewer(&tracker, &headers);
        ok(&tracker.catalog().comments(&id, viewer.as_ref())?)
    })())
}

async fn comment(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
    raw: Result<Bytes, BytesRejection>,
) -> Response {
    answer((|| {
        let id = bundle(&id)?;
        let key = signed_in(&tracker, &headers)?;
        let comment: NewComment = body(&headers, raw)?;
        let comment = tracker.catalog().comment(now(), &id, &key, &comment)?;
        Ok(json(StatusCode::CREATED, &comment))
    })())
}

async fn delete_comment(
    State(tracker): State<Tracker>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    answer((|| {
        let id: u64 = id
            .parse()
            .map_err(|_| Refusal::new(ErrorCode::InvalidRequest, "a comment ID is a number"))?;
        let key = signed_in(&tracker, &headers)?;
        tracker.catalog().delete_comment(id, &key)?;
        ok(&serde_json::json!({}))
    })())
}
