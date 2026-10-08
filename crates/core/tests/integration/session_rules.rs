//! A single session driven by hand: the opening sequence, protocol violations, request handling
//! and the policies around them.

use crate::common;

use std::time::Duration;

use common::{file_bytes, peer_id};
use tempest_core::config::{Superseed, WhenComplete};
use tempest_core::content::MemoryContent;
use tempest_core::event::ServeReason;
use tempest_core::io::{Closed, ConnInfo, Direction, OutRequest, PathKind};
use tempest_core::upload::{ServeDecision, UploadContext, UploadStrategy};
use tempest_core::{
    Action, Config, ConnId, EventKind, Inbound, LocalPeer, MAX_PEER_ADDRESSES, OPENING_TIMEOUT,
    Reply, RequestId, Response, Rng, Session, StreamId, Time,
};
use tempest_protocol::message::{
    Candidates, Data, GoodbyeReason, Have, HaveUpdate, Hello, ManifestMessage, PeerRecord, Peers,
    Reject, RejectReason, Request, Serving, Want,
};
use tempest_protocol::tree::VerifyError;
use tempest_protocol::{Address, BundleId, ChunkRange, Hash, Message, PeerId, RangeList};

const CHUNK: usize = 16 * 1024;
const C: ConnId = ConnId(7);

/// A bundle of 40 chunks in two files: chunks 0–2 and 3–39.
fn content() -> MemoryContent {
    MemoryContent::new(
        "bundle",
        vec![
            ("a".to_owned(), file_bytes(1, 40_000)),
            ("b".to_owned(), file_bytes(2, 37 * CHUNK)),
        ],
    )
}

fn local(n: u8) -> LocalPeer {
    LocalPeer {
        peer_id: peer_id(n),
        addresses: Vec::new(),
        relay: None,
    }
}

fn complete(config: Config) -> Session {
    let content = content();
    let have = RangeList::from_chunks(0..40);
    Session::with_manifest(config, local(5), content.manifest().clone(), &have, 0).unwrap()
}

fn fresh(bundle_id: BundleId) -> Session {
    Session::new(Config::default(), local(5), bundle_id, 0).unwrap()
}

fn hello(bundle_id: BundleId) -> Hello {
    Hello {
        bundle_id,
        capabilities: Vec::new(),
        client: "test".to_owned(),
        max_request_chunks: 64,
        max_open_requests: 32,
        addresses: Vec::new(),
        relay: None,
    }
}

fn ranges(chunks: impl IntoIterator<Item = u32>) -> RangeList {
    RangeList::from_chunks(chunks)
}

fn connect(session: &mut Session, conn: ConnId, peer: PeerId, direction: Direction) {
    let info = ConnInfo {
        peer,
        direction,
        path: PathKind::Direct,
    };
    session.handle_connected(Time::ZERO, conn, info);
}

fn control(session: &mut Session, conn: ConnId, message: Message) {
    session.handle_message(Time::ZERO, conn, Inbound::Control(message));
}

/// Accepts a connection from peer 9 and completes the opening with `have`.
fn open(session: &mut Session, have: RangeList) {
    connect(session, C, peer_id(9), Direction::Accepted);
    let id = session.bundle_id();
    control(session, C, Message::Hello(hello(id)));
    control(session, C, Message::Have(Have { ranges: have }));
}

fn request(session: &mut Session, stream: u64, message: Message) {
    let inbound = Inbound::Request {
        stream: StreamId(stream),
        message,
    };
    session.handle_message(Time::ZERO, C, inbound);
}

fn chunks(start: u32, count: u32) -> Message {
    Message::Request(Request { start, count })
}

fn closes(actions: &[Action]) -> Vec<GoodbyeReason> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Close { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

fn replies(actions: &[Action]) -> Vec<(u64, Reply)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Respond { stream, reply, .. } => Some((stream.0, reply.clone())),
            _ => None,
        })
        .collect()
}

fn reject_reason(reply: &Reply) -> Option<RejectReason> {
    match reply {
        Reply::Reject(Reject { reason, .. }) => Some(*reason),
        _ => None,
    }
}

fn sent(actions: &[Action]) -> Vec<Message> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_dialing_peer_speaks_first() {
    let mut session = complete(Config::default());
    connect(&mut session, C, peer_id(9), Direction::Dialed);
    let sent = sent(&session.poll_actions());
    assert!(matches!(&sent[0], Message::Hello(h) if h.bundle_id == session.bundle_id()));
    assert_eq!(
        sent[1],
        Message::Have(Have {
            ranges: ranges(0..40)
        })
    );
    assert_eq!(sent.len(), 2);
}

