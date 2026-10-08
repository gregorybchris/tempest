//! The catalog's HTTP endpoints, called through the tracker's router without a socket.

use std::net::SocketAddr;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use tempest_catalog_api::account::sign_in_message;
use tempest_catalog_api::listing::Content;
use tempest_protocol::tracker::{Signature, listing_message};
use tempest_protocol::{BundleId, PeerId};
use tempest_tracker::{Catalog, CatalogConfig, Tracker, TrackerConfig};
use tower::ServiceExt;

const HOST: &str = "tracker.example.org";

struct Key(Ed25519KeyPair);

impl Key {
    fn new(n: u8) -> Self {
        Self(Ed25519KeyPair::from_seed_unchecked(&[n; 32]).unwrap())
    }

    fn id(&self) -> PeerId {
        PeerId::new(self.0.public_key().as_ref().try_into().unwrap())
    }

    fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.0.sign(message).as_ref().try_into().unwrap())
    }
}

fn bundle(n: u8) -> BundleId {
    BundleId::new([n; 32])
}

fn tracker() -> Tracker {
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    Tracker::new(TrackerConfig::default())
        .unwrap()
        .with_catalog(catalog)
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// Sends a request with `headers`, and a JSON body unless it is `Value::Null`.
async fn call(
    router: &Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, HOST);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let body = match body {
        Value::Null => Body::empty(),
        body => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(body.to_string())
        }
    };
    let mut request = request.body(body).unwrap();
    let from: SocketAddr = "198.51.100.9:50000".parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(from));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    assert_eq!(headers[header::CONTENT_TYPE], "application/json");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    Answer {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap(),
    }
}

