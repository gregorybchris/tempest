//! The HTTP API, called through its router without sockets: every route, its errors, access
//! control, and resuming the event stream.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, BodyDataStream};
use axum::http::{Method, Request, StatusCode, header};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tempest_engine::api::{Access, router};
use tempest_engine::config::{LanSettings, PeerSettings};
use tempest_engine::{Engine, EngineConfig};
use tempest_protocol::{Hash, Link};
use tower::ServiceExt;

/// How long anything here may take.
const LIMIT: Duration = Duration::from_secs(30);

fn config(dir: &Path) -> EngineConfig {
    EngineConfig {
        peers: PeerSettings {
            port: 0,
            bind: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        },
        lan: LanSettings {
            enabled: false,
            ..LanSettings::default()
        },
        data_dir: Some(dir.join("data")),
        save_folder: Some(dir.join("downloads")),
        ..EngineConfig::default()
    }
}

fn nobodys_link(seed: u8) -> String {
    Link::new(Hash::new([seed; 32])).to_string()
}

/// A request with a JSON body, or none.
fn request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(uri);
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => builder.body(Body::empty()),
    }
    .unwrap()
}

/// Sends a request and returns the status and the JSON body, `null` if there is none.
async fn call(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = match bytes.is_empty() {
        true => Value::Null,
        false => serde_json::from_slice(&bytes).unwrap(),
    };
    (status, body)
}

async fn send(app: &Router, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    call(app, request(method, uri, body)).await
}

