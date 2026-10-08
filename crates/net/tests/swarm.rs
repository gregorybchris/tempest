//! Public swarms on localhost: peers that find each other through a tracker and peer exchange.

use std::fs;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempest_core::EventKind;
use tempest_core::config::LingerSettings;
use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Destination, Endpoint, EndpointConfig, Identity, Node,
    NodeConfig, Storage, StorageConfig, TrackerClient,
};
use tempest_protocol::message::PeerRecord;
use tempest_protocol::tree::{self, CHUNK_SIZE};
use tempest_protocol::{Address, Entry, FileInfo, Manifest, PeerId};
use tempest_tracker::{Tracker, TrackerConfig};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};

const TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// A tracker on localhost that asks for an announce every second. Returns its URL and what
/// stops it.
async fn start_tracker() -> (String, oneshot::Sender<()>) {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        ..TrackerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let tracker = Tracker::new(config).unwrap();
    tokio::spawn(tracker.serve(listener, async {
        let _ = stopped.await;
    }));
    (url, stop)
}

/// A node on IPv4 loopback that tells peers its address in Hello, so peer exchange can pass it
/// on. It keeps sharing after it completes: a downloader that lingers only briefly could leave
/// before the third peer, which has no tracker, learns of the seeder from it.
fn start_node() -> Node {
    let mut config = NodeConfig::default();
    config.session.lingering = LingerSettings::KEEP_SHARING;
    let endpoint_config = EndpointConfig {
        max_open_requests: config.session.upload.max_open_requests,
        ..EndpointConfig::localhost()
    };
    let endpoint = Endpoint::bind(Identity::generate().unwrap(), &endpoint_config).unwrap();
    config.addresses = addresses(&endpoint);
    Node::start(endpoint, config)
}

fn addresses(endpoint: &Endpoint) -> Vec<Address> {
    endpoint
        .local_addrs()
        .into_iter()
        .map(Address::from)
        .collect()
}

fn announce(node: &Node, tracker: &str, handle: &BundleHandle) -> Announcer {
    let endpoint = node.endpoint();
    let port = endpoint.local_addrs().iter().map(SocketAddr::port).next();
    let config = AnnounceConfig {
        min_interval: Duration::ZERO,
        ..AnnounceConfig::new(endpoint.peer_id(), port.unwrap())
    };
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    Announcer::spawn(client, tracker.to_owned(), config, handle, None)
}

/// A bundle of a few files, some several chunks long.
fn make_bundle(dir: &Path) -> Arc<Manifest> {
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("a.bin", pattern(CHUNK_SIZE * 40 + 17, 1)),
        ("b/c.bin", pattern(CHUNK_SIZE * 25, 2)),
        ("b/d.txt", b"found through the swarm\n".to_vec()),
    ];
    let entries = files
        .iter()
        .map(|(path, data)| {
            let fs_path = dir.join(path);
            fs::create_dir_all(fs_path.parent().unwrap()).unwrap();
            fs::write(&fs_path, data).unwrap();
            let info = FileInfo {
                size: data.len() as u64,
                root: tree::root(data),
                executable: false,
                modified: 1_700_000_000_000,
            };
            Entry::file(*path, info)
        })
        .collect();
    Arc::new(Manifest::new("swarm", "", 0, "test", entries).unwrap())
}

fn pattern(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|i| (i as u8).wrapping_mul(13).wrapping_add(seed))
        .collect()
}

/// Waits until the bundle has opened a connection with `peer`.
async fn opened_with(events: &mut broadcast::Receiver<tempest_core::Event>, peer: PeerId) {
    loop {
        match events.recv().await {
            Ok(event) if matches!(event.kind, EventKind::Opened { peer: p, .. } if p == peer) => {
                return;
            }
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("never opened with {peer}"),
        }
    }
}

async fn wait_complete(handle: &mut BundleHandle) {
    handle
        .status
        .wait_for(|status| status.complete)
        .await
        .expect("the bundle stopped before completing");
}

