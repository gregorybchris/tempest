//! Nodes reaching each other through a relay on localhost: the seeder registers with its home
//! relay, and the downloader knows only the seeder's relay address. Unless they hole-punch, the
//! connection stays relayed.

use std::collections::HashSet;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tempest_core::event::CloseCause;
use tempest_core::io::PathKind;
use tempest_core::{Event, EventKind};
use tempest_net::{
    AnnounceConfig, Announcer, BundleHandle, Destination, Endpoint, EndpointConfig, Identity, Node,
    NodeConfig, RelayChoice, RelayError, RelayStatus, Storage, StorageConfig, TrackerClient,
};
use tempest_protocol::message::{GoodbyeReason, PeerRecord};
use tempest_protocol::relay::{RelayHost, RelayRejectReason};
use tempest_protocol::tracker::{AnnounceEvent, AnnounceRequest};
use tempest_protocol::tree::{self, CHUNK_SIZE};
use tempest_protocol::{Address, Entry, FileInfo, Manifest, PeerId, RelayIdentity};
use tempest_relay::{Relay, RelayConfig};
use tempest_tracker::{Tracker, TrackerConfig};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot};

/// How long any step may take before it counts as hung.
const TIMEOUT: Duration = Duration::from_secs(30);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// A relay serving in the background until dropped.
struct TestRelay {
    relay: Arc<Relay>,
    _stop: oneshot::Sender<()>,
}

impl TestRelay {
    fn start(config: RelayConfig) -> Self {
        let relay = Arc::new(Relay::bind(&Identity::generate().unwrap(), config).unwrap());
        let (stop, stopped) = oneshot::channel();
        let serving = relay.clone();
        tokio::spawn(async move {
            serving
                .serve(async {
                    let _ = stopped.await;
                })
                .await;
        });
        Self { relay, _stop: stop }
    }

    fn identity(&self) -> RelayIdentity {
        let addr = self.relay.local_addr().unwrap();
        RelayIdentity {
            key: self.relay.key(),
            host: RelayHost::Ip(addr.ip()),
            port: addr.port(),
        }
    }
}

/// A node on localhost that doesn't hole-punch, so connections through a relay stay relayed:
/// on localhost, every punch would succeed.
fn start_node(identity: Identity) -> Node {
    let config = NodeConfig {
        punch_time: None,
        ..NodeConfig::default()
    };
    start_node_with(identity, config)
}

fn start_node_with(identity: Identity, config: NodeConfig) -> Node {
    let endpoint_config = EndpointConfig {
        max_open_requests: config.session.upload.max_open_requests,
        ..EndpointConfig::localhost()
    };
    Node::start(Endpoint::bind(identity, &endpoint_config).unwrap(), config)
}

/// Gives `node` `relay` as its only relay, and waits until the home relay settles.
async fn use_relay(node: &Node, relay: &TestRelay) -> RelayStatus {
    let home = node.endpoint().home_relay();
    let mut status = home.subscribe();
    node.endpoint().set_relays(RelayChoice {
        known: vec![relay.identity()],
        from_trackers: false,
    });
    let settled = status.wait_for(|status| {
        matches!(
            status,
            RelayStatus::Registered { .. } | RelayStatus::Unavailable { .. }
        )
    });
    within(settled).await.unwrap().clone()
}

/// A bundle of two files, one larger than a maximal Request.
fn sample_bundle(dir: &Path) -> Arc<Manifest> {
    let files = [
        ("small.txt", b"through the relay\n".to_vec()),
        (
            "big.bin",
            (0..CHUNK_SIZE * 80 + 77).map(|i| i as u8).collect(),
        ),
    ];
    let entries = files
        .iter()
        .map(|(path, data)| {
            fs::write(dir.join(path), data).unwrap();
            let info = FileInfo {
                size: data.len() as u64,
                root: tree::root(data),
                executable: false,
                modified: 1_700_000_000_000,
            };
            Entry::file(*path, info)
        })
        .collect();
    Arc::new(Manifest::new("bundle", "", 0, "test", entries).unwrap())
}

async fn share(node: &Node, dir: &Path) -> BundleHandle {
    let manifest = sample_bundle(dir);
    let storage = Storage::open_shared(dir, manifest, StorageConfig::default())
        .await
        .unwrap();
    node.share(storage).await.unwrap()
}

