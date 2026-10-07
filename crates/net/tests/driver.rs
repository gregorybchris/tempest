//! The production driver on localhost: whole transfers between nodes, and nodes facing
//! hand-driven peers that misbehave, cancel or crowd them.

use std::fs;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use quinn::{RecvStream, SendStream};
use tempest_core::event::CloseCause;
use tempest_core::io::Closed;
use tempest_core::{Event, EventKind};
use tempest_net::{
    BundleHandle, Connection, Endpoint, EndpointConfig, Identity, Node, NodeConfig, Storage,
    StorageConfig, closed_cause, read_message, write_message,
};
use tempest_protocol::message::{
    Data, GoodbyeReason, Have, Hello, ManifestMessage, PeerRecord, Request, Serving, Want,
};
use tempest_protocol::tree::{self, CHUNK_SIZE};
use tempest_protocol::{
    Address, BundleId, ChunkLayout, Entry, FileInfo, Hash, Manifest, Message, RangeList,
};
use tokio::sync::broadcast;

/// How long any test may take before it counts as hung.
const TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, future)
        .await
        .expect("timed out")
}

/// A file of `size` bytes where byte `i` is `(i * 7 + seed) mod 256`.
fn contents(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
        .collect()
}

/// Writes `files` under `dir` and returns their manifest, with `extra` entries added.
fn make_bundle(dir: &Path, files: &[(&str, Vec<u8>)], extra: Vec<Entry>) -> Arc<Manifest> {
    let mut entries = extra;
    for (path, data) in files {
        let fs_path = dir.join(path);
        fs::create_dir_all(fs_path.parent().unwrap()).unwrap();
        fs::write(&fs_path, data).unwrap();
        let info = FileInfo {
            size: data.len() as u64,
            root: tree::root(data),
            executable: false,
            modified: 1_700_000_000_000,
        };
        entries.push(Entry::file(*path, info));
    }
    Arc::new(Manifest::new("bundle", "", 0, "test", entries).unwrap())
}

/// A bundle with an empty file, a nested directory, an empty directory, and a file larger than
/// one maximal Request.
fn sample_bundle(dir: &Path) -> Arc<Manifest> {
    let big = 64 * CHUNK_SIZE * 3 / 2 + 123;
    fs::create_dir_all(dir.join("nested/nothing")).unwrap();
    make_bundle(
        dir,
        &[
            ("readme.txt", b"hello, swarm\n".to_vec()),
            ("empty", Vec::new()),
            ("nested/deeper/big.bin", contents(big, 1)),
            ("nested/small.bin", contents(40_000, 2)),
        ],
        vec![Entry::directory("nested/nothing")],
    )
}

fn config() -> NodeConfig {
    NodeConfig::default()
}

fn start_node(config: NodeConfig) -> Node {
    let endpoint_config = EndpointConfig {
        max_open_requests: config.session.upload.max_open_requests,
        ..EndpointConfig::localhost()
    };
    let endpoint = Endpoint::bind(Identity::generate().unwrap(), &endpoint_config).unwrap();
    Node::start(endpoint, config)
}

fn record(endpoint: &Endpoint) -> PeerRecord {
    PeerRecord {
        peer_id: endpoint.peer_id(),
        addresses: endpoint
            .local_addrs()
            .into_iter()
            .map(Address::from)
            .collect(),
        relay: None,
    }
}

async fn share(node: &Node, dir: &Path, manifest: &Arc<Manifest>) -> (BundleHandle, Storage) {
    let storage = Storage::open_shared(dir, manifest.clone(), StorageConfig::default())
        .await
        .unwrap();
    let handle = node.share(storage.clone()).await.unwrap();
    (handle, storage)
}

/// Waits for an event `matches` accepts, and returns it.
async fn wait_for_event(
    events: &mut broadcast::Receiver<Event>,
    matches: impl Fn(&EventKind) -> bool,
) -> EventKind {
    loop {
        match events.recv().await {
            Ok(event) if matches(&event.kind) => return event.kind,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("no such event"),
        }
    }
}

/// Asserts that every file, directory and empty file under `expected` is the same under
/// `actual`.
fn assert_same_tree(expected: &Path, actual: &Path) {
    for entry in fs::read_dir(expected).unwrap() {
        let entry = entry.unwrap();
        let theirs = actual.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            assert!(theirs.is_dir(), "{} is missing", theirs.display());
            assert_same_tree(&entry.path(), &theirs);
        } else {
            let want = fs::read(entry.path()).unwrap();
            let got = fs::read(&theirs).unwrap();
            assert!(want == got, "{} differs", theirs.display());
        }
    }
}