#[test]
fn the_accepting_peer_answers_hello_with_hello_and_have() {
    let mut session = fresh(content().manifest().bundle_id());
    connect(&mut session, C, peer_id(9), Direction::Accepted);
    assert!(session.poll_actions().is_empty());
    let id = session.bundle_id();
    control(&mut session, C, Message::Hello(hello(id)));
    let sent = sent(&session.poll_actions());
    assert!(matches!(sent[0], Message::Hello(_)));
    // Without the manifest, Have is empty.
    assert_eq!(sent[1], Message::Have(Have::default()));
}

#[test]
fn the_opening_must_be_hello_then_have() {
    let id = content().manifest().bundle_id();
    let mut session = fresh(id);
    connect(&mut session, C, peer_id(9), Direction::Accepted);
    control(&mut session, C, Message::Want(Want { wanted: true }));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );

    let mut session = fresh(id);
    connect(&mut session, C, peer_id(9), Direction::Accepted);
    control(&mut session, C, Message::Hello(hello(id)));
    control(&mut session, C, Message::Serving(Serving { serving: true }));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );

    let mut session = fresh(id);
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Hello(hello(id)));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );
}

#[test]
fn hello_for_another_bundle() {
    let other = Hash::new([3; 32]);
    let mut session = fresh(content().manifest().bundle_id());
    connect(&mut session, C, peer_id(9), Direction::Accepted);
    control(&mut session, C, Message::Hello(hello(other)));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::UnknownBundle]
    );

    let mut session = fresh(content().manifest().bundle_id());
    connect(&mut session, C, peer_id(9), Direction::Dialed);
    control(&mut session, C, Message::Hello(hello(other)));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );
}

#[test]
fn holdings_must_respect_the_range_rules() {
    let mut session = complete(Config::default());
    open(&mut session, ranges(0..41));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );

    let updates = [
        HaveUpdate {
            added: ranges([3]),
            removed: RangeList::new(),
        },
        HaveUpdate {
            added: RangeList::new(),
            removed: ranges([9]),
        },
    ];
    for update in updates {
        let mut session = complete(Config::default());
        open(&mut session, ranges(0..5));
        session.poll_actions();
        control(&mut session, C, Message::HaveUpdate(update));
        assert_eq!(
            closes(&session.poll_actions()),
            [GoodbyeReason::ProtocolViolation]
        );
    }
}

#[test]
fn holdings_before_the_manifest_are_checked_when_it_arrives() {
    let content = content();
    let mut session = fresh(content.manifest().bundle_id());
    // Out of range, but unknowable until the manifest arrives.
    open(&mut session, ranges(0..50));
    let actions = session.poll_actions();
    assert!(closes(&actions).is_empty());
    let request = actions
        .iter()
        .find_map(|a| match a {
            Action::Request {
                request,
                message: OutRequest::Manifest,
                ..
            } => Some(*request),
            _ => None,
        })
        .expect("asks for the manifest");
    let response = Response::Manifest(ManifestMessage {
        manifest: content.manifest_bytes(),
    });
    session.handle_message(Time::ZERO, C, Inbound::Response { request, response });
    let actions = session.poll_actions();
    assert!(matches!(actions[0], Action::StoreManifest { .. }));
    assert_eq!(closes(&actions), [GoodbyeReason::ProtocolViolation]);
}

#[test]
fn a_manifest_that_does_not_match_is_bad_data() {
    let mut session = fresh(content().manifest().bundle_id());
    open(&mut session, RangeList::new());
    let response = Response::Manifest(ManifestMessage {
        manifest: b"not it".to_vec(),
    });
    session.handle_message(
        Time::ZERO,
        C,
        Inbound::Response {
            request: RequestId(0),
            response,
        },
    );
    assert_eq!(closes(&session.poll_actions()), [GoodbyeReason::BadData]);
    assert!(session.manifest().is_none());
}

#[test]
fn requests_that_arrive_before_the_opening_are_held() {
    let mut session = complete(Config::default());
    connect(&mut session, C, peer_id(9), Direction::Accepted);
    request(&mut session, 1, Message::ManifestRequest);
    let id = session.bundle_id();
    control(&mut session, C, Message::Hello(hello(id)));
    assert!(replies(&session.poll_actions()).is_empty());
    control(&mut session, C, Message::Have(Have::default()));
    assert_eq!(replies(&session.poll_actions()), [(1, Reply::Manifest)]);
}