/// Downloads from `seeder` and checks the files arrived intact.
async fn download(node: &Node, seeder: &BundleHandle, source: &Path, record: PeerRecord) {
    let out = tempfile::tempdir().unwrap();
    let handle = node
        .download(
            seeder.bundle_id(),
            Destination::Dir(out.path().into()),
            vec![record],
        )
        .await
        .unwrap();
    let mut status = handle.status.clone();
    within(status.wait_for(|status| status.complete))
        .await
        .unwrap();
    handle.shutdown();
    let summary = within(handle.finished()).await.unwrap();
    assert!(summary.complete);
    for name in ["small.txt", "big.bin"] {
        let want = fs::read(source.join(name)).unwrap();
        assert!(fs::read(out.path().join(name)).unwrap() == want, "{name}");
    }
}

/// The paths of the connections opened so far, from the events that announced them.
fn paths(events: &mut broadcast::Receiver<Event>) -> HashSet<PathKind> {
    let mut paths = HashSet::new();
    while let Ok(event) = events.try_recv() {
        if let EventKind::Connected { path, .. } = event.kind {
            paths.insert(path);
        }
    }
    paths
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bundle_transfers_through_the_seeders_relay() {
    let relay = TestRelay::start(RelayConfig::localhost());
    // The seeder advertises no direct address: the relay is the only way in.
    let seeder = start_node(Identity::generate().unwrap());
    let RelayStatus::Registered {
        relay: home,
        relay_address,
        observed_address,
    } = use_relay(&seeder, &relay).await
    else {
        panic!("the seeder did not register");
    };
    assert_eq!(home, relay.identity());
    assert_eq!(seeder.endpoint().relay_address(), Some(relay_address));
    assert_eq!(
        observed_address,
        Address::from(seeder.endpoint().local_addrs()[0])
    );

    let dir = tempfile::tempdir().unwrap();
    let shared = share(&seeder, dir.path()).await;
    let mut seeder_events = shared.events.resubscribe();
    let downloader = start_node(Identity::generate().unwrap());
    let record = PeerRecord {
        peer_id: seeder.endpoint().peer_id(),
        addresses: Vec::new(),
        relay: Some(relay_address),
    };
    download(&downloader, &shared, dir.path(), record).await;

    // The seeder accepted the connection on a virtual address through its relay.
    assert_eq!(
        paths(&mut seeder_events),
        HashSet::from([PathKind::Relayed])
    );
    downloader.shutdown().await;
    seeder.shutdown().await;
    assert_eq!(seeder.endpoint().home_relay().status(), RelayStatus::Off);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_downloader_marks_its_connection_relayed() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let seeder = start_node(Identity::generate().unwrap());
    let relay_address = use_relay(&seeder, &relay).await.relay_address().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let shared = share(&seeder, dir.path()).await;

    let downloader = start_node(Identity::generate().unwrap());
    let out = tempfile::tempdir().unwrap();
    let record = PeerRecord {
        peer_id: seeder.endpoint().peer_id(),
        addresses: Vec::new(),
        relay: Some(relay_address),
    };
    let mut handle = downloader
        .download(
            shared.bundle_id(),
            Destination::Dir(out.path().into()),
            vec![record],
        )
        .await
        .unwrap();
    let mut status = handle.status.clone();
    within(status.wait_for(|status| status.relayed == 1 && status.connections == 1))
        .await
        .unwrap();
    within(status.wait_for(|status| status.complete))
        .await
        .unwrap();
    assert_eq!(
        paths(&mut handle.events),
        HashSet::from([PathKind::Relayed])
    );
    downloader.shutdown().await;
    seeder.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_is_reported_and_the_node_works_without_a_relay() {
    let others = HashSet::from([PeerId::new([7; 32])]);
    let relay = TestRelay::start(RelayConfig {
        allowlist: Some(others),
        ..RelayConfig::localhost()
    });
    let seeder = start_node(Identity::generate().unwrap());
    let RelayStatus::Unavailable { failures } = use_relay(&seeder, &relay).await else {
        panic!("the relay did not refuse");
    };
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].relay, relay.identity());
    assert!(matches!(
        failures[0].error,
        RelayError::Rejected {
            reason: RelayRejectReason::NotAllowed,
            ..
        }
    ));
    assert_eq!(seeder.endpoint().relay_address(), None);

    // Direct connections still work.
    let dir = tempfile::tempdir().unwrap();
    let shared = share(&seeder, dir.path()).await;
    let downloader = start_node(Identity::generate().unwrap());
    let record = PeerRecord {
        peer_id: seeder.endpoint().peer_id(),
        addresses: seeder
            .endpoint()
            .local_addrs()
            .into_iter()
            .map(Address::from)
            .collect(),
        relay: None,
    };
    download(&downloader, &shared, dir.path(), record).await;
    downloader.shutdown().await;
    seeder.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_with_the_wrong_key_is_not_used() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let node = start_node(Identity::generate().unwrap());
    let mut status = node.endpoint().home_relay().subscribe();
    let impostor = RelayIdentity {
        key: PeerId::new([9; 32]),
        ..relay.identity()
    };
    node.endpoint().set_relays(RelayChoice {
        known: vec![impostor],
        from_trackers: false,
    });
    let settled = within(status.wait_for(|s| matches!(s, RelayStatus::Unavailable { .. })))
        .await
        .unwrap()
        .clone();
    let RelayStatus::Unavailable { failures } = settled else {
        unreachable!()
    };
    assert!(
        matches!(failures[0].error, RelayError::Connect(_)),
        "{failures:?}"
    );
    assert_eq!(relay.relay.registrations(), 0);
    node.shutdown().await;
}

