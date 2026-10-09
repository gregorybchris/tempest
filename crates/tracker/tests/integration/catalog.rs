//! The catalog's HTTP endpoints, called through the tracker's router without a socket.

use std::net::SocketAddr;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use tempest_protocol::tracker::{Signature, listing_message};
use tempest_protocol::{BundleId, PeerId};
use tempest_tracker::limits::Action;
use tempest_tracker::{Catalog, CatalogConfig, CatalogLimits, Rate, Tracker, TrackerConfig};
use tempest_tracker_api::account::sign_in_message;
use tempest_tracker_api::listing::Content;
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
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .with_catalog(catalog)
}

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

/// Sends a request with `headers`, and a JSON body unless it is `Value::Null`. `Host` is
/// [`HOST`] unless `headers` has one.
async fn call(
    router: &Router,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Value,
) -> Answer {
    let mut request = Request::builder().method(method).uri(path);
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        request = request.header(header::HOST, HOST);
    }
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
        "/tempest/v1/session/challenges",
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
    let session = call(router, Method::POST, "/tempest/v1/session", &[], request).await;
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
    let info = call(&router, Method::GET, "/tempest/v1/info", &[], Value::Null).await;
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
    let info = call(&router, Method::GET, "/tempest/v1/info", &[], Value::Null).await;
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
        "/tempest/v1/session/challenges",
        &[],
        Value::Null,
    )
    .await;
    let code = challenge.body["code"].as_str().unwrap().to_owned();
    let poll = json!({ "code": code, "poll": challenge.body["poll"] });
    let waiting = call(
        &router,
        Method::POST,
        "/tempest/v1/session/challenges/poll",
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
        "/tempest/v1/session",
        &[],
        signed("evil.example"),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.body["error"], "sign_in_failed");
    let right = call(
        &router,
        Method::POST,
        "/tempest/v1/session",
        &[],
        signed(HOST),
    )
    .await;
    assert_eq!(right.status, StatusCode::OK);

    // The browser collects the session as a cookie, which then authenticates it.
    let collected = call(
        &router,
        Method::POST,
        "/tempest/v1/session/challenges/poll",
        &[],
        poll,
    )
    .await;
    assert_eq!(collected.body["session"]["account"]["key"], json!(key.id()));
    let set_cookie = collected.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
    // The public URL is HTTPS.
    assert!(set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    let me = call(
        &router,
        Method::GET,
        "/tempest/v1/account",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.body["key"], json!(key.id()));

    // Bodies must say they are JSON, which cross-site forms can't.
    let form = Request::builder()
        .method(Method::PUT)
        .uri("/tempest/v1/account")
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
        "/tempest/v1/session",
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
        "/tempest/v1/account",
        &[("cookie", &cookie)],
        Value::Null,
    )
    .await;
    assert_eq!(me.status, StatusCode::UNAUTHORIZED);
}

/// Asks for a code and signs it in as `key`, signed for `signed_for`, sending `headers`.
/// Returns the challenge and the answer to signing in.
async fn sign_in_as(
    router: &Router,
    key: &Key,
    signed_for: &str,
    headers: &[(&str, &str)],
) -> (Value, Answer) {
    let challenge = call(
        router,
        Method::POST,
        "/tempest/v1/session/challenges",
        headers,
        Value::Null,
    )
    .await;
    let code = challenge.body["code"].as_str().unwrap();
    let request = json!({
        "code": code,
        "key": key.id(),
        "sig": key.sign(&sign_in_message(signed_for, code)),
    });
    let answer = call(
        router,
        Method::POST,
        "/tempest/v1/session",
        headers,
        request,
    )
    .await;
    (challenge.body, answer)
}

#[tokio::test]
async fn sign_ins_name_the_public_url_whatever_the_host_header_says() {
    // A site that passed on this tracker's code to its visitor, whose client signed it for that
    // site, can't sign in here by sending that site's name as the host.
    let router = tracker().behind_proxy(true).router();
    let key = Key::new(1);
    let other = [
        ("host", "other.example"),
        ("x-forwarded-host", "other.example"),
    ];
    let (_, relayed) = sign_in_as(&router, &key, "other.example", &other).await;
    assert_eq!(relayed.status, StatusCode::UNAUTHORIZED);
    assert_eq!(relayed.body["error"], "sign_in_failed");
    assert!(relayed.body["message"].as_str().unwrap().contains(HOST));

    let (_, honest) = sign_in_as(&router, &key, HOST, &other).await;
    assert_eq!(honest.status, StatusCode::OK, "{}", honest.body);
}