#[test]
fn requests_are_checked_before_they_are_served() {
    let mut session = complete(Config::default());
    open(&mut session, RangeList::new());
    // Not serving a peer that hasn't said it wants anything.
    request(&mut session, 1, chunks(0, 1));
    control(&mut session, C, Message::Want(Want { wanted: true }));
    let actions = session.poll_actions();
    assert_eq!(
        reject_reason(&replies(&actions)[0].1),
        Some(RejectReason::Busy)
    );
    assert!(sent(&actions).contains(&Message::Serving(Serving { serving: true })));

    let cases = [
        (chunks(1, 2), RejectReason::Invalid),   // misaligned
        (chunks(0, 3), RejectReason::Invalid),   // not a power of two
        (chunks(0, 8), RejectReason::Invalid),   // wider than file a's tree
        (chunks(40, 1), RejectReason::Invalid),  // outside the bundle
        (chunks(3, 128), RejectReason::Invalid), // above max_request_chunks
    ];
    for (i, (message, reason)) in cases.into_iter().enumerate() {
        request(&mut session, 10 + i as u64, message);
        let replies = replies(&session.poll_actions());
        assert_eq!(reject_reason(&replies[0].1), Some(reason), "case {i}");
    }

    // File a's last aligned pair reaches into padding, which is allowed.
    request(&mut session, 20, chunks(2, 1));
    request(&mut session, 21, chunks(3, 32));
    let replies = replies(&session.poll_actions());
    assert_eq!(replies[0], (20, Reply::Data { start: 2, count: 1 }));
    assert_eq!(
        replies[1],
        (
            21,
            Reply::Data {
                start: 3,
                count: 32
            }
        )
    );
}

#[test]
fn a_peer_without_the_chunks_says_dont_have() {
    let content = content();
    let have = ranges(0..3);
    let mut session = Session::with_manifest(
        Config::default(),
        local(5),
        content.manifest().clone(),
        &have,
        0,
    )
    .unwrap();
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    request(&mut session, 1, chunks(3, 1));
    let replies = replies(&session.poll_actions());
    assert_eq!(reject_reason(&replies[0].1), Some(RejectReason::DontHave));
}

#[test]
fn open_requests_and_upload_slots_are_limited() {
    let mut config = Config::default();
    config.upload.max_open_requests = 2;
    config.upload.parallel_responses = 1;
    let mut session = complete(config);
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    session.poll_actions();
    for stream in 1..=3 {
        request(&mut session, stream, chunks(3 + stream as u32, 1));
    }
    let replies = replies(&session.poll_actions());
    // One is sent, one waits for the upload slot, and one is too many.
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0], (1, Reply::Data { start: 4, count: 1 }));
    assert_eq!(reject_reason(&replies[1].1), Some(RejectReason::Busy));

    let sent = Inbound::DataSent {
        stream: StreamId(1),
    };
    session.handle_message(Time::ZERO, C, sent);
    let next = self::replies(&session.poll_actions());
    assert_eq!(next, [(2, Reply::Data { start: 5, count: 1 })]);
    let uploaded = session
        .poll_events()
        .into_iter()
        .filter(|e| matches!(e.kind, EventKind::Uploaded { .. }))
        .count();
    assert_eq!(uploaded, 1);

    // A reset frees the slot without counting as sent.
    session.handle_message(
        Time::ZERO,
        C,
        Inbound::RequestReset {
            stream: StreamId(2),
        },
    );
    assert_eq!(session.uploaded(), CHUNK as u64);
}

#[test]
fn messages_on_the_wrong_stream_are_violations() {
    let mut session = complete(Config::default());
    open(&mut session, RangeList::new());
    request(&mut session, 1, Message::Want(Want { wanted: true }));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );

    let mut session = complete(Config::default());
    open(&mut session, RangeList::new());
    control(&mut session, C, chunks(0, 1));
    assert_eq!(
        closes(&session.poll_actions()),
        [GoodbyeReason::ProtocolViolation]
    );
}

#[test]
fn violations_the_driver_sees_close_the_connection() {
    let mut session = complete(Config::default());
    open(&mut session, RangeList::new());
    session.poll_actions();
    session.handle_violation(Time::ZERO, C, "a frame did not decode");
    let actions = session.poll_actions();
    assert_eq!(closes(&actions), [GoodbyeReason::ProtocolViolation]);
    assert_eq!(session.connection_count(), 0);

    // A connection the session already closed is left alone.
    session.handle_violation(Time::ZERO, C, "again");
    assert!(session.poll_actions().is_empty());
}

/// Opens a connection to a provider with every chunk that serves us, returning the first
/// request the session sends.
fn fetching_session() -> (Session, RequestId, u32, u32) {
    let content = content();
    let mut session = Session::with_manifest(
        Config::default(),
        local(5),
        content.manifest().clone(),
        &RangeList::new(),
        0,
    )
    .unwrap();
    open(&mut session, ranges(0..40));
    control(&mut session, C, Message::Serving(Serving { serving: true }));
    let actions = session.poll_actions();
    assert!(sent(&actions).contains(&Message::Want(Want { wanted: true })));
    let (request, start, count) = actions
        .iter()
        .find_map(|a| match a {
            Action::Request {
                request,
                message: OutRequest::Chunks { start, count },
                ..
            } => Some((*request, *start, *count)),
            _ => None,
        })
        .expect("requests chunks");
    (session, request, start, count)
}

