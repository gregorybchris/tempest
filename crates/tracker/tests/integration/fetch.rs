//! A tracker with a catalog lists a bundle once it has fetched the manifest from the swarm,
//! with a sharer that publishes through `tempest-net`'s tracker client.

use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Endpoint, EndpointConfig, FetchLimits, Identity, Node,
    NodeConfig, Storage, StorageConfig, TrackerClient, TrackerError,
};
use tempest_protocol::tree;
use tempest_protocol::{Address, Entry, FileInfo, Manifest};
use tempest_tracker::{Catalog, CatalogConfig, FetcherConfig, Tracker, TrackerConfig};
use tempest_tracker_api::listing::{Content, ListingEdit, ListingStatus};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// A tracker with a catalog on localhost whose fetcher looks often and retries soon. Returns
/// its URL and what stops it.
async fn start_tracker(limits: FetchLimits) -> (String, oneshot::Sender<()>) {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        ..TrackerConfig::default()
    };
    let fetcher = FetcherConfig {
        limits,
        retry: Duration::from_millis(200),
        scan_interval: Duration::from_millis(100),
        endpoint: EndpointConfig::localhost(),
        ..FetcherConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let catalog = Catalog::in_memory(CatalogConfig::default()).unwrap();
    let tracker = Tracker::new(config)
        .unwrap()
        .with_catalog(catalog)
        .fetcher(fetcher);
    tokio::spawn(tracker.serve(listener, async {
        let _ = stopped.await;
    }));
    (url, stop)
}

/// A node on IPv4 loopback sharing a small bundle made in `dir`.
async fn start_sharer(identity: &Identity, dir: &Path) -> (Node, BundleHandle, Arc<Manifest>) {
    let data = b"listed through the catalog\n";
    fs::write(dir.join("readme.txt"), data).unwrap();
    fs::write(dir.join("empty.txt"), b"").unwrap();
    let file = |path: &str, data: &[u8]| {
        let info = FileInfo {
            size: data.len() as u64,
            root: tree::root(data),
            executable: false,
            modified: 1_700_000_000_000,
        };
        Entry::file(path, info)
    };
    let entries = vec![file("readme.txt", data), file("empty.txt", b"")];
    let manifest = Arc::new(Manifest::new("Field notes", "", 0, "test", entries).unwrap());

    let endpoint = Endpoint::bind(identity.clone(), &EndpointConfig::localhost()).unwrap();
    let config = NodeConfig {
        addresses: endpoint
            .local_addrs()
            .into_iter()
            .map(Address::from)
            .collect(),
        ..NodeConfig::default()
    };
    let node = Node::start(endpoint, config);
    let storage = Storage::open_shared(dir, manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let handle = node.share(storage).await.unwrap();
    (node, handle, manifest)
}

fn announce(node: &Node, tracker: &str, handle: &BundleHandle, identity: &Identity) -> Announcer {
    let endpoint = node.endpoint();
    let port = endpoint.local_addrs().iter().map(SocketAddr::port).next();
    let config = AnnounceConfig {
        min_interval: Duration::ZERO,
        ..AnnounceConfig::new(endpoint.peer_id(), port.unwrap())
    };
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let listing = Some(identity.listing(&handle.bundle_id()));
    Announcer::spawn(client, tracker.to_owned(), config, handle, listing)
}

/// The sharer signs in, sets the listing's metadata and announces with `listing`; the tracker
/// fetches the manifest from it and lists the bundle with its content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_published_bundle_is_listed_once_its_manifest_is_fetched() {
    within(async {
        let (tracker, stop) = start_tracker(FetchLimits::default()).await;
        let identity = Identity::generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (node, handle, manifest) = start_sharer(&identity, dir.path()).await;
        let bundle = manifest.bundle_id();

        let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
        let session = client.sign_in(&tracker, &identity, None).await.unwrap();
        assert_eq!(session.account.key, identity.peer_id());
        let edit = ListingEdit {
            title: Some("Field notes, 2026".into()),
            category: Some(Some("documents".into())),
            tags: Some(vec!["Notes".into()]),
            ..ListingEdit::default()
        };
        let listing = client
            .edit_listing(&tracker, &session.token, &bundle, &edit)
            .await
            .unwrap();
        assert_eq!(listing.status, ListingStatus::Pending);
        // Pending listings show only to their publisher.
        let hidden = client.listing(&tracker, None, &bundle).await.unwrap_err();
        assert!(matches!(hidden, TrackerError::Refused { status: 404, .. }));

        let announcer = announce(&node, &tracker, &handle, &identity);
        let listing = loop {
            let listing = client.listing(&tracker, None, &bundle).await;
            if let Ok(listing) = listing {
                break listing;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert_eq!(listing.status, ListingStatus::Listed);
        assert_eq!(listing.title, "Field notes, 2026");
        assert_eq!(listing.category.as_deref(), Some("documents"));
        assert_eq!(listing.tags, ["notes"]);
        assert_eq!(listing.publisher.key, identity.peer_id());
        let content = Content {
            name: "Field notes".into(),
            size: manifest.total_size(),
            files: 2,
        };
        assert_eq!(listing.content, Some(content));
        assert_eq!(listing.swarm.complete, 1);

        client.sign_out(&tracker, &session.token).await.unwrap();
        let signed_out = client.edit_listing(&tracker, &session.token, &bundle, &edit);
        assert!(matches!(
            signed_out.await,
            Err(TrackerError::Refused { status: 401, .. })
        ));

        node.shutdown().await;
        announcer.stop().await;
        let _ = stop.send(());
    })
    .await;
}

/// A manifest over the fetcher's limit is never read, so the listing stays pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manifest_over_the_limit_leaves_the_listing_pending() {
    within(async {
        let limits = FetchLimits {
            max_manifest: 16,
            ..FetchLimits::default()
        };
        let (tracker, stop) = start_tracker(limits).await;
        let identity = Identity::generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (node, handle, manifest) = start_sharer(&identity, dir.path()).await;
        let announcer = announce(&node, &tracker, &handle, &identity);

        let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
        let session = client.sign_in(&tracker, &identity, None).await.unwrap();
        // Long enough for several attempts.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let listing = client
            .listing(&tracker, Some(&session.token), &manifest.bundle_id())
            .await
            .unwrap();
        assert_eq!(listing.status, ListingStatus::Pending);
        assert_eq!(listing.content, None);

        node.shutdown().await;
        announcer.stop().await;
        let _ = stop.send(());
    })
    .await;
}