/// A tracker on localhost that recommends `relays`. Returns its URL and what stops it.
async fn start_tracker(relays: Vec<RelayIdentity>) -> (String, oneshot::Sender<()>) {
    let config = TrackerConfig {
        interval: Duration::from_secs(1),
        min_interval: Duration::ZERO,
        relays,
        ..TrackerConfig::default()
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(Tracker::new(config).unwrap().serve(listener, async {
        let _ = stopped.await;
    }));
    (url, stop)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_a_tracker_recommends_becomes_home_and_is_announced() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let (tracker, _stop) = start_tracker(vec![relay.identity()]).await;
    let seeder = start_node(Identity::generate().unwrap());
    let mut status = seeder.endpoint().home_relay().subscribe();
    seeder.endpoint().set_relays(RelayChoice {
        known: Vec::new(),
        from_trackers: true,
    });
    let dir = tempfile::tempdir().unwrap();
    let shared = share(&seeder, dir.path()).await;
    let port = seeder.endpoint().local_addrs()[0].port();
    let config = AnnounceConfig {
        min_interval: Duration::ZERO,
        ..AnnounceConfig::new(seeder.endpoint().peer_id(), port)
    };
    let client = TrackerClient::new(Duration::from_secs(5)).unwrap();
    let announcer = Announcer::spawn(client.clone(), tracker.clone(), config, &shared, None);

    // The first announce brings the recommendation; the home relay registers with it.
    let registered = within(status.wait_for(|s| matches!(s, RelayStatus::Registered { .. })))
        .await
        .unwrap()
        .clone();
    let relay_address = registered.relay_address().unwrap();

    // The announcer tells the tracker the new relay address, which other peers then hear.
    let request = AnnounceRequest {
        bundle: shared.bundle_id(),
        peer: PeerId::new([3; 32]),
        port: 9,
        addrs: Vec::new(),
        relay: None,
        event: AnnounceEvent::Started,
        complete: false,
        num_want: 50,
        listing: None,
    };
    let heard = async {
        loop {
            let response = client.announce(&tracker, &request).await.unwrap();
            assert_eq!(response.relays, [relay.identity()]);
            let seeder_entry = response
                .peers
                .iter()
                .find(|entry| entry.peer == seeder.endpoint().peer_id());
            if seeder_entry.is_some_and(|entry| entry.relay == Some(relay_address)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    within(heard).await;
    announcer.stop().await;
    seeder.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relayed_transfer_finishes_over_a_direct_connection() {
    let relay = TestRelay::start(RelayConfig::localhost());
    let seeder = start_node_with(Identity::generate().unwrap(), NodeConfig::default());
    let relay_address = use_relay(&seeder, &relay).await.relay_address().unwrap();
    // Slow enough that requests are still open when the relayed connection closes.
    seeder.limits().upload.set_rate(Some(2_000_000));
    let dir = tempfile::tempdir().unwrap();
    let shared = share(&seeder, dir.path()).await;

    let downloader = start_node_with(Identity::generate().unwrap(), NodeConfig::default());
    let mut events = within(async {
        let out = tempfile::tempdir().unwrap();
        let record = PeerRecord {
            peer_id: seeder.endpoint().peer_id(),
            addresses: Vec::new(),
            relay: Some(relay_address),
        };
        let handle = downloader
            .download(
                shared.bundle_id(),
                Destination::Dir(out.path().into()),
                vec![record],
            )
            .await
            .unwrap();
        let events = handle.events.resubscribe();
        let mut status = handle.status.clone();
        status.wait_for(|status| status.complete).await.unwrap();
        for name in ["small.txt", "big.bin"] {
            let want = fs::read(dir.path().join(name)).unwrap();
            assert!(fs::read(out.path().join(name)).unwrap() == want, "{name}");
        }
        events
    })
    .await;

    let mut relayed = None;
    let mut direct = HashSet::new();
    let mut over_relay = false;
    let mut duplicate_closed = false;
    let mut received_direct = 0;
    // The events about connections, to show if an assertion fails.
    let mut log = Vec::new();
    while let Ok(event) = events.try_recv() {
        if !matches!(
            event.kind,
            EventKind::Received { .. } | EventKind::Requested { .. }
        ) {
            log.push(event.to_string());
        }
        match event.kind {
            EventKind::Connected { conn, path, .. } => match path {
                PathKind::Relayed => assert!(relayed.replace(conn).is_none(), "one relayed"),
                PathKind::Direct => _ = direct.insert(conn),
            },
            EventKind::ManifestReceived { conn, .. } | EventKind::Requested { conn, .. } => {
                over_relay |= Some(conn) == relayed;
            }
            EventKind::Received { conn, bytes, .. } if direct.contains(&conn) => {
                received_direct += bytes;
            }
            EventKind::Closed { conn, cause, .. } if Some(conn) == relayed => {
                let reason = match cause {
                    CloseCause::Local { reason, .. } | CloseCause::Remote { reason, .. } => {
                        Some(reason)
                    }
                    CloseCause::Lost => None,
                };
                duplicate_closed = reason == Some(GoodbyeReason::Duplicate);
            }
            _ => {}
        }
    }
    assert!(relayed.is_some(), "the transfer started through the relay");
    assert!(over_relay, "the relayed connection carried the transfer");
    assert!(!direct.is_empty(), "a direct connection opened");
    assert!(
        duplicate_closed,
        "the relayed connection closed as a duplicate: {log:#?}"
    );
    assert!(
        received_direct > 0,
        "data arrived over the direct connection"
    );
    downloader.shutdown().await;
    seeder.shutdown().await;
}

/// A relay that registers the peer, then claims its relay address changed, which the protocol
/// doesn't allow: the address may already be published.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_that_changes_the_relay_address_is_a_violation() {
    use quinn::crypto::rustls::QuicServerConfig;
    use tempest_net::{read_relay_message, tls, write_relay_message};
    use tempest_protocol::RelayMessage;
    use tempest_protocol::relay::{RELAY_ALPN, Registered};

    let identity = Identity::generate().unwrap();
    let crypto = QuicServerConfig::try_from(tls::server_config(&identity, RELAY_ALPN)).unwrap();
    let server = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let fake = quinn::Endpoint::server(server, "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = fake.local_addr().unwrap();
    let identity = RelayIdentity {
        key: identity.peer_id(),
        host: RelayHost::Ip(addr.ip()),
        port: addr.port(),
    };

    let node = start_node(Identity::generate().unwrap());
    node.endpoint().set_relays(RelayChoice {
        known: vec![identity],
        from_trackers: false,
    });
    let conn = within(async { fake.accept().await.unwrap().await.unwrap() }).await;
    let (mut send, mut recv) = within(conn.accept_bi()).await.unwrap();
    let register = within(read_relay_message(&mut recv)).await.unwrap();
    assert_eq!(register, Some(RelayMessage::Register));
    let registered = |port| {
        RelayMessage::Registered(Registered {
            relay_address: Address::new(addr.ip(), port),
            observed_address: conn.remote_address().into(),
        })
    };
    write_relay_message(&mut send, &registered(40_000))
        .await
        .unwrap();
    write_relay_message(&mut send, &registered(40_001))
        .await
        .unwrap();
    let closed = within(conn.closed()).await;
    let quinn::ConnectionError::ApplicationClosed(close) = closed else {
        panic!("the peer closed the connection: {closed:?}");
    };
    assert_eq!(
        close.error_code,
        quinn::VarInt::from_u32(GoodbyeReason::ProtocolViolation as u32)
    );
    node.shutdown().await;
}