#[test]
fn data_that_fails_verification_bans_the_peer() {
    let (mut session, request, ..) = fetching_session();
    let data = Data::default();
    assert!(matches!(
        session.verify_data(C, request, &data),
        Some(Err(
            VerifyError::WrongProofLength { .. } | VerifyError::WrongDataLength { .. }
        ))
    ));
    let response = Response::Data(Err(VerifyError::RootMismatch));
    session.handle_message(Time::ZERO, C, Inbound::Response { request, response });
    assert_eq!(closes(&session.poll_actions()), [GoodbyeReason::BadData]);
    assert!(
        session
            .poll_events()
            .iter()
            .any(|e| e.kind == EventKind::Banned { peer: peer_id(9) })
    );

    connect(&mut session, ConnId(8), peer_id(9), Direction::Accepted);
    assert_eq!(closes(&session.poll_actions()), [GoodbyeReason::BadData]);
}

#[test]
fn a_rejected_provider_is_retried_after_a_delay() {
    let (mut session, request, ..) = fetching_session();
    let response = Response::Reject(Reject {
        reason: RejectReason::Busy,
        message: String::new(),
    });
    session.handle_message(Time::ZERO, C, Inbound::Response { request, response });
    // The other requests in flight are still open, so nothing new is sent yet.
    assert!(session.poll_actions().is_empty());
    let retry = session.next_timer().expect("a retry is scheduled");
    assert_eq!(retry, Time::ZERO + Duration::from_secs(1));
    session.handle_timer(retry);
    let actions = session.poll_actions();
    assert!(actions.iter().any(|a| matches!(a, Action::Request { .. })));
}

#[test]
fn verified_data_is_announced() {
    let (mut session, request, start, count) = fetching_session();
    let response = Response::Data(Ok(()));
    session.handle_message(Time::ZERO, C, Inbound::Response { request, response });
    let sent = sent(&session.poll_actions());
    // The request may reach past its file's end into the tree's padding.
    let have = session.have().unwrap();
    assert!(have.len() <= count && have.iter().all(|c| (start..start + count).contains(&c)));
    let added = ranges(have.iter());
    assert!(sent.contains(&Message::HaveUpdate(HaveUpdate {
        added,
        removed: RangeList::new()
    })));
}