/// Asserts that a response is an error with `status` and `code`, in the API's one shape.
#[track_caller]
fn assert_error((status, body): (StatusCode, Value), expected: StatusCode, code: &str) {
    assert_eq!(status, expected, "{body}");
    assert_eq!(body["error"], code, "{body}");
    assert!(
        body["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_route_works_and_fails_in_one_shape() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let app = router(engine.clone(), Access::Local);
    let (get, post, put, patch, delete) = (
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
    );

    let (status, info) = send(&app, get.clone(), "/v1/engine", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["peer_id"], engine.peer_id().to_string());

    // Adding: a download from a link, and a share of a folder.
    let link = nobodys_link(1);
    let (status, download) = send(
        &app,
        post.clone(),
        "/v1/transfers",
        Some(json!({ "link": link, "stopped": true, "tags": ["linux"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{download}");
    assert_eq!(download["status"]["state"], "stopped");
    assert_eq!(download["kind"], "download");
    assert_eq!(download["progress"], 0.0);
    assert_eq!(download["tags"], json!(["linux"]));
    let id = download["id"].as_str().unwrap().to_owned();

    let shared = root.path().join("notes");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("a.txt"), b"hello").unwrap();
    let (status, share) = send(
        &app,
        post.clone(),
        "/v1/transfers",
        Some(json!({ "path": shared, "name": "Notes" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    assert_eq!(share["kind"], "share");
    assert_eq!(share["name"], "Notes");
    assert_eq!(share["progress"], 1.0);
    let share_id = share["id"].as_str().unwrap().to_owned();
    assert!(share["link"].as_str().unwrap().starts_with("tempest:"));

    let add = |body: Value| send(&app, post.clone(), "/v1/transfers", Some(body));
    assert_error(
        add(json!({ "link": link })).await,
        StatusCode::CONFLICT,
        "already_added",
    );
    assert_error(
        add(json!({})).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let both = json!({ "link": nobodys_link(2), "path": "/tmp" });
    assert_error(add(both).await, StatusCode::BAD_REQUEST, "invalid_request");
    let relative = json!({ "path": "relative" });
    assert_error(
        add(relative).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let bad_link = json!({ "link": "magnet:?xt=nope" });
    assert_error(
        add(bad_link).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let unknown = json!({ "link": nobodys_link(2), "colour": "red" });
    assert_error(
        add(unknown).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let not_json = Request::builder()
        .method(post.clone())
        .uri("/v1/transfers")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(json!({ "link": nobodys_link(2) }).to_string()))
        .unwrap();
    assert_error(
        call(&app, not_json).await,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported_media_type",
    );

    // Reading.
    let (status, list) = send(&app, get.clone(), "/v1/transfers", None).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<_> = list.as_array().unwrap().iter().map(|t| &t["id"]).collect();
    assert_eq!(ids, [&json!(id), &json!(share_id)], "in queue order");
    let one = format!("/v1/transfers/{id}");
    let (status, got) = send(&app, get.clone(), &one, None).await;
    assert_eq!((status, &got["id"]), (StatusCode::OK, &json!(id)));
    for field in [
        "size",
        "eta",
        "ratio",
        "download_rate",
        "peers",
        "added",
        "category",
    ] {
        assert!(got.get(field).is_some(), "{field} in {got}");
    }
    let missing = format!("/v1/transfers/{}", Hash::new([9; 32]));
    assert_error(
        send(&app, get.clone(), &missing, None).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_error(
        send(&app, get.clone(), "/v1/transfers/nope", None).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    // Controls.
    let control = |action: &str| format!("/v1/transfers/{id}/{action}");
    let (status, started) = send(&app, post.clone(), &control("start"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(started["status"]["state"], "stopped");
    let (_, forced) = send(&app, post.clone(), &control("force-start"), None).await;
    assert_eq!(forced["forced"], true);
    let (_, stopped) = send(&app, post.clone(), &control("stop"), None).await;
    assert_eq!(stopped["forced"], false);
    assert_eq!(stopped["status"]["state"], "stopped");

    let (_, renamed) = send(
        &app,
        put.clone(),
        &control("name"),
        Some(json!({ "name": "ISO" })),
    )
    .await;
    assert_eq!(renamed["name"], "ISO");
    let (_, tagged) = send(
        &app,
        put.clone(),
        &control("tags"),
        Some(json!({ "tags": ["b", "a"] })),
    )
    .await;
    assert_eq!(tagged["tags"], json!(["a", "b"]));
    let (_, tuned) = send(
        &app,
        put.clone(),
        &control("settings"),
        Some(json!({ "upload.reciprocal_slots": 2 })),
    )
    .await;
    assert_eq!(tuned["id"], json!(id));
    assert_error(
        send(
            &app,
            put.clone(),
            &control("settings"),
            Some(json!({ "upload.nonsense": 2 })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let (_, limited) = send(
        &app,
        put.clone(),
        &control("settings"),
        Some(json!({ "limits.download": 1000, "sharing.action": "remove" })),
    )
    .await;
    assert_eq!(limited["settings"]["limits.download"], 1000);

    // Rechecking needs data; moving needs an absolute folder.
    assert_error(
        send(&app, post.clone(), &control("recheck"), None).await,
        StatusCode::CONFLICT,
        "no_data",
    );
    let (status, rechecked) = send(
        &app,
        post.clone(),
        &format!("/v1/transfers/{share_id}/recheck"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rechecked}");
    let moved_to = root.path().join("moved");
    let (status, moved) = send(
        &app,
        post.clone(),
        &control("move"),
        Some(json!({ "save_folder": moved_to })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["save_folder"], json!(moved_to));
    assert_error(
        send(
            &app,
            post.clone(),
            &control("move"),
            Some(json!({ "save_folder": "relative" })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    // Categories.
    let isos = root.path().join("isos");
    let (status, category) = send(
        &app,
        put.clone(),
        "/v1/categories/isos",
        Some(json!({ "save_folder": isos })),
    )
    .await;
    assert_eq!(
        (status, &category["save_folder"]),
        (StatusCode::OK, &json!(isos))
    );
    let (_, categories) = send(&app, get.clone(), "/v1/categories", None).await;
    assert_eq!(categories, json!({ "isos": { "save_folder": isos } }));
    let (_, categorized) = send(
        &app,
        put.clone(),
        &control("category"),
        Some(json!({ "category": "isos" })),
    )
    .await;
    assert_eq!(categorized["category"], "isos");
    assert_error(
        send(
            &app,
            put.clone(),
            &control("category"),
            Some(json!({ "category": "films" })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "unknown_category",
    );
    let (status, _) = send(&app, delete.clone(), "/v1/categories/isos", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, uncategorized) = send(&app, get.clone(), &one, None).await;
    assert_eq!(uncategorized["category"], Value::Null);

    // The queue.
    let (status, order) = send(
        &app,
        post.clone(),
        &control("queue"),
        Some(json!({ "to": "bottom" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(order[1]["id"], json!(id));
    let (_, order) = send(
        &app,
        post.clone(),
        &control("queue"),
        Some(json!({ "to": 0 })),
    )
    .await;
    assert_eq!(order[0]["id"], json!(id));
    assert_error(
        send(
            &app,
            post.clone(),
            &control("queue"),
            Some(json!({ "to": "sideways" })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    // Settings.
    let (status, settings) = send(&app, get.clone(), "/v1/settings", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(settings["settings"]["queue"]["max_active_downloads"], 3);
    assert_eq!(settings["changed"], json!({}));
    let (status, changed) = send(
        &app,
        patch.clone(),
        "/v1/settings",
        Some(json!({ "queue.max_active_downloads": 1, "api.port": 4999 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["settings"]["queue"]["max_active_downloads"], 1);
    assert_eq!(changed["restart_required"], json!(["api"]));
    assert_error(
        send(
            &app,
            patch.clone(),
            "/v1/settings",
            Some(json!({ "queue.max_active_downloads": 0 })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    assert_error(
        send(
            &app,
            patch.clone(),
            "/v1/settings",
            Some(json!({ "data_dir": "/elsewhere" })),
        )
        .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let (status, reset) = send(&app, delete.clone(), "/v1/settings/api.port", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reset["changed"], json!({ "queue.max_active_downloads": 1 }));
    assert_eq!(reset["restart_required"], json!([]));

    // Removing.
    let (status, body) = send(&app, delete.clone(), &one, None).await;
    assert_eq!((status, body), (StatusCode::NO_CONTENT, Value::Null));
    let share_uri = format!("/v1/transfers/{share_id}?delete_files=true");
    let (status, _) = send(&app, delete.clone(), &share_uri, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!shared.join("a.txt").exists(), "deleted with its files");
    let (_, list) = send(&app, get.clone(), "/v1/transfers", None).await;
    assert_eq!(list, json!([]));
    assert_error(
        send(&app, delete.clone(), &one, None).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let bad_query = format!("{one}?delete_files=perhaps");
    assert_error(
        send(&app, delete.clone(), &bad_query, None).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    // Unknown endpoints and methods.
    assert_error(
        send(&app, get.clone(), "/v2/transfers", None).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_error(
        send(&app, put.clone(), "/v1/transfers", None).await,
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
    );

    // Once the engine stops, the API says so.
    engine.shutdown().await;
    assert_error(
        send(&app, get.clone(), "/v1/settings", None).await,
        StatusCode::SERVICE_UNAVAILABLE,
        "unavailable",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_access_refuses_other_hosts_and_remote_access_needs_the_token() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let with = |headers: &[(header::HeaderName, &str)]| {
        let mut builder = Request::builder().uri("/v1/transfers");
        for (name, value) in headers {
            builder = builder.header(name, *value);
        }
        builder.body(Body::empty()).unwrap()
    };

    // Locally, no token is needed, but the request must be to and from this machine.
    let local = router(engine.clone(), Access::Local);
    for ok in [
        with(&[]),
        with(&[(header::HOST, "127.0.0.1:4776")]),
        with(&[(header::HOST, "localhost:4776")]),
        with(&[
            (header::HOST, "[::1]:4776"),
            (header::ORIGIN, "http://localhost:4776"),
        ]),
    ] {
        assert_eq!(call(&local, ok).await.0, StatusCode::OK);
    }
    for refused in [
        // DNS rebinding: a page on another site whose name now resolves here.
        with(&[(header::HOST, "attacker.example:4776")]),
        // A page on another site sending a request here.
        with(&[
            (header::HOST, "127.0.0.1:4776"),
            (header::ORIGIN, "https://attacker.example"),
        ]),
        with(&[(header::HOST, "127.0.0.1:4776"), (header::ORIGIN, "null")]),
    ] {
        assert_error(
            call(&local, refused).await,
            StatusCode::FORBIDDEN,
            "forbidden",
        );
    }

    // Remotely, every request needs the token, wherever it comes from.
    let remote = router(engine.clone(), Access::Token("s3cret-token".into()));
    for refused in [
        with(&[]),
        with(&[(header::HOST, "127.0.0.1:4776")]),
        with(&[(header::AUTHORIZATION, "Bearer wrong")]),
        with(&[(header::AUTHORIZATION, "s3cret-token")]),
    ] {
        assert_error(
            call(&remote, refused).await,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        );
    }
    let authorized = with(&[
        (header::HOST, "tempest.example.org"),
        (header::AUTHORIZATION, "Bearer s3cret-token"),
    ]);
    assert_eq!(call(&remote, authorized).await.0, StatusCode::OK);
    engine.shutdown().await;
}

/// Reads server-sent events from a response body.
struct SseReader {
    body: BodyDataStream,
    buffer: String,
}

/// One server-sent event.
#[derive(Debug)]
struct Sse {
    id: Option<u64>,
    event: String,
    data: Value,
}

impl SseReader {
    async fn open(app: &Router, last_event_id: Option<u64>) -> Self {
        let mut builder = Request::builder().uri("/v1/events");
        if let Some(id) = last_event_id {
            builder = builder.header("last-event-id", id.to_string());
        }
        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        Self {
            body: response.into_body().into_data_stream(),
            buffer: String::new(),
        }
    }

    /// The next event, skipping keep-alive comments.
    async fn next(&mut self) -> Sse {
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let (mut id, mut event, mut data) = (None, None, None);
                for line in block.lines() {
                    match line.split_once(':') {
                        Some(("id", value)) => id = Some(value.trim().parse().unwrap()),
                        Some(("event", value)) => event = Some(value.trim().to_owned()),
                        Some(("data", value)) => data = Some(value.trim().to_owned()),
                        _ => {}
                    }
                }
                if let (Some(event), Some(data)) = (event, data) {
                    let data = serde_json::from_str(&data).unwrap();
                    return Sse { id, event, data };
                }
                continue;
            }
            let chunk = tokio::time::timeout(LIMIT, self.body.next())
                .await
                .expect("an event in time")
                .expect("the stream goes on")
                .unwrap();
            self.buffer.push_str(std::str::from_utf8(&chunk).unwrap());
        }
    }

    /// The next event named `name`.
    async fn next_named(&mut self, name: &str) -> Sse {
        loop {
            let event = self.next().await;
            if event.event == name {
                return event;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_event_stream_resumes_after_the_last_event_seen() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let app = router(engine.clone(), Access::Local);
    let add = |seed: u8| {
        let body = json!({ "link": nobodys_link(seed), "stopped": true });
        send(&app, Method::POST, "/v1/transfers", Some(body))
    };
    add(1).await;

    // A new stream starts with every transfer, as an event without an ID.
    let mut stream = SseReader::open(&app, None).await;
    let snapshot = stream.next().await;
    assert_eq!(snapshot.event, "snapshot");
    assert_eq!(snapshot.id, None);
    assert_eq!(snapshot.data["transfers"].as_array().unwrap().len(), 1);

    let (_, second) = add(2).await;
    let first_seen = stream.next_named("transfer").await;
    assert_eq!(first_seen.data["id"], second["id"]);
    let seen = first_seen.id.expect("numbered");

    // Events after the last one seen arrive while the client is away...
    let (_, third) = add(3).await;
    let id = third["id"].as_str().unwrap();
    send(
        &app,
        Method::POST,
        &format!("/v1/transfers/{id}/start"),
        None,
    )
    .await;
    let (status, _) = send(&app, Method::DELETE, &format!("/v1/transfers/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    drop(stream);

    // ...and are replayed, in order, when it reconnects with Last-Event-ID.
    let mut resumed = SseReader::open(&app, Some(seen)).await;
    let mut last = seen;
    let mut names = Vec::new();
    loop {
        let event = resumed.next().await;
        let id = event.id.expect("a resumed stream has no snapshot");
        assert!(id > last, "event IDs increase");
        last = id;
        names.push(event.event.clone());
        if event.event == "removed" {
            assert_eq!(event.data["id"], third["id"]);
            break;
        }
    }
    assert_eq!(names.first().map(String::as_str), Some("transfer"));

    // A client whose last event is gone, such as one from before the engine restarted, is
    // told to start over.
    let mut stale = SseReader::open(&app, Some(1_000_000)).await;
    let reset = stale.next().await;
    assert_eq!((reset.event.as_str(), reset.id), ("reset", None));
    assert_eq!(reset.data["transfers"].as_array().unwrap().len(), 2);

    let bad = Request::builder()
        .uri("/v1/events")
        .header("last-event-id", "yesterday")
        .body(Body::empty())
        .unwrap();
    assert_error(
        call(&app, bad).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );

    // Streams end when the engine stops.
    engine.shutdown().await;
    let end = tokio::time::timeout(LIMIT, async {
        while let Some(chunk) = stale.body.next().await {
            chunk.unwrap();
        }
    });
    end.await.expect("the stream ends");
}