fn assert_downloaded(source: &Path, parent: &Path, manifest: &Manifest) {
    for entry in manifest.entries() {
        let path = entry.path.as_str();
        let want = fs::read(source.join(path)).unwrap();
        let got = fs::read(parent.join(manifest.name()).join(path)).unwrap();
        assert!(want == got, "{path} differs");
    }
}

/// Two downloaders find each other through the tracker before anyone has the bundle. A third,
/// which knows only the first and never asks the tracker, learns of the second through peer
/// exchange. Then a seeder announces, the tracker leads the first two to it, and all three
/// download and verify the bundle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_swarm_grows_through_the_tracker_and_peer_exchange() {
    within(async {
        let (tracker, stop_tracker) = start_tracker().await;
        let source = tempfile::tempdir().unwrap();
        let manifest = make_bundle(source.path());
        let bundle = manifest.bundle_id();
        let targets: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let destination = |n: usize| Destination::NewFolderIn(targets[n].path().to_owned());

        let first = start_node();
        let second = start_node();
        let mut first_handle = first
            .download(bundle, destination(0), vec![])
            .await
            .unwrap();
        let first_announcer = announce(&first, &tracker, &first_handle);
        let mut second_handle = second
            .download(bundle, destination(1), vec![])
            .await
            .unwrap();
        let second_announcer = announce(&second, &tracker, &second_handle);
        opened_with(&mut first_handle.events, second.endpoint().peer_id()).await;

        let third = start_node();
        let first_record = PeerRecord {
            peer_id: first.endpoint().peer_id(),
            addresses: addresses(first.endpoint()),
            relay: None,
        };
        let mut third_handle = third
            .download(bundle, destination(2), vec![first_record])
            .await
            .unwrap();
        opened_with(&mut third_handle.events, second.endpoint().peer_id()).await;

        let seeder = start_node();
        let storage =
            Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
                .await
                .unwrap();
        let seeding = seeder.share(storage).await.unwrap();
        let seeder_announcer = announce(&seeder, &tracker, &seeding);

        for handle in [&mut first_handle, &mut second_handle, &mut third_handle] {
            wait_complete(handle).await;
        }
        for (n, target) in targets.iter().enumerate() {
            assert_downloaded(source.path(), target.path(), &manifest);
            assert!(target.path().join("swarm").is_dir(), "peer {n}");
        }

        // Every announcer was heard, and the seeder counted itself complete.
        for announcer in [&first_announcer, &second_announcer, &seeder_announcer] {
            let state = announcer.state.borrow().clone();
            assert!(state.announces >= 1 && state.error.is_none(), "{state:?}");
        }
        assert!(seeder_announcer.state.borrow().complete >= 1);

        for node in [&first, &second, &third, &seeder] {
            node.shutdown().await;
        }
        for announcer in [first_announcer, second_announcer, seeder_announcer] {
            announcer.finished().await;
        }
        // Each sent `stopped` as its bundle stopped.
        let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
        let stats = client.stats(&tracker, &bundle).await.unwrap();
        assert_eq!(stats.complete + stats.incomplete, 0);
        let _ = stop_tracker.send(());
    })
    .await;
}

#[tokio::test]
async fn a_refusing_tracker_is_reported() {
    within(async {
        let config = TrackerConfig {
            allowed: Some(Default::default()),
            ..TrackerConfig::default()
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let tracker = Tracker::new(config).unwrap();
        tokio::spawn(tracker.serve(listener, std::future::pending()));

        let node = start_node();
        let bundle = tempest_protocol::Hash::new([7; 32]);
        let target = tempfile::tempdir().unwrap();
        let handle = node.download(bundle, target.path(), vec![]).await.unwrap();
        let mut announcer = announce(&node, &url, &handle);
        let state = announcer
            .state
            .wait_for(|state| state.error.is_some())
            .await
            .unwrap()
            .clone();
        assert_eq!(state.announces, 0);
        let error = state.error.unwrap();
        assert!(error.contains("403 (bundle_not_allowed"), "{error}");
        // The tracker never heard `started`, so stopping sends nothing and returns at once.
        announcer.stop().await;
        node.shutdown().await;
    })
    .await;
}