#[test]
fn the_lower_peer_id_keeps_one_of_two_connections() {
    let bundle_id = content().manifest().bundle_id();
    // Local ID 5 is lower than 9, so this session decides.
    let mut session = complete(Config::default());
    for conn in [ConnId(1), ConnId(2)] {
        connect(&mut session, conn, peer_id(9), Direction::Accepted);
        control(&mut session, conn, Message::Hello(hello(bundle_id)));
        control(&mut session, conn, Message::Have(Have::default()));
    }
    let closed: Vec<_> = session
        .poll_actions()
        .into_iter()
        .filter_map(|a| match a {
            Action::Close { conn, reason, .. } => Some((conn, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(closed, [(ConnId(2), GoodbyeReason::Duplicate)]);

    // Peer 3 is lower, so the session waits for its choice.
    let mut session = complete(Config::default());
    for conn in [ConnId(1), ConnId(2)] {
        connect(&mut session, conn, peer_id(3), Direction::Accepted);
        control(&mut session, conn, Message::Hello(hello(bundle_id)));
        control(&mut session, conn, Message::Have(Have::default()));
    }
    assert!(closes(&session.poll_actions()).is_empty());
    session.handle_closed(
        Time::ZERO,
        ConnId(2),
        Closed::Goodbye {
            reason: GoodbyeReason::Duplicate,
            message: String::new(),
        },
    );
    assert_eq!(session.connection_count(), 1);
}

#[test]
fn candidates_get_one_reply() {
    let mut session = complete(Config::default());
    open(&mut session, RangeList::new());
    session.poll_actions();
    for _ in 0..2 {
        control(&mut session, C, Message::Candidates(Candidates::default()));
    }
    let replies: Vec<_> = sent(&session.poll_actions())
        .into_iter()
        .filter(|m| matches!(m, Message::Candidates(_)))
        .collect();
    assert_eq!(replies.len(), 1);
}

#[test]
fn ranges_must_fit_when_starting_with_a_manifest() {
    let content = content();
    let have = RangeList::from_ranges(vec![ChunkRange { start: 0, end: 41 }]).unwrap();
    let result = Session::with_manifest(
        Config::default(),
        local(5),
        content.manifest().clone(),
        &have,
        0,
    );
    assert!(result.is_err());
}

#[test]
fn a_superseeding_peer_announces_only_what_it_offers() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::On;
    config.upload.superseed_window = 1;
    let mut session = complete(config);
    assert!(session.is_superseeding());
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    let sent = sent(&session.poll_actions());
    // Have hides everything; a HaveUpdate then offers the first file's 3 chunks.
    assert_eq!(sent[1], Message::Have(Have::default()));
    assert!(sent.contains(&Message::HaveUpdate(HaveUpdate {
        added: ranges(0..3),
        removed: RangeList::new(),
    })));
    request(&mut session, 1, chunks(8, 1));
    request(&mut session, 2, chunks(0, 4));
    let replies = replies(&session.poll_actions());
    assert_eq!(reject_reason(&replies[0].1), Some(RejectReason::DontHave));
    assert_eq!(replies[1], (2, Reply::Data { start: 0, count: 4 }));

    // Once the peer has them, it is offered the next range.
    let update = HaveUpdate {
        added: ranges(0..3),
        removed: RangeList::new(),
    };
    control(&mut session, C, Message::HaveUpdate(update));
    assert!(
        self::sent(&session.poll_actions()).contains(&Message::HaveUpdate(HaveUpdate {
            added: ranges(3..19),
            removed: RangeList::new(),
        }))
    );
}

#[test]
fn a_superseeding_peer_keeps_a_window_of_ranges_on_offer() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::On;
    config.upload.superseed_window = 3;
    config.requests.chunks = 4;
    let mut session = complete(config);
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    let offers = |messages: Vec<Message>| -> Vec<RangeList> {
        messages
            .into_iter()
            .filter_map(|m| match m {
                Message::HaveUpdate(update) => Some(update.added),
                _ => None,
            })
            .collect()
    };
    // The window starts at one range and grows by one each time the peer gains chunks, while
    // nobody else waits to be served.
    let first = offers(sent(&session.poll_actions()));
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[0], ranges(0..3));
    let update = |added: RangeList| HaveUpdate {
        added,
        removed: RangeList::new(),
    };
    control(&mut session, C, Message::HaveUpdate(update(ranges(0..2))));
    let more = offers(sent(&session.poll_actions()));
    assert_eq!(more.len(), 1, "{more:?}");

    // At its limit, a range is offered only once the peer has a whole one.
    control(&mut session, C, Message::HaveUpdate(update(ranges(3..4))));
    assert!(offers(sent(&session.poll_actions())).is_empty());
    control(&mut session, C, Message::HaveUpdate(update(ranges(2..3))));
    let next = offers(sent(&session.poll_actions()));
    assert_eq!(next.len(), 1, "{next:?}");
    assert!(first.iter().chain(&more).all(|r| *r != next[0]));
}

/// Accepts a connection `conn` from peer `n` and completes the opening with an empty Have.
fn open_peer(session: &mut Session, conn: ConnId, n: u8) {
    connect(session, conn, peer_id(n), Direction::Accepted);
    let id = session.bundle_id();
    control(session, conn, Message::Hello(hello(id)));
    control(session, conn, Message::Have(Have::default()));
}

/// The chunks announced to `conn` by HaveUpdates in `actions`.
fn announced_to(actions: &[Action], conn: ConnId) -> Vec<u32> {
    let mut chunks = Vec::new();
    for action in actions {
        if let Action::Send {
            conn: c,
            message: Message::HaveUpdate(update),
        } = action
            && *c == conn
        {
            chunks.extend(update.added.chunks());
        }
    }
    chunks
}

#[test]
fn crowd_superseeding_announces_everything_to_a_lone_downloader() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::Crowd;
    let mut session = complete(config);
    assert!(session.is_superseeding());
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    let actions = session.poll_actions();
    // Its Have lists every chunk, as a session that doesn't superseed would.
    assert_eq!(
        sent(&actions)[1],
        Message::Have(Have {
            ranges: ranges(0..40)
        })
    );
    assert!(announced_to(&actions, C).is_empty());
    // It can request any chunk straight away.
    request(&mut session, 1, chunks(20, 1));
    assert_eq!(
        replies(&session.poll_actions())[0],
        (
            1,
            Reply::Data {
                start: 20,
                count: 1
            }
        )
    );
}

#[test]
fn crowd_superseeding_offers_ranges_once_a_second_peer_wants_chunks() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::Crowd;
    config.requests.chunks = 4;
    let mut session = complete(config);
    let second = ConnId(8);
    open(&mut session, RangeList::new());
    session.poll_actions();
    open_peer(&mut session, second, 10);
    control(&mut session, second, Message::Want(Want { wanted: true }));
    let actions = session.poll_actions();
    // The first peer keeps what it was shown. The second is offered one range, not everything.
    let offered = announced_to(&actions, second);
    assert!(!offered.is_empty() && offered.len() < 40, "{offered:?}");
    assert_eq!(offered[..3], [0, 1, 2], "the first file");
    assert!(announced_to(&actions, C).is_empty());
    let inbound = Inbound::Request {
        stream: StreamId(1),
        message: chunks(30, 1),
    };
    session.handle_message(Time::ZERO, second, inbound);
    assert_eq!(
        reject_reason(&replies(&session.poll_actions())[0].1),
        Some(RejectReason::DontHave)
    );
    assert!(session.is_superseeding());
}

