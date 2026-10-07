//! The tracker's HTTP endpoints, called through its router without a socket.

use std::net::SocketAddr;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use serde_json::{Value, json};
use tempest_tracker::{Tracker, TrackerConfig};
use tower::ServiceExt;

const BUNDLE: &str = "3f9a000000000000000000000000000000000000000000000000000000000000";
const PEER_A: &str = "a1b2000000000000000000000000000000000000000000000000000000000000";
const PEER_B: &str = "7c21000000000000000000000000000000000000000000000000000000000000";

fn router(behind_proxy: bool) -> Router {
    Tracker::new(TrackerConfig::default())
        .unwrap()
        .behind_proxy(behind_proxy)
        .router()
}

/// Sends a request from `from` and returns the status, the `Retry-After` header and the body.
async fn call(
    router: &Router,
    method: Method,
    path: &str,
    from: &str,
    headers: &[(&str, &str)],
    body: impl Into<Body>,
) -> (StatusCode, Option<String>, Value) {
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(body.into()).unwrap();
    let from: SocketAddr = from.parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(from));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response.headers()[header::CONTENT_TYPE].clone();
    assert_eq!(content_type, "application/json");
    let retry_after = response
        .headers()
        .get(header::RETRY_AFTER)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, retry_after, serde_json::from_slice(&bytes).unwrap())
}

async fn announce(router: &Router, from: &str, body: Value) -> (StatusCode, Option<String>, Value) {
    call(
        router,
        Method::POST,
        "/tempest/v1/announce",
        from,
        &[("content-type", "application/json")],
        body.to_string(),
    )
    .await
}

fn started(peer: &str) -> Value {
    json!({
        "bundle": BUNDLE,
        "peer": peer,
        "port": 4433,
        "relay": null,
        "event": "started",
        "complete": false
    })
}

#[tokio::test]
async fn the_spec_example_works() {
    let router = router(false);
    let (status, _, body) = announce(&router, "198.51.100.9:50000", started(PEER_A)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peers"], json!([]));

    let mut complete = started(PEER_B);
    complete["complete"] = json!(true);
    complete["future_field"] = json!("ignored");
    let (status, _, body) = announce(&router, "[::ffff:203.0.113.5]:50001", complete).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({
            "interval_secs": 300,
            "peers": [
                { "peer": PEER_A, "addrs": ["198.51.100.9:4433"], "relay": null }
            ],
            "complete": 1,
            "incomplete": 1,
            "your_ip": "203.0.113.5",
            "relays": []
        })
    );

    let path = format!("/tempest/v1/bundles/{BUNDLE}");
    let (status, _, body) = call(&router, Method::GET, &path, "192.0.2.1:1", &[], "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "complete": 1, "incomplete": 1 }));
}

#[tokio::test]
async fn malformed_requests_are_invalid() {
    let router = router(false);
    let mut missing_relay = started(PEER_A);
    missing_relay.as_object_mut().unwrap().remove("relay");
    for body in [missing_relay, json!("not an object")] {
        let (status, _, body) = announce(&router, "192.0.2.1:1", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_request");
        assert!(body["message"].as_str().unwrap().len() > 1);
    }

    let path = "/tempest/v1/bundles/3F9A";
    let (status, _, body) = call(&router, Method::GET, path, "192.0.2.1:1", &[], "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
}

#[tokio::test]
async fn oversized_bodies_are_refused() {
    let router = router(false);
    let mut huge = started(PEER_A);
    huge["padding"] = json!("x".repeat(100_000));
    let (status, _, body) = announce(&router, "192.0.2.1:1", huge).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "invalid_request");
}

#[tokio::test]
async fn rate_limits_carry_retry_after() {
    let router = router(false);
    announce(&router, "192.0.2.1:1", started(PEER_A)).await;
    let mut update = started(PEER_A);
    update["event"] = json!("update");
    let (status, retry_after, body) = announce(&router, "192.0.2.1:1", update).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"], "rate_limited");
    let secs = body["retry_after_secs"].as_u64().unwrap();
    assert!((59..=60).contains(&secs));
    assert_eq!(retry_after, Some(secs.to_string()));
}

#[tokio::test]
async fn unknown_endpoints_and_methods_answer_in_json() {
    let router = router(false);
    let (status, _, body) = call(&router, Method::GET, "/nothing", "192.0.2.1:1", &[], "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    let path = "/tempest/v1/announce";
    let (status, _, body) = call(&router, Method::GET, path, "192.0.2.1:1", &[], "").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(body["error"], "method_not_allowed");
}

#[tokio::test]
async fn behind_a_proxy_the_last_forwarded_ip_is_observed() {
    let forwarded = [("x-forwarded-for", "192.0.2.66, 203.0.113.8")];
    for (behind_proxy, expected) in [(true, "203.0.113.8"), (false, "127.0.0.1")] {
        let router = router(behind_proxy);
        let (_, _, body) = call(
            &router,
            Method::POST,
            "/tempest/v1/announce",
            "127.0.0.1:1",
            &forwarded,
            started(PEER_A).to_string(),
        )
        .await;
        assert_eq!(body["your_ip"], expected);
    }
}