/// Signs `key` in through the API and returns its `Authorization` header value.
async fn sign_in(router: &Router, key: &Key) -> String {
    let challenge = call(
        router,
        Method::POST,
        "/catalog/v1/challenges",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(challenge.status, StatusCode::OK);
    let code = challenge.body["code"].as_str().unwrap();
    let request = json!({
        "code": code,
        "key": key.id(),
        "sig": key.sign(&sign_in_message(HOST, code)),
    });
    let session = call(router, Method::POST, "/catalog/v1/sessions", &[], request).await;
    assert_eq!(session.status, StatusCode::OK, "{}", session.body);
    format!("Bearer {}", session.body["token"].as_str().unwrap())
}

fn announce_body(key: &Key, bundle: &BundleId, sig: Option<Signature>) -> Value {
    let mut body = json!({
        "bundle": bundle,
        "peer": key.id(),
        "port": 4433,
        "relay": null,
        "event": "started",
        "complete": true
    });
    if let Some(sig) = sig {
        body["listing"] = json!({ "sig": sig });
    }
    body
}

/// Announces `bundle` as `key`, asking for its listing.
async fn announce_listed(router: &Router, key: &Key, bundle: &BundleId) -> Answer {
    let sig = key.sign(&listing_message(bundle));
    let body = announce_body(key, bundle, Some(sig));
    call(router, Method::POST, "/tempest/v1/announce", &[], body).await
}

#[tokio::test]
async fn a_tracker_without_a_catalog_ignores_listings() {
    let router = Tracker::new(TrackerConfig::default()).unwrap().router();
    let info = call(&router, Method::GET, "/catalog/v1/info", &[], Value::Null).await;
    assert_eq!(info.status, StatusCode::NOT_FOUND);
    // Even a bad signature: the field means nothing here.
    let key = Key::new(1);
    let body = announce_body(&key, &bundle(1), Some(Signature([0; 64])));
    let answer = call(&router, Method::POST, "/tempest/v1/announce", &[], body).await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn info_lists_the_categories() {
    let router = tracker().router();
    let info = call(&router, Method::GET, "/catalog/v1/info", &[], Value::Null).await;
    assert_eq!(info.status, StatusCode::OK);
    assert_eq!(info.body["invites_required"], false);
    assert_eq!(info.body["categories"][0]["slug"], "datasets");
}

#[tokio::test]
async fn signing_in_through_a_browser_sets_a_cookie() {
    let router = tracker().router();
    let key = Key::new(1);
    // The browser asks for a challenge and shows its code.
    let challenge = call(
        &router,
        Method::POST,
        "/catalog/v1/challenges",
        &[],
        Value::Null,
    )
    .await;
    let code = challenge.body["code"].as_str().unwrap().to_owned();
    let poll = json!({ "code": code, "poll": challenge.body["poll"] });
    let waiting = call(
        &router,
        Method::POST,
        "/catalog/v1/challenges/poll",
        &[],
        poll.clone(),
    )
    .await;
    assert_eq!(waiting.body, json!({ "session": null }));

    // A signature for another host fails; the user's client signs for this one.
    let signed = |host: &str| json!({ "code": code, "key": key.id(), "sig": key.sign(&sign_in_message(host, &code)) });
    let wrong = call(
        &router,
        Method::POST,
        "/catalog/v1/sessions",
        &[],
        signed("evil.example"),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body["error"], "sign_in_failed");
    let right = call(
        &router,
        Method::POST,
        "/catalog/v1/sessions",
        &[],
        signed(HOST),
    )
    .await;
    assert_eq!(right.status, StatusCode::OK);

    // The browser collects the session as a cookie, which then authenticates it.
    let collected = call(
        &router,
        Method::POST,
        "/catalog/v1/challenges/poll",
        &[],
        poll,
    )
    .await;
    assert_eq!(collected.body["session"]["account"]["key"], json!(key.id()));
    let set_cookie = collected.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    let me = call(
        &router,
        Method::GET,
        "/catalog/v1/me",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["key"], json!(key.id()));

    // Bodies must say they are JSON, which cross-site forms can't.
    let form = Request::builder()
        .method(Method::PUT)
        .uri("/catalog/v1/me")
        .header(header::HOST, HOST)
        .header(header::COOKIE, &cookie)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(r#"{"name":"forged"}"#))
        .unwrap();
    let response = router.clone().oneshot(form).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Signing out clears the cookie and ends the session.
    let out = call(
        &router,
        Method::DELETE,
        "/catalog/v1/session",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert!(
        out.headers[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let me = call(
        &router,
        Method::GET,
        "/catalog/v1/me",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn behind_a_proxy_the_forwarded_host_is_signed() {
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let router = Tracker::new(TrackerConfig::default())
        .unwrap()
        .behind_proxy(true)
        .with_catalog(catalog)
        .router();
    let key = Key::new(1);
    let challenge = call(
        &router,
        Method::POST,
        "/catalog/v1/challenges",
        &[],
        Value::Null,
    )
    .await;
    let code = challenge.body["code"].as_str().unwrap();
    let request = json!({
        "code": code,
        "key": key.id(),
        "sig": key.sign(&sign_in_message("public.example.org", code)),
    });
    let forwarded = [("x-forwarded-host", "public.example.org:443")];
    let session = call(
        &router,
        Method::POST,
        "/catalog/v1/sessions",
        &forwarded,
        request,
    )
    .await;
    assert_eq!(session.status, StatusCode::OK, "{}", session.body);
}

#[tokio::test]
async fn announces_ask_for_listings_with_a_valid_signature() {
    let tracker = tracker();
    let router = tracker.router();
    let alice = Key::new(1);
    let id = bundle(1);

    // Signed by another key: refused, and nothing listed.
    let forged = announce_body(&alice, &id, Some(Key::new(2).sign(&listing_message(&id))));
    let refused = call(&router, Method::POST, "/tempest/v1/announce", &[], forged).await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST);
    assert_eq!(refused.body["error"], "invalid_request");

    let answer = announce_listed(&router, &alice, &id).await;
    assert_eq!(answer.status, StatusCode::OK);
    let catalog = tracker.catalog_handle().unwrap();
    assert_eq!(
        catalog.lock().unwrap().pending().unwrap(),
        [(id, alice.id())]
    );

    // Pending: hidden from others, shown to its publisher.
    let path = format!("/catalog/v1/listings/{id}");
    let hidden = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let auth = sign_in(&router, &alice).await;
    let mine = call(
        &router,
        Method::GET,
        "/catalog/v1/me/listings",
        &[("authorization", &auth)],
        Value::Null,
    )
    .await;
    assert_eq!(mine.body["total"], 1);
    assert_eq!(mine.body["listings"][0]["status"], "pending");

    // Once the manifest is known it is listed, with the live swarm's counts.
    let content = Content {
        name: "weather.csv".into(),
        size: 4096,
        files: 1,
    };
    catalog.lock().unwrap().set_content(&id, &content).unwrap();
    let listing = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(listing.status, StatusCode::OK);
    assert_eq!(listing.body["title"], "weather.csv");
    assert_eq!(listing.body["publisher"]["key"], json!(alice.id()));
    assert_eq!(
        listing.body["swarm"],
        json!({ "complete": 1, "incomplete": 0 })
    );
    let page = call(
        &router,
        Method::GET,
        "/catalog/v1/listings",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(page.body["total"], 1);
}

#[tokio::test]
async fn publishers_describe_listings_and_others_vote_and_comment() {
    let tracker = tracker();
    let router = tracker.router();
    let (alice, bob) = (Key::new(1), Key::new(2));
    let id = bundle(1);
    let path = format!("/catalog/v1/listings/{id}");

    // Signed out, nothing can change.
    let edit = json!({
        "title": "Weather, hourly",
        "description": "Since 1990.",
        "category": "datasets",
        "tags": ["Climate", "open data"]
    });
    let anonymous = call(&router, Method::PUT, &path, &[], edit.clone()).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);

    // Publishing through the API, as `tempest share --publish` will.
    let alice_auth = sign_in(&router, &alice).await;
    let alice_h = [("authorization", alice_auth.as_str())];
    let listing = call(&router, Method::PUT, &path, &alice_h, edit.clone()).await;
    assert_eq!(listing.status, StatusCode::OK, "{}", listing.body);
    assert_eq!(listing.body["status"], "pending");
    assert_eq!(listing.body["tags"], json!(["climate", "open-data"]));
    let content = Content {
        name: "weather".into(),
        size: 1,
        files: 1,
    };
    let catalog = tracker.catalog_handle().unwrap();
    catalog.lock().unwrap().set_content(&id, &content).unwrap();

    let bob_auth = sign_in(&router, &bob).await;
    let bob_h = [("authorization", bob_auth.as_str())];
    let taken = call(&router, Method::PUT, &path, &bob_h, edit).await;
    assert_eq!(taken.status, StatusCode::FORBIDDEN);
    let bad = call(
        &router,
        Method::PUT,
        &path,
        &alice_h,
        json!({ "category": "nope" }),
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // Votes.
    let vote_path = format!("{path}/vote");
    let voted = call(
        &router,
        Method::PUT,
        &vote_path,
        &bob_h,
        json!({ "vote": "up" }),
    )
    .await;
    assert_eq!(voted.body["votes"], json!({ "up": 1, "down": 0 }));
    assert_eq!(voted.body["my_vote"], "up");
    let seen = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(seen.body["my_vote"], "none");

    // Comments.
    let comments_path = format!("{path}/comments");
    let said = call(
        &router,
        Method::POST,
        &comments_path,
        &bob_h,
        json!({ "body": "Thanks!" }),
    )
    .await;
    assert_eq!(said.status, StatusCode::CREATED);
    let comment_id = said.body["id"].as_u64().unwrap();
    let all = call(&router, Method::GET, &comments_path, &[], Value::Null).await;
    assert_eq!(all.body[0]["body"], "Thanks!");
    let delete_path = format!("/catalog/v1/comments/{comment_id}");
    let not_hers = call(&router, Method::DELETE, &delete_path, &alice_h, Value::Null).await;
    assert_eq!(not_hers.status, StatusCode::FORBIDDEN);
    let deleted = call(&router, Method::DELETE, &delete_path, &bob_h, Value::Null).await;
    assert_eq!(deleted.status, StatusCode::OK);

    // Names, and withdrawing.
    let named = call(
        &router,
        Method::PUT,
        "/catalog/v1/me",
        &alice_h,
        json!({ "name": "Alice" }),
    )
    .await;
    assert_eq!(named.body["name"], "Alice");
    let shown = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(shown.body["publisher"]["name"], "Alice");
    let withdrawn = call(&router, Method::DELETE, &path, &alice_h, Value::Null).await;
    assert_eq!(withdrawn.status, StatusCode::OK);
    let gone = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn browsing_filters_and_pages() {
    let tracker = tracker();
    let router = tracker.router();
    let alice = Key::new(1);
    let catalog = tracker.catalog_handle().unwrap();
    for n in 1..=3 {
        announce_listed(&router, &alice, &bundle(n)).await;
        let content = Content {
            name: format!("b{n}"),
            size: 1,
            files: 1,
        };
        catalog
            .lock()
            .unwrap()
            .set_content(&bundle(n), &content)
            .unwrap();
    }
    let page = call(
        &router,
        Method::GET,
        "/catalog/v1/listings?limit=2",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(page.body["total"], 3);
    assert_eq!(page.body["listings"].as_array().unwrap().len(), 2);
    let none = call(
        &router,
        Method::GET,
        "/catalog/v1/listings?category=models",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(none.body["total"], 0);
    let bad = call(
        &router,
        Method::GET,
        "/catalog/v1/listings?limit=-1",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}