#[tokio::test]
async fn a_bundle_downloads_byte_for_byte() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());

        let seeder = start_node(config());
        let (seeding, _) = share(&seeder, source.path(), &manifest).await;
        let leecher = start_node(config());
        let mut downloading = leecher
            .download(
                manifest.bundle_id(),
                target.path(),
                vec![record(seeder.endpoint())],
            )
            .await
            .unwrap();
        downloading
            .status
            .wait_for(|status| status.complete)
            .await
            .unwrap();
        // The events were kept from the start.
        wait_for_event(&mut downloading.events, |e| {
            matches!(e, EventKind::ManifestReceived { .. })
        })
        .await;

        assert_same_tree(source.path(), target.path());
        assert!(target.path().join("nested/nothing").is_dir());
        assert_eq!(fs::metadata(target.path().join("empty")).unwrap().len(), 0);

        leecher.shutdown().await;
        seeder.shutdown().await;
        let downloaded = downloading.finished().await.unwrap();
        let seeded = seeding.finished().await.unwrap();
        assert!(downloaded.complete);
        assert_eq!(downloaded.downloaded, manifest.total_size());
        assert_eq!(seeded.uploaded, manifest.total_size());
    })
    .await;
}

/// On several threads, as the client runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_peers_share_a_bundle() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let seeder = start_node(config());
        let (_seeding, _) = share(&seeder, source.path(), &manifest).await;

        let mut targets = Vec::new();
        let mut leechers = Vec::new();
        for _ in 0..2 {
            let target = tempfile::tempdir().unwrap();
            let node = start_node(config());
            let handle = node
                .download(
                    manifest.bundle_id(),
                    target.path(),
                    vec![record(seeder.endpoint())],
                )
                .await
                .unwrap();
            targets.push(target);
            leechers.push((node, handle));
        }
        for (_, handle) in &mut leechers {
            handle.status.wait_for(|s| s.complete).await.unwrap();
        }
        for target in &targets {
            assert_same_tree(source.path(), target.path());
        }
        for (node, _) in &leechers {
            node.shutdown().await;
        }
        seeder.shutdown().await;
    })
    .await;
}

#[tokio::test]
async fn shutdown_says_goodbye_and_flushes() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let seeder = start_node(config());
        let (seeding, _) = share(&seeder, source.path(), &manifest).await;

        let peer =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (connection, _send, mut recv) =
            open(&peer, seeder.endpoint(), manifest.bundle_id()).await;
        // The seeder's opening shows the session took the connection.
        assert!(matches!(
            read_message(&mut recv).await,
            Ok(Some(Message::Hello(_)))
        ));

        seeder.shutdown().await;
        let closed = closed_cause(&connection.closed().await);
        assert!(
            matches!(
                closed,
                Some(Closed::Goodbye {
                    reason: GoodbyeReason::ShuttingDown,
                    ..
                })
            ),
            "{closed:?}"
        );
        let summary = seeding.finished().await.unwrap();
        assert!(summary.complete);
        // A second shutdown is harmless.
        seeder.shutdown().await;
    })
    .await;
}

// ----- Hand-driven peers -----

fn hello(bundle_id: BundleId) -> Message {
    Message::Hello(Hello {
        bundle_id,
        capabilities: Vec::new(),
        client: "tempest/test".into(),
        max_request_chunks: 64,
        max_open_requests: 32,
        addresses: Vec::new(),
        relay: None,
    })
}

/// Dials `node` from `peer` and sends the opening for `bundle_id`, holding nothing.
async fn open(
    peer: &Endpoint,
    node: &Endpoint,
    bundle_id: BundleId,
) -> (Connection, SendStream, RecvStream) {
    let connection = peer
        .dial(node.local_addrs()[0], Some(node.peer_id()))
        .await
        .unwrap();
    let (mut send, recv) = connection.quic().open_bi().await.unwrap();
    write_message(&mut send, &hello(bundle_id)).await.unwrap();
    let have = Message::Have(Have {
        ranges: RangeList::new(),
    });
    write_message(&mut send, &have).await.unwrap();
    (connection, send, recv)
}

/// Reads control messages until the other peer says it serves us.
async fn wait_for_serving(recv: &mut RecvStream) {
    loop {
        match read_message(recv).await.unwrap().unwrap() {
            Message::Serving(Serving { serving: true }) => return,
            _ => continue,
        }
    }
}

#[tokio::test]
async fn a_connection_for_another_bundle_is_turned_away() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let seeder = start_node(config());
        let _seeding = share(&seeder, source.path(), &manifest).await;

        let peer =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (connection, _send, _recv) = open(&peer, seeder.endpoint(), Hash::new([9; 32])).await;
        let closed = closed_cause(&connection.closed().await);
        assert!(
            matches!(
                closed,
                Some(Closed::Goodbye {
                    reason: GoodbyeReason::UnknownBundle,
                    ..
                })
            ),
            "{closed:?}"
        );
        seeder.shutdown().await;
    })
    .await;
}

