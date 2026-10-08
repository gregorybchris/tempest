//! Publishing shares to a tracker (D-129): by `Engine::publish`, by the config's
//! `publish` trackers, and through the API, withdrawing them too; and signing in for a browser
//! with a code.

mod common;

use std::fs;
use std::time::Duration;

use serde_json::{Value, json};
use tempest_engine::api::client::{ApiClient, ClientError};
use tempest_engine::api::{Access, PublishRequest, router};
use tempest_engine::{Engine, EngineError, ShareOptions};
use tempest_net::TrackerError;
use tempest_net::{EndpointConfig, TrackerClient};
use tempest_protocol::BundleId;
use tempest_tracker::{Catalog, CatalogConfig, FetcherConfig, Tracker, TrackerConfig};
use tempest_tracker_api::account::{Challenge, PollResponse};
use tempest_tracker_api::listing::{Listing, ListingEdit, ListingStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use common::{LIMIT, config, contents};

/// A tracker with a catalog on localhost whose fetcher looks often. Returns its URL.
async fn start_tracker() -> String {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        ..TrackerConfig::default()
    };
    let fetcher = FetcherConfig {
        retry: Duration::from_millis(200),
        scan_interval: Duration::from_millis(100),
        endpoint: EndpointConfig::localhost(),
        ..FetcherConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let tracker = Tracker::new(config)
        .unwrap()
        .with_catalog(catalog)
        .fetcher(fetcher);
    tokio::spawn(tracker.serve(listener, std::future::pending()));
    url
}

/// Shares a new folder holding one file of `size` bytes.
async fn share(engine: &Engine, dir: &std::path::Path, name: &str, size: usize) -> BundleId {
    let folder = dir.join(name);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("data.bin"), contents(size, size as u64)).unwrap();
    engine
        .add_path(&folder, ShareOptions::default())
        .await
        .unwrap()
        .id
}

/// Waits until the tracker lists `bundle` to everyone.
async fn listed(tracker: &str, bundle: &BundleId) -> Listing {
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let wait = async {
        loop {
            if let Ok(listing) = client.listing(tracker, None, bundle).await {
                return listing;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(LIMIT, wait)
        .await
        .expect("the bundle was never listed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_published_share_is_listed_with_its_metadata() {
    let tracker = start_tracker().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(dir.path())).await.unwrap();
    let id = share(&engine, dir.path(), "Survey", 1000).await;

    let edit = ListingEdit {
        title: Some("Bird survey".into()),
        description: Some("Counts from the spring survey.".into()),
        category: Some(Some("datasets".into())),
        tags: Some(vec!["birds".into(), "Spring".into()]),
    };
    let listing = engine.publish(id, &tracker, &edit, None).await.unwrap();
    assert_eq!(listing.status, ListingStatus::Pending);
    assert_eq!(listing.publisher.key, engine.peer_id());

    // The tracker starts hearing announces only now, and fetches the manifest from the swarm.
    let listing = listed(&tracker, &id).await;
    assert_eq!(listing.status, ListingStatus::Listed);
    assert_eq!(listing.title, "Bird survey");
    assert_eq!(listing.tags, ["birds", "spring"]);
    let content = listing.content.unwrap();
    assert_eq!(
        (content.name.as_str(), content.size, content.files),
        ("Survey", 1000, 1)
    );

    let unknown = engine.publish(BundleId::new([7; 32]), &tracker, &edit, None);
    assert!(matches!(unknown.await, Err(EngineError::NotFound(_))));
    let invalid = engine.publish(id, "tracker.example.org", &edit, None);
    assert!(matches!(invalid.await, Err(EngineError::InvalidTracker(_))));
    engine.shutdown().await;
}

/// Shares made while the config names a tracker in `publish` are listed there by their
/// announces alone, untitled, and stay listed there after a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_configs_publish_trackers_list_new_shares() {
    let tracker = start_tracker().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.publish = vec![tracker.clone()];
    let engine = Engine::open(config.clone()).await.unwrap();
    let id = share(&engine, dir.path(), "Maps", 500).await;
    let listing = listed(&tracker, &id).await;
    assert_eq!(listing.title, "Maps");
    assert_eq!(listing.publisher.key, engine.peer_id());
    engine.shutdown().await;

    // The transfer remembers where it is listed, whatever the config says now.
    config.publish.clear();
    let engine = Engine::open(config).await.unwrap();
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let stats = async {
        loop {
            let listing = client.listing(&tracker, None, &id).await.unwrap();
            if listing.swarm.complete == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(LIMIT, stats).await.unwrap();
    engine.shutdown().await;
}

/// Posts `body` to the tracker endpoint at `path`, as a browser would, and reads the answer.
async fn post<T: serde::de::DeserializeOwned>(tracker: &str, path: &str, body: Value) -> T {
    let address = tracker.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).await.unwrap();
    let body = body.to_string();
    let request = format!(
        "POST /tempest/v1/{path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{response}");
    serde_json::from_str(body).unwrap()
}

/// A browser asks the tracker for a code; the engine signs it, and the browser collects the
/// session of the engine's key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_engine_signs_in_a_browsers_code() {
    let tracker = start_tracker().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(dir.path())).await.unwrap();

    let challenge: Challenge = post(&tracker, "session/challenges", Value::Null).await;
    let code = challenge.code.to_lowercase().replace('-', " ");
    let account = engine.sign_in(&tracker, &code, None).await.unwrap();
    assert_eq!(account.key, engine.peer_id());

    let poll = json!({ "code": challenge.code, "poll": challenge.poll });
    let collected: PollResponse = post(&tracker, "session/challenges/poll", poll).await;
    assert_eq!(collected.session.unwrap().account.key, engine.peer_id());

    // A used code signs nobody in again.
    let again = engine.sign_in(&tracker, &challenge.code, None).await;
    assert!(matches!(again, Err(EngineError::Tracker { .. })));
    engine.shutdown().await;
}

/// Publishing and then unpublishing through the API withdraws the listing, and the engine
/// forgets the tracker, so later announces don't list it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpublishing_through_the_api_withdraws_the_listing() {
    let tracker = start_tracker().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(config(dir.path())).await.unwrap();
    let id = share(&engine, dir.path(), "Notes", 300).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = router(engine.clone(), Access::Local);
    tokio::spawn(async move { axum::serve(listener, app).await });
    let api = ApiClient::new(&url, None, None).unwrap();

    let request = PublishRequest {
        tracker: tracker.clone(),
        listing: ListingEdit::default(),
        invite: None,
    };
    api.publish(id, &request).await.unwrap();
    listed(&tracker, &id).await;
    let transfer = api.transfer(id).await.unwrap();
    assert!(transfer.published.contains(&tracker));

    let transfer = api.unpublish(id, &tracker).await.unwrap();
    assert!(transfer.published.is_empty());
    let browse = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let gone = |result: Result<Listing, TrackerError>| {
        matches!(result, Err(TrackerError::Refused { status: 404, .. }))
    };
    assert!(gone(browse.listing(&tracker, None, &id).await));
    // Past a few announce intervals, it stays withdrawn.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(gone(browse.listing(&tracker, None, &id).await));

    // Withdrawing again changes nothing; an unknown transfer is refused.
    api.unpublish(id, &tracker).await.unwrap();
    let unknown = api.unpublish(BundleId::new([7; 32]), &tracker).await;
    assert!(matches!(unknown, Err(ClientError::Refused(e)) if e.error == "not_found"));
    engine.shutdown().await;
}