#[test]
fn crowd_superseeding_announces_everything_to_the_peer_left() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::Crowd;
    config.requests.chunks = 4;
    let mut session = complete(config);
    let first = ConnId(8);
    let second = ConnId(9);
    open_peer(&mut session, first, 10);
    open_peer(&mut session, second, 11);
    let offered = announced_to(&session.poll_actions(), second);
    assert!(session.is_superseeding());
    // The first peer to arrive was alone when it did, so it was shown everything; the second is
    // offered only a few ranges. When the first goes, the second is shown the rest.
    session.handle_closed(Time::ZERO, first, Closed::Lost);
    let mut all = offered;
    all.extend(announced_to(&session.poll_actions(), second));
    all.sort_unstable();
    assert_eq!(all, (0..40).collect::<Vec<_>>());
}

#[test]
fn crowd_superseeding_does_not_throttle_a_one_to_one_send() {
    let mut config = Config::default();
    config.strategies.superseed = Superseed::Crowd;
    let mut session = complete(config);
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    session.poll_actions();
    // No window to wait on: every request is answered, without any new announcement.
    for (stream, start) in [(1, 0), (2, 8), (3, 16), (4, 32)] {
        request(&mut session, stream, chunks(start, 1));
    }
    let actions = session.poll_actions();
    assert_eq!(replies(&actions).len(), 4);
    assert!(
        replies(&actions)
            .iter()
            .all(|(_, r)| matches!(r, Reply::Data { .. }))
    );
    assert!(announced_to(&actions, C).is_empty());
}

#[test]
fn known_peers_are_capped() {
    let mut config = Config::default();
    config.connections.max_known_peers = 3;
    config.connections.max = 1;
    let mut session = Session::new(config, local(5), content().manifest().bundle_id(), 0).unwrap();
    let records = (10..30).map(|n| common::record(peer_id(n)));
    session.add_peers(Time::ZERO, records);
    assert_eq!(session.known_peer_count(), 3);
}

#[test]
fn records_for_a_known_peer_are_merged() {
    let bundle_id = content().manifest().bundle_id();
    let mut session = Session::new(Config::default(), local(5), bundle_id, 0).unwrap();
    let addr = |n: u16| Address::from(std::net::SocketAddr::from(([192, 0, 2, 1], n)));
    let peer = peer_id(10);
    let record = |addresses: Vec<Address>, relay: Option<Address>| PeerRecord {
        peer_id: peer,
        addresses,
        relay,
    };
    let dialed = |session: &mut Session| -> Vec<PeerRecord> {
        session
            .poll_actions()
            .into_iter()
            .filter_map(|a| match a {
                Action::Dial(record) => Some(record),
                _ => None,
            })
            .collect()
    };
    let mut now = Time::ZERO;
    // Redials the peer and returns the record it dialed.
    let redial = |session: &mut Session, now: &mut Time| -> PeerRecord {
        session.handle_dial_failed(*now, peer);
        *now = session.next_timer().unwrap();
        session.handle_timer(*now);
        let mut records = dialed(session);
        assert_eq!(records.len(), 1);
        records.pop().unwrap()
    };

    // From a tracker, with a relay.
    session.add_peers(now, [record(vec![addr(1), addr(2)], Some(addr(9)))]);
    assert_eq!(
        dialed(&mut session),
        [record(vec![addr(1), addr(2)], Some(addr(9)))]
    );

    // From the LAN, with one address and no relay: both are added to what is known.
    session.add_peers(now, [record(vec![addr(3), addr(1)], None)]);
    assert_eq!(
        redial(&mut session, &mut now),
        record(vec![addr(3), addr(1), addr(2)], Some(addr(9)))
    );

    // From peer exchange, listing the relay address among the direct ones: it is dropped.
    session.add_peers(now, [record(vec![addr(9), addr(4)], Some(addr(9)))]);
    assert_eq!(
        redial(&mut session, &mut now),
        record(vec![addr(4), addr(3), addr(1), addr(2)], Some(addr(9)))
    );

    // A new relay replaces the old one, and a known address equal to it is dropped.
    session.add_peers(now, [record(Vec::new(), Some(addr(3)))]);
    assert_eq!(
        redial(&mut session, &mut now),
        record(vec![addr(4), addr(1), addr(2)], Some(addr(3)))
    );

    // The newest addresses are kept, up to the bound.
    let many: Vec<Address> = (100..130).map(addr).collect();
    session.add_peers(now, [record(many.clone(), None)]);
    let merged = redial(&mut session, &mut now);
    assert_eq!(merged.addresses, many[..MAX_PEER_ADDRESSES]);
    assert_eq!(merged.relay, Some(addr(3)));
}