/// How a hand-driven provider answers Requests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    /// With Data whose first byte is flipped.
    Tampering,
    /// Never; it reports when each request stream is stopped.
    Silent,
}

/// A provider that has every chunk and serves whoever connects, answering ManifestRequests
/// honestly and Requests as `kind` says. Sends the stop code of each stopped request stream on
/// `stopped`, and the connection's end on `closed`.
async fn run_provider(
    endpoint: Endpoint,
    storage: Storage,
    kind: Provider,
    stopped: tokio::sync::mpsc::Sender<Option<u64>>,
    closed: tokio::sync::oneshot::Sender<Option<Closed>>,
) {
    let connection = endpoint.accept().await.unwrap().accept().await.unwrap();
    let (mut send, mut recv) = connection.quic().accept_bi().await.unwrap();
    let bundle_id = storage.manifest().bundle_id();
    let chunks = ChunkLayout::new(storage.manifest()).chunk_count();
    assert!(matches!(
        read_message(&mut recv).await,
        Ok(Some(Message::Hello(_)))
    ));
    assert!(matches!(
        read_message(&mut recv).await,
        Ok(Some(Message::Have(_)))
    ));
    let have = Message::Have(Have {
        ranges: RangeList::from_chunks(0..chunks),
    });
    let serving = Message::Serving(Serving { serving: true });
    for message in [hello(bundle_id), have, serving] {
        write_message(&mut send, &message).await.unwrap();
    }

    let serve = async {
        while let Ok((mut send, mut recv)) = connection.quic().accept_bi().await {
            let storage = storage.clone();
            let stopped = stopped.clone();
            tokio::spawn(async move {
                let answer = match read_message(&mut recv).await {
                    Ok(Some(Message::ManifestRequest)) => Message::Manifest(ManifestMessage {
                        manifest: storage.manifest_bytes().to_vec(),
                    }),
                    Ok(Some(Message::Request(Request { start, count }))) => match kind {
                        Provider::Tampering => {
                            let Data { proof, mut chunks } =
                                storage.read(start, count).await.unwrap();
                            chunks[0] ^= 1;
                            Message::Data(Data { proof, chunks })
                        }
                        Provider::Silent => {
                            let code = send.stopped().await.ok().flatten();
                            let _ = stopped.send(code.map(u64::from)).await;
                            return;
                        }
                    },
                    _ => return,
                };
                let _ = write_message(&mut send, &answer).await;
                let _ = send.finish();
                let _ = send.stopped().await;
            });
        }
    };
    // Keep reading the control stream, as a well-behaved peer does.
    let drain = async { while let Ok(Some(_)) = read_message(&mut recv).await {} };
    tokio::join!(serve, drain);
    let _ = closed.send(closed_cause(&connection.closed().await));
}

#[tokio::test]
async fn a_peer_sending_bad_data_is_closed_and_banned() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let storage =
            Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
                .await
                .unwrap();
        let provider =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (stopped, _) = tokio::sync::mpsc::channel(8);
        let (closed, provider_closed) = tokio::sync::oneshot::channel();
        tokio::spawn(run_provider(
            provider.clone(),
            storage,
            Provider::Tampering,
            stopped,
            closed,
        ));

        let leecher = start_node(config());
        let mut handle = leecher
            .download(manifest.bundle_id(), target.path(), vec![record(&provider)])
            .await
            .unwrap();
        let banned = wait_for_event(&mut handle.events, |e| {
            matches!(e, EventKind::Banned { .. })
        })
        .await;
        assert_eq!(
            banned,
            EventKind::Banned {
                peer: provider.peer_id()
            }
        );
        let closed = wait_for_event(&mut handle.events, |e| {
            matches!(e, EventKind::Closed { .. })
        })
        .await;
        assert!(
            matches!(
                closed,
                EventKind::Closed {
                    cause: CloseCause::Local {
                        reason: GoodbyeReason::BadData,
                        ..
                    },
                    ..
                }
            ),
            "{closed:?}"
        );
        let seen = provider_closed.await.unwrap();
        assert!(
            matches!(
                seen,
                Some(Closed::Goodbye {
                    reason: GoodbyeReason::BadData,
                    ..
                })
            ),
            "{seen:?}"
        );
        // Nothing that failed verification was stored.
        assert_eq!(handle.status.borrow().have, 1, "only the empty file");
        leecher.shutdown().await;
        assert!(!handle.finished().await.unwrap().complete);
    })
    .await;
}