#[tokio::test]
async fn without_a_public_url_only_this_machine_signs_in() {
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let router = Tracker::new(TrackerConfig::default())
        .unwrap()
        .with_catalog(catalog)
        .router();
    let key = Key::new(1);
    let (_, remote) = sign_in_as(&router, &key, HOST, &[]).await;
    assert_eq!(remote.status, StatusCode::FORBIDDEN);
    assert!(
        remote.body["message"]
            .as_str()
            .unwrap()
            .contains("--public-url")
    );

    for local in ["127.0.0.1:4777", "localhost:8080", "[::1]:4777"] {
        let (challenge, answer) = sign_in_as(&router, &key, local, &[("host", local)]).await;
        assert_eq!(answer.status, StatusCode::OK, "{local}: {}", answer.body);

        // Over plain HTTP, the session cookie can't be marked Secure.
        let poll = json!({ "code": challenge["code"], "poll": challenge["poll"] });
        let collected = call(
            &router,
            Method::POST,
            "/tempest/v1/session/challenges/poll",
            &[("host", local)],
            poll,
        )
        .await;
        let cookie = collected.headers[header::SET_COOKIE].to_str().unwrap();
        assert!(!cookie.contains("Secure"), "{cookie}");
    }
}

#[test]
fn public_urls_must_be_http() {
    let tracker = || Tracker::new(TrackerConfig::default()).unwrap();
    for bad in [
        "tracker.example.org",
        "ftp://tracker.example.org",
        "https://",
    ] {
        assert!(tracker().public_url(bad).is_err(), "{bad}");
    }
    assert!(
        tracker()
            .public_url("HTTPS://Tracker.example.org:443/")
            .is_ok()
    );
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
    let path = format!("/tempest/v1/listings/{id}");
    let hidden = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND);
    let auth = sign_in(&router, &alice).await;
    let mine = call(
        &router,
        Method::GET,
        "/tempest/v1/account/listings",
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
        "/tempest/v1/listings",
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
    let path = format!("/tempest/v1/listings/{id}");

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
    let delete_path = format!("/tempest/v1/comments/{comment_id}");
    let not_hers = call(&router, Method::DELETE, &delete_path, &alice_h, Value::Null).await;
    assert_eq!(not_hers.status, StatusCode::FORBIDDEN);
    let deleted = call(&router, Method::DELETE, &delete_path, &bob_h, Value::Null).await;
    assert_eq!(deleted.status, StatusCode::OK);

    // Names, and unpublishing.
    let named = call(
        &router,
        Method::PUT,
        "/tempest/v1/account",
        &alice_h,
        json!({ "name": "Alice" }),
    )
    .await;
    assert_eq!(named.body["name"], "Alice");
    let shown = call(&router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(shown.body["publisher"]["name"], "Alice");
    let unpublished = call(&router, Method::DELETE, &path, &alice_h, Value::Null).await;
    assert_eq!(unpublished.status, StatusCode::OK);
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
        "/tempest/v1/listings?limit=2",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(page.body["total"], 3);
    assert_eq!(page.body["listings"].as_array().unwrap().len(), 2);
    let none = call(
        &router,
        Method::GET,
        "/tempest/v1/listings?category=models",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(none.body["total"], 0);
    let bad = call(
        &router,
        Method::GET,
        "/tempest/v1/listings?limit=-1",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

/// Lists bundle `n` for `key`, whose session is `auth`, with the fields in `edit`.
async fn list(tracker: &Tracker, key: &Key, auth: &str, n: u8, edit: Value) {
    let router = tracker.router();
    let id = bundle(n);
    announce_listed(&router, key, &id).await;
    let content = Content {
        name: format!("content-{n}"),
        size: 1,
        files: 1,
    };
    let catalog = tracker.catalog_handle().unwrap();
    catalog.lock().unwrap().set_content(&id, &content).unwrap();
    let path = format!("/tempest/v1/listings/{id}");
    let edited = call(
        &router,
        Method::PUT,
        &path,
        &[("authorization", auth)],
        edit,
    )
    .await;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.body);
}

/// The bundles of a search's listings, by their first byte, and the whole page.
async fn search(router: &Router, query: &str) -> (Vec<u8>, Value) {
    let path = format!("/tempest/v1/listings?{query}");
    let page = call(router, Method::GET, &path, &[], Value::Null).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    let found = page.body["listings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|listing| {
            let id: BundleId = serde_json::from_value(listing["bundle"].clone()).unwrap();
            id.as_bytes()[0]
        })
        .collect();
    (found, page.body)
}

/// Three listings for searching.
async fn weather_and_models() -> (Tracker, Router) {
    let tracker = tracker();
    let router = tracker.router();
    let alice = Key::new(1);
    let auth = sign_in(&router, &alice).await;
    let listings = [
        json!({
            "title": "Global weather archive",
            "description": "Hourly readings from ten thousand stations.",
            "category": "datasets",
            "tags": ["climate", "weather"],
        }),
        json!({
            "title": "Station notes",
            "description": "Notes about maintaining a weather station.",
            "category": "documents",
            "tags": ["climate"],
        }),
        json!({
            "title": "Llama weights",
            "description": "A small language model.",
            "category": "models",
            "tags": ["machine-learning"],
        }),
    ];
    for (n, edit) in (1..).zip(listings) {
        list(&tracker, &alice, &auth, n, edit).await;
    }
    (tracker, router)
}

#[tokio::test]
async fn searching_forgives_typos_and_ranks_titles_first() {
    let (_tracker, router) = weather_and_models().await;
    assert_eq!(search(&router, "q=weather").await.0, [1, 2]);
    assert_eq!(search(&router, "q=wether").await.0, [1, 2]);
    assert_eq!(search(&router, "q=weathr+archve").await.0, [1]);
    // The last word matches as a prefix, so results follow typing.
    assert_eq!(search(&router, "q=lla").await.0, [3]);
    assert_eq!(search(&router, "q=machine%20learning").await.0, [3]);
    assert_eq!(search(&router, "q=content-2").await.0, [2]);
    let (found, page) = search(&router, "q=zebra").await;
    assert!(found.is_empty());
    assert_eq!(page["total"], 0);
}

#[tokio::test]
async fn searching_counts_facets_and_filters_by_tag() {
    let (_tracker, router) = weather_and_models().await;
    let (found, page) = search(&router, "tag=climate").await;
    assert_eq!(found.len(), 2);
    assert_eq!(
        page["facets"],
        json!({
            "categories": [
                { "value": "datasets", "count": 1 },
                { "value": "documents", "count": 1 },
            ],
            "tags": [
                { "value": "climate", "count": 2 },
                { "value": "weather", "count": 1 },
            ],
        })
    );
    // A category narrows the matches but not the category counts.
    let (found, page) = search(&router, "tag=Climate&category=datasets").await;
    assert_eq!(found, [1]);
    assert_eq!(page["facets"]["categories"].as_array().unwrap().len(), 2);
    assert_eq!(page["facets"]["tags"].as_array().unwrap().len(), 2);
    let (found, _) = search(&router, "q=station&category=documents").await;
    assert_eq!(found, [2]);

    let bad = call(
        &router,
        Method::GET,
        "/tempest/v1/listings?tag=c%2B%2B",
        &[],
        Value::Null,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.body["message"].as_str().unwrap().starts_with("tag:"));
}

#[tokio::test]
async fn search_follows_edits_and_unpublishings() {
    let tracker = tracker();
    let router = tracker.router();
    let alice = Key::new(1);
    let auth = sign_in(&router, &alice).await;
    list(
        &tracker,
        &alice,
        &auth,
        1,
        json!({ "title": "Tide tables" }),
    )
    .await;
    assert_eq!(search(&router, "q=tide").await.0, [1]);

    let path = format!("/tempest/v1/listings/{}", bundle(1));
    let headers = [("authorization", auth.as_str())];
    let edit = json!({ "title": "Sea levels" });
    call(&router, Method::PUT, &path, &headers, edit).await;
    assert!(search(&router, "q=tide").await.0.is_empty());
    assert_eq!(search(&router, "q=sea").await.0, [1]);

    call(&router, Method::DELETE, &path, &headers, Value::Null).await;
    assert!(search(&router, "q=sea").await.0.is_empty());
    call(&router, Method::PUT, &path, &headers, json!({})).await;
    assert_eq!(search(&router, "q=sea").await.0, [1]);
}

/// Announces `bundle` as `key`, with `event` and `complete`.
async fn announce(router: &Router, key: &Key, bundle: &BundleId, event: &str, complete: bool) {
    let mut body = announce_body(key, bundle, None);
    body["event"] = json!(event);
    body["complete"] = json!(complete);
    let answer = call(router, Method::POST, "/tempest/v1/announce", &[], body).await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
}

#[tokio::test]
async fn sorts_rank_by_swarm_votes_and_downloads() {
    let (_tracker, router) = weather_and_models().await;
    let (b1, b2, b3) = (bundle(1), bundle(2), bundle(3));
    // Every bundle has its publisher as a complete peer. Completing peers count as downloads,
    // once each; peers that start complete add seeders only.
    announce(&router, &Key::new(20), &b1, "completed", true).await;
    for n in [21, 22] {
        announce(&router, &Key::new(n), &b2, "started", false).await;
        announce(&router, &Key::new(n), &b2, "completed", true).await;
        announce(&router, &Key::new(n), &b2, "completed", true).await;
    }
    for n in [30, 31, 32] {
        announce(&router, &Key::new(n), &b3, "started", true).await;
    }
    let (bob, carol) = (Key::new(2), Key::new(3));
    let (bob_auth, carol_auth) = (sign_in(&router, &bob).await, sign_in(&router, &carol).await);
    for (auth, id, vote) in [
        (&bob_auth, &b1, "up"),
        (&carol_auth, &b1, "up"),
        (&bob_auth, &b3, "down"),
    ] {
        let path = format!("/tempest/v1/listings/{id}/vote");
        let headers = [("authorization", auth.as_str())];
        let voted = call(
            &router,
            Method::PUT,
            &path,
            &headers,
            json!({ "vote": vote }),
        )
        .await;
        assert_eq!(voted.status, StatusCode::OK, "{}", voted.body);
    }

    let (seeders, page) = search(&router, "sort=seeders").await;
    assert_eq!(seeders, [3, 2, 1]);
    assert_eq!(page["listings"][0]["swarm"]["complete"], 4);
    let (trending, page) = search(&router, "sort=trending").await;
    assert_eq!(trending, [2, 1, 3]);
    assert_eq!(page["listings"][0]["downloads"], 2);
    assert_eq!(page["listings"][2]["downloads"], 0);
    assert_eq!(search(&router, "sort=votes").await.0, [1, 2, 3]);
    // Relevance without text is popularity: health, net votes and trending together.
    assert_eq!(search(&router, "").await.0, [1, 2, 3]);
    assert_eq!(search(&router, "sort=votes&q=climate").await.0, [1, 2]);
    assert_eq!(search(&router, "sort=seeders&q=climate").await.0, [2, 1]);
    let (paged, page) = search(&router, "sort=seeders&offset=1&limit=1").await;
    assert_eq!(paged, [2]);
    assert_eq!(page["total"], 3);
}

#[tokio::test]
async fn requests_over_a_clients_limit_are_refused() {
    let mut limits = CatalogLimits::none();
    limits.rates.insert(Action::Challenge, Rate::new(2, 30));
    limits.rates.insert(Action::Browse, Rate::new(1, 60));
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let router = Tracker::new(TrackerConfig::default())
        .unwrap()
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .behind_proxy(true)
        .catalog_limits(limits)
        .with_catalog(catalog)
        .router();
    let challenge = async |client: &str| {
        let from = [("x-forwarded-for", client)];
        call(
            &router,
            Method::POST,
            "/tempest/v1/session/challenges",
            &from,
            Value::Null,
        )
        .await
    };
    // Behind a proxy, clients are told apart by the forwarded address.
    for _ in 0..2 {
        assert_eq!(challenge("203.0.113.5").await.status, StatusCode::OK);
    }
    let refused = challenge("203.0.113.5").await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.body["error"], "rate_limited");
    assert_eq!(refused.body["retry_after_secs"], 30);
    assert_eq!(refused.headers[header::RETRY_AFTER], "30");
    assert_eq!(challenge("203.0.113.6").await.status, StatusCode::OK);

    // Each action has its own bucket.
    let info = async || call(&router, Method::GET, "/tempest/v1/info", &[], Value::Null).await;
    assert_eq!(info().await.status, StatusCode::OK);
    assert_eq!(info().await.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn announces_over_the_limits_succeed_but_list_less() {
    let mut limits = CatalogLimits::none();
    limits.rates.insert(Action::Publish, Rate::new(1, 60));
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let tracker = Tracker::new(TrackerConfig::default())
        .unwrap()
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .catalog_limits(limits)
        .with_catalog(catalog);
    let router = tracker.router();
    let alice = Key::new(1);
    for n in 1..=2 {
        let answer = announce_listed(&router, &alice, &bundle(n)).await;
        assert_eq!(answer.status, StatusCode::OK);
    }
    // Only the first new bundle was listed. Asking again for a known one costs nothing.
    let pending = || {
        let catalog = tracker.catalog_handle().unwrap();
        catalog.lock().unwrap().pending().unwrap()
    };
    assert_eq!(pending(), [(bundle(1), alice.id())]);
    let answer = announce_listed(&router, &alice, &bundle(1)).await;
    assert_eq!(answer.status, StatusCode::OK);
}

#[tokio::test]
async fn reports_reach_the_operator_one_per_key_and_listing() {
    let tracker = tracker();
    let router = tracker.router();
    let (alice, bob) = (Key::new(1), Key::new(2));
    let (alice_auth, bob_auth) = (sign_in(&router, &alice).await, sign_in(&router, &bob).await);
    list(
        &tracker,
        &alice,
        &alice_auth,
        1,
        json!({ "title": "Weather" }),
    )
    .await;
    let path = format!("/tempest/v1/listings/{}/reports", bundle(1));
    let bob_h = [("authorization", bob_auth.as_str())];
    let report = async |reason: &str| {
        let body = json!({ "reason": reason });
        call(&router, Method::POST, &path, &bob_h, body)
            .await
            .status
    };

    let body = json!({ "reason": "spam" });
    let anonymous = call(&router, Method::POST, &path, &[], body.clone()).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(report("  ").await, StatusCode::BAD_REQUEST);
    let missing = format!("/tempest/v1/listings/{}/reports", bundle(9));
    let none = call(&router, Method::POST, &missing, &bob_h, body).await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);

    assert_eq!(report("spam").await, StatusCode::CREATED);
    assert_eq!(report("actually malware").await, StatusCode::CREATED);
    let catalog = tracker.catalog_handle().unwrap();
    let catalog = catalog.lock().unwrap();
    let reports = catalog.reports(None).unwrap();
    assert_eq!(
        reports.len(),
        1,
        "a key's later report replaces its earlier one"
    );
    assert_eq!(reports[0].reason, "actually malware");
    assert_eq!(reports[0].bundle, bundle(1));
    assert_eq!(reports[0].reporter, bob.id());
    assert_eq!(reports[0].publisher, alice.id());
    assert!(!reports[0].blocked);
    assert!(catalog.reports(Some(&bundle(2))).unwrap().is_empty());
}

#[tokio::test]
async fn reports_are_rate_limited() {
    let mut limits = CatalogLimits::none();
    limits.rates.insert(Action::Report, Rate::new(1, 60));
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let tracker = Tracker::new(TrackerConfig::default())
        .unwrap()
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .catalog_limits(limits)
        .with_catalog(catalog);
    let router = tracker.router();
    let (alice, bob) = (Key::new(1), Key::new(2));
    let (alice_auth, bob_auth) = (sign_in(&router, &alice).await, sign_in(&router, &bob).await);
    list(&tracker, &alice, &alice_auth, 1, json!({})).await;
    let path = format!("/tempest/v1/listings/{}/reports", bundle(1));
    let headers = [("authorization", bob_auth.as_str())];
    let body = json!({ "reason": "spam" });
    let first = call(&router, Method::POST, &path, &headers, body.clone()).await;
    assert_eq!(first.status, StatusCode::CREATED);
    let second = call(&router, Method::POST, &path, &headers, body).await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(second.body["error"], "rate_limited");
}

/// The operator's commands run in another process on the catalog's file: a second `Catalog` on
/// the same file stands in for it, and the running tracker must obey at once.
#[tokio::test]
async fn hiding_and_banning_take_effect_in_a_running_tracker() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("catalog.db");
    let catalog = Catalog::open(&file, CatalogConfig::default()).unwrap();
    let tracker = Tracker::new(TrackerConfig::default())
        .unwrap()
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .with_catalog(catalog);
    let router = tracker.router();
    let mut op = Catalog::open(&file, CatalogConfig::default()).unwrap();
    let (alice, bob) = (Key::new(1), Key::new(2));
    let (alice_auth, bob_auth) = (sign_in(&router, &alice).await, sign_in(&router, &bob).await);
    let alice_h = [("authorization", alice_auth.as_str())];
    let bob_h = [("authorization", bob_auth.as_str())];
    let titled = |title: &str| json!({ "title": title });
    list(&tracker, &alice, &alice_auth, 1, titled("Weather")).await;
    list(&tracker, &alice, &alice_auth, 2, titled("Weather models")).await;
    list(&tracker, &bob, &bob_auth, 3, titled("Weather bob")).await;
    let page = |id: u8| format!("/tempest/v1/listings/{}", bundle(id));
    let status = async |auth: &[(&str, &str)], id: u8| {
        let answer = call(&router, Method::GET, &page(id), auth, Value::Null).await;
        answer.status
    };
    let act = async |method: Method, auth: &[(&str, &str)], id: u8, what: &str, body: Value| {
        let path = format!("{}/{what}", page(id));
        call(&router, method, &path, auth, body).await.status
    };
    assert_eq!(search(&router, "q=weather").await.0.len(), 3);

    // Hiding a listing takes it off the pages and out of search, for all but its publisher.
    op.set_hidden(&bundle(1), true).unwrap();
    assert_eq!(status(&[], 1).await, StatusCode::NOT_FOUND);
    assert_eq!(status(&bob_h, 1).await, StatusCode::NOT_FOUND);
    assert_eq!(status(&alice_h, 1).await, StatusCode::OK);
    let (found, results) = search(&router, "q=weather").await;
    assert_eq!(found.len(), 2);
    assert_eq!(results["total"], 2);
    let vote = json!({ "vote": "up" });
    let status_vote = act(Method::PUT, &bob_h, 1, "vote", vote).await;
    assert_eq!(status_vote, StatusCode::NOT_FOUND);
    let comment = json!({ "body": "hi" });
    let status_comment = act(Method::POST, &bob_h, 1, "comments", comment).await;
    assert_eq!(status_comment, StatusCode::NOT_FOUND);
    let reason = json!({ "reason": "x" });
    let status_report = act(Method::POST, &bob_h, 1, "reports", reason).await;
    assert_eq!(status_report, StatusCode::NOT_FOUND);
    op.set_hidden(&bundle(1), false).unwrap();
    assert_eq!(status(&[], 1).await, StatusCode::OK);
    assert_eq!(search(&router, "q=weather").await.0.len(), 3);
    assert!(op.set_hidden(&bundle(7), true).is_err(), "no such listing");

    // Banning a key hides everything it listed, and stops it acting.
    op.set_banned(&alice.id(), true).unwrap();
    assert!(op.is_banned(&alice.id()).unwrap());
    assert_eq!(status(&[], 1).await, StatusCode::NOT_FOUND);
    assert_eq!(status(&[], 2).await, StatusCode::NOT_FOUND);
    assert_eq!(status(&[], 3).await, StatusCode::OK);
    assert_eq!(search(&router, "q=weather").await.0, vec![3]);
    let edit = call(&router, Method::PUT, &page(1), &alice_h, titled("New")).await;
    assert_eq!(edit.status, StatusCode::FORBIDDEN);
    let vote = json!({ "vote": "up" });
    let voted = act(Method::PUT, &alice_h, 3, "vote", vote).await;
    assert_eq!(voted, StatusCode::FORBIDDEN);
    // A banned key's announces list nothing.
    announce_listed(&router, &alice, &bundle(4)).await;
    assert!(!op.knows_listing(&bundle(4)).unwrap());

    // Lifting the ban restores them, but not a listing hidden on its own.
    op.set_hidden(&bundle(2), true).unwrap();
    op.set_banned(&alice.id(), false).unwrap();
    assert_eq!(status(&[], 1).await, StatusCode::OK);
    assert_eq!(status(&[], 2).await, StatusCode::NOT_FOUND);
    assert_eq!(search(&router, "q=weather").await.0.len(), 2);
    let stranger = Key::new(9).id();
    assert!(op.set_banned(&stranger, true).is_err(), "no such account");

    // A restart rebuilds the index without what is hidden.
    let restarted = Catalog::open(&file, CatalogConfig::default()).unwrap();
    let router = Tracker::new(TrackerConfig::default())
        .unwrap()
        .public_url(&format!("https://{HOST}"))
        .unwrap()
        .with_catalog(restarted)
        .router();
    assert_eq!(search(&router, "q=weather").await.0.len(), 2);
}