#[test]
fn a_new_peer_record_never_holds_its_relay_address_as_a_direct_one() {
    let bundle_id = content().manifest().bundle_id();
    let mut session = Session::new(Config::default(), local(5), bundle_id, 0).unwrap();
    let addr = |n: u16| Address::from(std::net::SocketAddr::from(([192, 0, 2, 1], n)));
    let peers = Peers {
        peers: vec![PeerRecord {
            peer_id: peer_id(10),
            addresses: vec![addr(9), addr(1), addr(1)],
            relay: Some(addr(9)),
        }],
    };
    connect(&mut session, C, peer_id(11), Direction::Accepted);
    control(&mut session, C, Message::Hello(hello(bundle_id)));
    control(&mut session, C, Message::Have(Have::default()));
    session.poll_actions();
    control(&mut session, C, Message::Peers(peers));
    let dialed: Vec<PeerRecord> = session
        .poll_actions()
        .into_iter()
        .filter_map(|a| match a {
            Action::Dial(record) => Some(record),
            _ => None,
        })
        .collect();
    assert_eq!(
        dialed,
        [PeerRecord {
            peer_id: peer_id(10),
            addresses: vec![addr(1)],
            relay: Some(addr(9)),
        }]
    );
}

#[test]
fn an_opening_that_never_completes_is_closed() {
    let bundle_id = content().manifest().bundle_id();
    let mut session = complete(Config::default());
    connect(&mut session, C, peer_id(10), Direction::Dialed);
    // Hello arrives, but never Have.
    control(&mut session, C, Message::Hello(hello(bundle_id)));
    session.poll_actions();
    let deadline = Time::ZERO + OPENING_TIMEOUT;
    assert!(session.next_timer().is_some_and(|t| t <= deadline));
    session.handle_timer(deadline);
    let closed = session.poll_actions().into_iter().any(|a| {
        matches!(
            a,
            Action::Close {
                conn: C,
                reason: GoodbyeReason::ProtocolViolation,
                ..
            }
        )
    });
    assert!(closed);
}

#[test]
fn a_hello_with_many_addresses_is_shared_with_a_few() {
    const D: ConnId = ConnId(8);
    let bundle_id = content().manifest().bundle_id();
    let mut session = complete(Config::default());
    let addr = |n: u16| Address::from(std::net::SocketAddr::from(([192, 0, 2, 1], n)));
    let crowded = Hello {
        addresses: (1..=10_000).map(addr).collect(),
        ..hello(bundle_id)
    };
    connect(&mut session, C, peer_id(10), Direction::Accepted);
    control(&mut session, C, Message::Hello(crowded));
    control(&mut session, C, Message::Have(Have::default()));
    connect(&mut session, D, peer_id(11), Direction::Accepted);
    control(&mut session, D, Message::Hello(hello(bundle_id)));
    control(&mut session, D, Message::Have(Have::default()));
    let shared: Vec<PeerRecord> = session
        .poll_actions()
        .into_iter()
        .filter_map(|a| match a {
            Action::Send {
                conn,
                message: Message::Peers(peers),
            } if conn == D => Some(peers.peers),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].addresses.len(), MAX_PEER_ADDRESSES);
}

#[test]
fn peer_exchange_respects_the_protocol_limits() {
    let bundle_id = content().manifest().bundle_id();
    let mut config = Config::default();
    config.connections.max = 100;
    let mut session = complete(config);
    for n in 0..60u8 {
        let conn = ConnId(u64::from(n));
        connect(&mut session, conn, peer_id(100 + n), Direction::Accepted);
        control(&mut session, conn, Message::Hello(hello(bundle_id)));
        control(&mut session, conn, Message::Have(Have::default()));
    }
    let shared = |actions: &[Action]| -> Vec<(ConnId, usize)> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Send {
                    conn,
                    message: Message::Peers(peers),
                } => Some((*conn, peers.peers.len())),
                _ => None,
            })
            .collect()
    };
    let first = shared(&session.poll_actions());
    // Each connection hears about the peers connected before it, up to 50.
    assert_eq!(first.len(), 59);
    assert!(first.iter().all(|&(_, n)| n <= 50));
    assert_eq!(first.last(), Some(&(ConnId(59), 50)));

    // Nothing more until a minute has passed.
    let soon = Time::ZERO + Duration::from_secs(59);
    session.handle_timer(soon);
    assert!(shared(&session.poll_actions()).is_empty());
    let minute = session.next_timer().unwrap();
    assert_eq!(minute, Time::ZERO + Duration::from_secs(60));
    session.handle_timer(minute);
    let again = shared(&session.poll_actions());
    assert_eq!(again.len(), 60);
    assert!(again.iter().all(|&(_, n)| n == 50));
}