#[tokio::test]
async fn cancelling_a_request_stops_its_stream() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let storage =
            Storage::open_shared(source.path(), manifest.clone(), StorageConfig::default())
                .await
                .unwrap();
        let provider =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (stopped, mut stops) = tokio::sync::mpsc::channel(64);
        let (closed, _) = tokio::sync::oneshot::channel();
        tokio::spawn(run_provider(
            provider.clone(),
            storage,
            Provider::Silent,
            stopped,
            closed,
        ));

        // A provider that sends nothing is soon snubbed, and its requests cancelled.
        let mut config = config();
        config.session.requests.snub_timeout = Duration::from_millis(300);
        let leecher = start_node(config);
        let mut handle = leecher
            .download(manifest.bundle_id(), target.path(), vec![record(&provider)])
            .await
            .unwrap();
        wait_for_event(&mut handle.events, |e| {
            matches!(e, EventKind::Cancelled { .. })
        })
        .await;
        assert_eq!(stops.recv().await, Some(Some(0)));
        leecher.shutdown().await;
    })
    .await;
}

#[tokio::test]
async fn a_provider_stops_answering_a_cancelled_request() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let seeder = start_node(config());
        let (mut seeding, storage) = share(&seeder, source.path(), &manifest).await;

        let peer =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (connection, mut control, mut recv) =
            open(&peer, seeder.endpoint(), manifest.bundle_id()).await;
        let want = Message::Want(Want { wanted: true });
        write_message(&mut control, &want).await.unwrap();
        wait_for_serving(&mut recv).await;

        // The big file's first 64 chunks, asked for and cancelled at once.
        let layout = ChunkLayout::new(&manifest);
        let big = layout
            .files()
            .iter()
            .find(|f| f.chunk_count > 64)
            .unwrap()
            .first_chunk;
        let request = Message::Request(Request {
            start: big,
            count: 64,
        });
        let (mut send, mut cancelled) = connection.quic().open_bi().await.unwrap();
        write_message(&mut send, &request).await.unwrap();
        send.finish().unwrap();
        cancelled.stop(0u32.into()).unwrap();

        // A later request is still answered: the cancelled one freed its upload slot.
        let (mut send, mut answered) = connection.quic().open_bi().await.unwrap();
        write_message(&mut send, &request).await.unwrap();
        send.finish().unwrap();
        let response = read_message(&mut answered).await.unwrap().unwrap();
        assert!(matches!(response, Message::Data(_)), "{response:?}");

        let uploaded = wait_for_event(&mut seeding.events, |e| {
            matches!(e, EventKind::Uploaded { .. })
        })
        .await;
        assert!(matches!(uploaded, EventKind::Uploaded { count: 64, .. }));
        // Only the answered request was read from disk.
        assert_eq!(storage.stats().await.unwrap().reads, 1);
        assert_eq!(seeding.status.borrow().uploaded, 64 * CHUNK_SIZE as u64);
        seeder.shutdown().await;
    })
    .await;
}

#[tokio::test]
async fn request_streams_are_bounded_and_freed() {
    within(async {
        let source = tempfile::tempdir().unwrap();
        let manifest = sample_bundle(source.path());
        let mut config = config();
        config.session.upload.max_open_requests = 4;
        let seeder = start_node(config);
        let _seeding = share(&seeder, source.path(), &manifest).await;

        let peer =
            Endpoint::bind(Identity::generate().unwrap(), &EndpointConfig::localhost()).unwrap();
        let (connection, _control, _recv) =
            open(&peer, seeder.endpoint(), manifest.bundle_id()).await;
        let request = Message::Request(Request { start: 0, count: 1 });

        // Cancelled requests give their streams back.
        for _ in 0..12 {
            let (mut send, mut recv) = connection.quic().open_bi().await.unwrap();
            write_message(&mut send, &request).await.unwrap();
            send.finish().unwrap();
            recv.stop(0u32.into()).unwrap();
        }

        // Four request streams, plus the control stream, fill the limit.
        let mut streams = Vec::new();
        for _ in 0..4 {
            streams.push(connection.quic().open_bi().await.unwrap());
        }
        let quic = connection.quic().clone();
        let mut fifth = tokio::spawn(async move { quic.open_bi().await });
        let waited = tokio::time::timeout(Duration::from_millis(300), &mut fifth).await;
        assert!(waited.is_err(), "a fifth request stream opened");

        // An answered request makes room for another.
        let (mut send, mut recv) = streams.pop().unwrap();
        write_message(&mut send, &request).await.unwrap();
        send.finish().unwrap();
        assert!(read_message(&mut recv).await.unwrap().is_some());
        assert!(read_message(&mut recv).await.unwrap().is_none());
        fifth.await.unwrap().unwrap();
        seeder.shutdown().await;
    })
    .await;
}
