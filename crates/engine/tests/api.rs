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

/// What clients type, as the shared parsing makes it, is what the engine keeps, and the
/// clients' own reading of the settings (`tempest_api::settings`) sees it back.
#[tokio::test(flavor = "multi_thread")]
async fn the_clients_settings_writes_round_trip() {
    use tempest_api::prefs;
    use tempest_api::schedule::Schedule;
    use tempest_api::settings::{SettingsInfo, ValueKind, keys};

    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let app = router(engine.clone(), Access::Local);
    let parse = |kind: ValueKind, text: &str| kind.parse(text).unwrap();
    let weekdays = [true, true, true, true, true, false, false];
    let schedule = Schedule::from_form(weekdays, "9:00", "17:30").unwrap();
    let relay = format!("{}@relay.example.org:4775", "ab".repeat(32));
    let changes = json!({
        keys::DOWNLOAD: parse(ValueKind::Limit, "2.5M"),
        keys::UPLOAD: parse(ValueKind::Limit, "none"),
        keys::ALT_DOWNLOAD: parse(ValueKind::Limit, "500K"),
        keys::ALT_UPLOAD: parse(ValueKind::Limit, "100K"),
        keys::ALT_ENABLED: true,
        keys::ALT_SCHEDULE: schedule,
        keys::MAX_DOWNLOADS: parse(ValueKind::Count, "2"),
        keys::MAX_UPLOADS: parse(ValueKind::Count, "none"),
        keys::RATIO: parse(ValueKind::Ratio, "1.5"),
        keys::TIME: parse(ValueKind::Duration, "12h"),
        keys::ACTION: "remove",
        keys::SAVE_FOLDER: parse(ValueKind::Folder, "/srv/downloads"),
        keys::TRACKERS: ["https://tracker.example.org/announce"],
        keys::RELAYS: [&relay],
        keys::RELAYS_FROM_TRACKERS: false,
        prefs::THEME: "vector",
        prefs::SCANLINES: true,
        prefs::CLEAN: true,
        prefs::COLUMNS: ["size", "tags"],
        prefs::VIEWS: [{ "name": "music", "query": "#music", "sort": "size" }],
    });
    let (status, body) = send(&app, Method::PATCH, "/v1/settings", Some(changes.clone())).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let network = json!({ keys::PORT_MAPPING: false, keys::LAN: false });
    let (status, body) = send(&app, Method::PATCH, "/v1/settings", Some(network)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let explained = json!({ prefs::NETWORK_EXPLAINED: true });
    let (status, body) = send(&app, Method::PATCH, "/v1/settings", Some(explained)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = send(&app, Method::GET, "/v1/settings", None).await;
    let info: SettingsInfo = serde_json::from_value(body).unwrap();
    let settings = &info.settings;
    assert_eq!(settings.limits.download, Some(2_500_000));
    assert_eq!(settings.limits.upload, None);
    let alternative = &settings.limits.alternative;
    assert_eq!(
        (alternative.download, alternative.upload),
        (Some(500_000), Some(100_000))
    );
    assert!(alternative.enabled && info.alternative_limits_active);
    assert_eq!(
        alternative
            .schedule
            .as_ref()
            .map(Schedule::describe)
            .as_deref(),
        Some("Mon–Fri 09:00–17:30")
    );
    assert_eq!(settings.queue.max_active_downloads, Some(2));
    assert_eq!(settings.queue.max_active_uploads, None);
    assert_eq!(settings.sharing.ratio, Some(1.5));
    assert_eq!(settings.sharing.time.as_deref(), Some("12h"));
    assert_eq!(settings.sharing.action, "remove");
    assert_eq!(settings.save_folder.as_deref(), Some("/srv/downloads"));
    assert_eq!(settings.trackers, ["https://tracker.example.org/announce"]);
    assert_eq!(settings.relays.known, [relay]);
    assert!(!settings.relays.from_trackers);
    assert!(!settings.port_mapping.enabled && !settings.lan.enabled);
    let ui = &settings.ui;
    assert!(ui.network_explained);
    assert_eq!(ui.theme, "vector");
    assert!(ui.scanlines && ui.clean);
    assert_eq!(prefs::columns(ui).len(), 2);
    assert_eq!(ui.views[0].query, "#music");
    // Every change is recorded as made through the API, so `x` can undo it.
    let changed = changes.as_object().unwrap();
    assert!(changed.keys().all(|key| info.changed.contains_key(key)));

    // A setting given back to the config file.
    let uri = format!("/v1/settings/{}", keys::DOWNLOAD);
    let (_, body) = send(&app, Method::DELETE, &uri, None).await;
    let info: SettingsInfo = serde_json::from_value(body).unwrap();
    assert_eq!(info.settings.limits.download, None);
    assert!(!info.changed.contains_key(keys::DOWNLOAD));

    // A schedule cleared with the form's empty times.
    let cleared = json!({ keys::ALT_SCHEDULE: Schedule::from_form(weekdays, "", "").unwrap() });
    let (_, body) = send(&app, Method::PATCH, "/v1/settings", Some(cleared)).await;
    let info: SettingsInfo = serde_json::from_value(body).unwrap();
    assert_eq!(info.settings.limits.alternative.schedule, None);
    engine.shutdown().await;
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
        Self::open_at(app, "/v1/events", last_event_id).await
    }

    async fn open_at(app: &Router, uri: &str, last_event_id: Option<u64>) -> Self {
        let mut builder = Request::builder().uri(uri);
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

    // Browsers, which can't set the header on a new stream, resume with a query instead.
    let mut queried = SseReader::open_at(&app, &format!("/v1/events?after={seen}"), None).await;
    assert_eq!(queried.next().await.id, Some(seen + 1));

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

    let bad = Request::builder()
        .uri("/v1/events?after=soon")
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

/// What `share` and `get` can give for one transfer, the API takes (D-228).
#[tokio::test]
async fn a_share_takes_its_own_trackers_and_creation_time() {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(root.path())).await.unwrap();
    let app = router(engine.clone(), Access::Local);
    let shared = root.path().join("notes");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("a.txt"), b"hello").unwrap();
    let tracker = "https://tracker.example.org/announce";

    let body = json!({ "path": shared, "trackers": [tracker], "created": 1_700_000_000_000_i64 });
    let (status, share) = send(&app, Method::POST, "/v1/transfers", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{share}");
    let link: Link = share["link"].as_str().unwrap().parse().unwrap();
    assert_eq!(link.trackers, [tracker]);

    // The creation time is part of the bundle, so another time makes another bundle.
    let other = json!({ "path": shared, "created": 1_700_000_000_001_i64 });
    let (status, other) = send(&app, Method::POST, "/v1/transfers", Some(other)).await;
    assert_eq!(status, StatusCode::CREATED, "{other}");
    assert_ne!(other["id"], share["id"]);

    let bad = json!({ "path": shared, "trackers": ["ftp://nope"], "created": 5 });
    let response = send(&app, Method::POST, "/v1/transfers", Some(bad)).await;
    assert_error(response, StatusCode::BAD_REQUEST, "invalid_request");

    // A download takes trackers too, and names them in its link; a creation time is a share's.
    let body = json!({ "link": nobodys_link(7), "stopped": true, "trackers": [tracker] });
    let (status, download) = send(&app, Method::POST, "/v1/transfers", Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{download}");
    let link: Link = download["link"].as_str().unwrap().parse().unwrap();
    assert_eq!(link.trackers, [tracker]);
    let body = json!({ "link": nobodys_link(8), "created": 5 });
    let response = send(&app, Method::POST, "/v1/transfers", Some(body)).await;
    assert_error(response, StatusCode::BAD_REQUEST, "invalid_request");

    engine.shutdown().await;
}