#[test]
fn dials_leave_room_for_peers_that_dial_in() {
    let bundle_id = content().manifest().bundle_id();
    let mut config = Config::default();
    config.connections.max_dialing = 3;
    let mut session = Session::new(config, local(5), bundle_id, 0).unwrap();
    let dialed = |actions: &[Action]| -> Vec<PeerId> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Dial(record) => Some(record.peer_id),
                _ => None,
            })
            .collect()
    };
    session.add_peers(Time::ZERO, (10..30).map(|n| common::record(peer_id(n))));
    let first = dialed(&session.poll_actions());
    assert_eq!(first.len(), 3);
    assert_eq!(session.next_timer(), None, "no dial waits on a timer");

    // A dial that connects stays pending until the connection opens.
    connect(&mut session, ConnId(1), first[0], Direction::Dialed);
    assert!(dialed(&session.poll_actions()).is_empty());
    // Peers that dial in are accepted meanwhile.
    connect(&mut session, ConnId(2), peer_id(40), Direction::Accepted);
    assert_eq!(session.connection_count(), 2);
    control(&mut session, ConnId(1), Message::Hello(hello(bundle_id)));
    control(&mut session, ConnId(1), Message::Have(Have::default()));
    assert_eq!(dialed(&session.poll_actions()).len(), 1);

    // A failed dial frees its place too.
    session.handle_dial_failed(Time::ZERO, first[1]);
    assert_eq!(dialed(&session.poll_actions()).len(), 1);
}

/// Serves every peer that wants data until `until`, then nobody.
#[derive(Debug)]
struct ServeUntil(Time);

impl UploadStrategy for ServeUntil {
    fn review(&mut self, ctx: &UploadContext<'_>, _rng: &mut Rng) -> Vec<ServeDecision> {
        ctx.peers
            .iter()
            .map(|p| ServeDecision {
                conn: p.conn,
                serve: p.wants && ctx.now < self.0,
                reason: ServeReason::FreeSlot,
            })
            .collect()
    }

    fn next_review(&self) -> Option<Time> {
        Some(self.0)
    }
}

#[test]
fn a_peer_no_longer_served_loses_its_waiting_requests() {
    let mut config = Config::default();
    config.upload.parallel_responses = 1;
    config.strategies.when_complete = WhenComplete::Unchanged;
    let mut session = complete(config);
    let until = Time::ZERO + Duration::from_secs(5);
    session.set_upload_strategy(Box::new(ServeUntil(until)));
    open(&mut session, RangeList::new());
    control(&mut session, C, Message::Want(Want { wanted: true }));
    session.poll_actions();
    for stream in 1..=3 {
        request(&mut session, stream, chunks(3 + stream as u32, 1));
    }
    assert_eq!(
        replies(&session.poll_actions()),
        [(1, Reply::Data { start: 4, count: 1 })]
    );

    session.handle_timer(until);
    let actions = session.poll_actions();
    assert!(sent(&actions).contains(&Message::Serving(Serving { serving: false })));
    let refused = replies(&actions);
    assert_eq!(refused.len(), 2);
    assert!(
        refused
            .iter()
            .all(|(_, r)| reject_reason(r) == Some(RejectReason::Busy))
    );
    // What was already being sent still counts.
    let done = Inbound::DataSent {
        stream: StreamId(1),
    };
    session.handle_message(until, C, done);
    assert!(replies(&session.poll_actions()).is_empty());
    assert_eq!(session.uploaded(), CHUNK as u64);
}

#[test]
fn a_session_shows_its_peers_and_chunk_map() {
    let content = content();
    let have = ranges(0..20);
    let mut session = Session::with_manifest(
        Config::default(),
        local(5),
        content.manifest().clone(),
        &have,
        0,
    )
    .unwrap();
    open(&mut session, ranges(0..40));
    control(&mut session, C, Message::Serving(Serving { serving: true }));
    session.poll_actions();

    let peers = session.peers(Time::ZERO);
    assert_eq!(peers.len(), 1);
    let peer = &peers[0];
    assert_eq!(
        (peer.conn, peer.peer, peer.path),
        (C, peer_id(9), PathKind::Direct)
    );
    assert!(peer.open && peer.we_want && peer.serving_us && !peer.they_want);
    assert_eq!(peer.progress, Some(1.0));

    // Two cells of 20 chunks: the first stored, the second requested in part. Peer 9 holds
    // every chunk.
    let map = session.chunk_map(2).unwrap();
    assert_eq!((map[0].have, map[0].availability), (20, 1));
    assert_eq!(map[1].have, 0);
    assert!(map[1].in_flight > 0, "{map:?}");
    assert_eq!(map[1].in_flight + map[1].missing, 20);

    // Before the manifest there is no map.
    assert!(fresh(content.manifest().bundle_id()).chunk_map(2).is_none());
}
