//! Upgrading relayed connections to direct ones: the Candidates exchange, hole punching and
//! duplicate resolution (`protocol/relays.md`, Upgrading to a direct connection;
//! `protocol/connections.md`, Duplicate connections).

use crate::common;

use common::{file_bytes, peer_id};
use proptest::prelude::*;
use tempest_core::content::MemoryContent;
use tempest_core::io::{ConnInfo, Direction, OutRequest, PathKind};
use tempest_core::{Action, Config, ConnId, Inbound, LocalPeer, Session, Time};
use tempest_protocol::message::{Candidates, GoodbyeReason, Have, Hello, Serving};
use tempest_protocol::{Address, Message, PeerId, RangeList};

/// This session's peer ID is 5, so it decides about duplicates with peers above 5.
const LOCAL: u8 = 5;

fn content() -> MemoryContent {
    MemoryContent::new("bundle", vec![("a".to_owned(), file_bytes(1, 640 * 1024))])
}

fn address(last: u8, port: u16) -> Address {
    Address::new([192, 0, 2, last].into(), port)
}

fn local() -> LocalPeer {
    LocalPeer {
        peer_id: peer_id(LOCAL),
        addresses: vec![address(1, 4773), address(2, 4773)],
        relay: None,
    }
}

fn session(config: Config, have: &RangeList) -> Session {
    let manifest = content().manifest().clone();
    Session::with_manifest(config, local(), manifest, have, 0).unwrap()
}

fn complete() -> Session {
    session(Config::default(), &RangeList::from_chunks(0..40))
}

fn hello(session: &Session) -> Hello {
    Hello {
        bundle_id: session.bundle_id(),
        capabilities: Vec::new(),
        client: "test".to_owned(),
        max_request_chunks: 64,
        max_open_requests: 32,
        addresses: Vec::new(),
        relay: None,
    }
}

fn connect(session: &mut Session, conn: ConnId, peer: u8, direction: Direction, path: PathKind) {
    let info = ConnInfo {
        peer: peer_id(peer),
        direction,
        path,
    };
    session.handle_connected(Time::ZERO, conn, info);
}

fn control(session: &mut Session, conn: ConnId, message: Message) {
    session.handle_message(Time::ZERO, conn, Inbound::Control(message));
}

/// Connects to `peer` and completes the opening, with the other peer having `have`.
fn open(
    session: &mut Session,
    conn: ConnId,
    peer: u8,
    direction: Direction,
    path: PathKind,
    have: RangeList,
) {
    connect(session, conn, peer, direction, path);
    let hello = hello(session);
    control(session, conn, Message::Hello(hello));
    control(session, conn, Message::Have(Have { ranges: have }));
}

fn candidates_sent(actions: &[Action]) -> Vec<(ConnId, Vec<Address>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Send {
                conn,
                message: Message::Candidates(c),
            } => Some((*conn, c.addresses.clone())),
            _ => None,
        })
        .collect()
}

fn punches(actions: &[Action]) -> Vec<(ConnId, PeerId, Vec<Address>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Punch {
                conn,
                peer,
                addresses,
            } => Some((*conn, *peer, addresses.clone())),
            _ => None,
        })
        .collect()
}

fn closes(actions: &[Action]) -> Vec<(ConnId, GoodbyeReason)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Close { conn, reason, .. } => Some((*conn, *reason)),
            _ => None,
        })
        .collect()
}

fn chunk_requests(actions: &[Action], on: ConnId) -> Vec<(u32, u32)> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Request {
                conn,
                message: OutRequest::Chunks { start, count },
                ..
            } if *conn == on => Some((*start, *count)),
            _ => None,
        })
        .collect()
}

const RELAYED: ConnId = ConnId(1);
const DIRECT: ConnId = ConnId(2);

#[test]
fn the_registered_peer_sends_candidates_after_the_opening() {
    let mut session = complete();
    let observed = address(9, 61000);
    // The observed address comes first; an address listed twice appears once.
    session.set_external_addresses(vec![observed, address(1, 4773)]);
    connect(
        &mut session,
        RELAYED,
        9,
        Direction::Accepted,
        PathKind::Relayed,
    );
    let hello = hello(&session);
    control(&mut session, RELAYED, Message::Hello(hello));
    assert!(candidates_sent(&session.poll_actions()).is_empty());
    control(&mut session, RELAYED, Message::Have(Have::default()));
    let expected = vec![observed, address(1, 4773), address(2, 4773)];
    assert_eq!(
        candidates_sent(&session.poll_actions()),
        [(RELAYED, expected)]
    );
}

#[test]
fn addresses_set_later_go_in_hello_and_candidates() {
    let mut session = complete();
    // A port mapping's external address, then the interfaces' addresses.
    let mapped = address(7, 40000);
    session.set_addresses(vec![mapped, address(1, 4773)]);
    connect(&mut session, DIRECT, 9, Direction::Dialed, PathKind::Direct);
    let hellos: Vec<Vec<Address>> = session
        .poll_actions()
        .iter()
        .filter_map(|a| match a {
            Action::Send {
                message: Message::Hello(hello),
                ..
            } => Some(hello.addresses.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(hellos, [vec![mapped, address(1, 4773)]]);

    open(
        &mut session,
        RELAYED,
        8,
        Direction::Accepted,
        PathKind::Relayed,
        RangeList::new(),
    );
    assert_eq!(
        candidates_sent(&session.poll_actions()),
        [(RELAYED, vec![mapped, address(1, 4773)])]
    );
}

#[test]
fn only_the_registered_peer_sends_candidates_first() {
    // This peer dialed the other's relay address, so it waits for the other's Candidates.
    let mut session = complete();
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Dialed,
        PathKind::Relayed,
        RangeList::new(),
    );
    assert!(candidates_sent(&session.poll_actions()).is_empty());

    // Nobody starts an exchange on a direct connection.
    let mut session = complete();
    open(
        &mut session,
        DIRECT,
        9,
        Direction::Accepted,
        PathKind::Direct,
        RangeList::new(),
    );
    assert!(candidates_sent(&session.poll_actions()).is_empty());
}

#[test]
fn receiving_candidates_on_a_relayed_connection_starts_a_punch() {
    let mut session = complete();
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Dialed,
        PathKind::Relayed,
        RangeList::new(),
    );
    session.poll_actions();
    let theirs = vec![address(7, 5000), address(8, 5000)];
    let message = Message::Candidates(Candidates {
        addresses: theirs.clone(),
    });
    control(&mut session, RELAYED, message.clone());
    let actions = session.poll_actions();
    // The reply comes before the dialing.
    let reply = actions.iter().position(|a| {
        matches!(
            a,
            Action::Send {
                message: Message::Candidates(_),
                ..
            }
        )
    });
    let punch = actions
        .iter()
        .position(|a| matches!(a, Action::Punch { .. }));
    assert!(reply < punch, "{actions:?}");
    assert_eq!(candidates_sent(&actions).len(), 1);
    assert_eq!(punches(&actions), [(RELAYED, peer_id(9), theirs)]);

    // A second Candidates changes nothing.
    control(&mut session, RELAYED, message);
    assert!(session.poll_actions().is_empty());
}

#[test]
fn the_registered_peer_punches_once_it_has_the_reply() {
    let mut session = complete();
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Accepted,
        PathKind::Relayed,
        RangeList::new(),
    );
    assert_eq!(candidates_sent(&session.poll_actions()).len(), 1);
    let reply = Message::Candidates(Candidates {
        addresses: vec![address(7, 5000)],
    });
    control(&mut session, RELAYED, reply);
    let actions = session.poll_actions();
    assert!(candidates_sent(&actions).is_empty(), "no second Candidates");
    assert_eq!(
        punches(&actions),
        [(RELAYED, peer_id(9), vec![address(7, 5000)])]
    );
}

#[test]
fn candidates_on_a_direct_connection_get_a_reply_but_no_punch() {
    let mut session = complete();
    open(
        &mut session,
        DIRECT,
        9,
        Direction::Accepted,
        PathKind::Direct,
        RangeList::new(),
    );
    session.poll_actions();
    control(
        &mut session,
        DIRECT,
        Message::Candidates(Candidates::default()),
    );
    let actions = session.poll_actions();
    assert_eq!(candidates_sent(&actions).len(), 1);
    assert!(punches(&actions).is_empty());
}

#[test]
fn a_direct_connection_replaces_the_relayed_one_and_stops_the_punch() {
    // Peer 9 is higher, so this session keeps the direct connection.
    let mut session = complete();
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Dialed,
        PathKind::Relayed,
        RangeList::new(),
    );
    control(
        &mut session,
        RELAYED,
        Message::Candidates(Candidates::default()),
    );
    session.poll_actions();
    open(
        &mut session,
        DIRECT,
        9,
        Direction::Dialed,
        PathKind::Direct,
        RangeList::new(),
    );
    let actions = session.poll_actions();
    assert_eq!(closes(&actions), [(RELAYED, GoodbyeReason::Duplicate)]);
    assert!(actions.contains(&Action::StopPunching { peer: peer_id(9) }));
    assert_eq!(session.connection_count(), 1);
}

#[test]
fn the_higher_peer_stops_punching_but_leaves_the_choice_to_the_other() {
    let mut session = complete();
    open(
        &mut session,
        RELAYED,
        3,
        Direction::Accepted,
        PathKind::Relayed,
        RangeList::new(),
    );
    control(
        &mut session,
        RELAYED,
        Message::Candidates(Candidates::default()),
    );
    session.poll_actions();
    open(
        &mut session,
        DIRECT,
        3,
        Direction::Accepted,
        PathKind::Direct,
        RangeList::new(),
    );
    let actions = session.poll_actions();
    assert!(closes(&actions).is_empty());
    assert!(actions.contains(&Action::StopPunching { peer: peer_id(3) }));
    assert_eq!(session.connection_count(), 2);
}

#[test]
fn an_upgrade_is_let_in_at_the_connection_limit() {
    let mut config = Config::default();
    config.connections.max = 1;
    let mut session = session(config, &RangeList::from_chunks(0..40));
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Accepted,
        PathKind::Relayed,
        RangeList::new(),
    );
    session.poll_actions();

    // Another relayed connection with the same peer, or anyone else's, is not an upgrade.
    connect(
        &mut session,
        ConnId(3),
        9,
        Direction::Accepted,
        PathKind::Relayed,
    );
    connect(
        &mut session,
        ConnId(4),
        8,
        Direction::Accepted,
        PathKind::Direct,
    );
    let refused = vec![
        (ConnId(3), GoodbyeReason::TooManyConnections),
        (ConnId(4), GoodbyeReason::TooManyConnections),
    ];
    assert_eq!(closes(&session.poll_actions()), refused);

    connect(
        &mut session,
        DIRECT,
        9,
        Direction::Accepted,
        PathKind::Direct,
    );
    assert!(closes(&session.poll_actions()).is_empty());
    assert_eq!(session.connection_count(), 2);
    // A second direct connection is no longer an upgrade.
    connect(
        &mut session,
        ConnId(5),
        9,
        Direction::Dialed,
        PathKind::Direct,
    );
    assert_eq!(
        closes(&session.poll_actions()),
        [(ConnId(5), GoodbyeReason::TooManyConnections)]
    );
}

#[test]
fn requests_open_on_the_relayed_connection_move_to_the_direct_one() {
    let mut session = session(Config::default(), &RangeList::new());
    let all = RangeList::from_chunks(0..40);
    open(
        &mut session,
        RELAYED,
        9,
        Direction::Dialed,
        PathKind::Relayed,
        all.clone(),
    );
    control(
        &mut session,
        RELAYED,
        Message::Serving(Serving { serving: true }),
    );
    let mut before = chunk_requests(&session.poll_actions(), RELAYED);
    assert!(!before.is_empty());

    open(
        &mut session,
        DIRECT,
        9,
        Direction::Dialed,
        PathKind::Direct,
        all,
    );
    control(
        &mut session,
        DIRECT,
        Message::Serving(Serving { serving: true }),
    );
    let actions = session.poll_actions();
    assert_eq!(closes(&actions), [(RELAYED, GoodbyeReason::Duplicate)]);
    let mut after = chunk_requests(&actions, DIRECT);
    before.sort();
    after.sort();
    // Every chunk asked for on the relayed connection is asked for again.
    let covered = |(start, count): (u32, u32)| {
        after
            .iter()
            .any(|&(s, c)| s <= start && start + count <= s + c)
    };
    assert!(before.iter().all(|&r| covered(r)), "{before:?} {after:?}");
}

/// One connection with peer `peer` in a duplicate-resolution scenario.
#[derive(Debug, Clone, Copy)]
struct Dup {
    path: PathKind,
    direction: Direction,
}

fn dup() -> impl Strategy<Value = Dup> {
    (any::<bool>(), any::<bool>()).prop_map(|(relayed, dialed)| Dup {
        path: if relayed {
            PathKind::Relayed
        } else {
            PathKind::Direct
        },
        direction: if dialed {
            Direction::Dialed
        } else {
            Direction::Accepted
        },
    })
}

proptest! {
    /// The lower peer ID keeps exactly one connection: a direct one if any opened, and the
    /// earliest to open of that kind. The higher peer ID never closes one as a duplicate.
    #[test]
    fn duplicate_resolution(conns in prop::collection::vec(dup(), 1..6), lower in any::<bool>()) {
        let peer = if lower { LOCAL + 1 } else { LOCAL - 1 };
        let mut session = complete();
        let mut closed = Vec::new();
        for (n, c) in conns.iter().enumerate() {
            let id = ConnId(n as u64);
            open(&mut session, id, peer, c.direction, c.path, RangeList::new());
            closed.extend(closes(&session.poll_actions()));
        }
        prop_assert!(closed.iter().all(|&(_, reason)| reason == GoodbyeReason::Duplicate));
        if lower {
            let kept = (0..conns.len())
                .min_by_key(|&n| (conns[n].path, n))
                .map(|n| ConnId(n as u64))
                .unwrap();
            let expected: Vec<ConnId> =
                (0..conns.len() as u64).map(ConnId).filter(|&c| c != kept).collect();
            let mut got: Vec<ConnId> = closed.iter().map(|&(c, _)| c).collect();
            got.sort();
            prop_assert_eq!(got, expected);
            prop_assert_eq!(session.connection_count(), 1);
        } else {
            prop_assert!(closed.is_empty());
            prop_assert_eq!(session.connection_count(), conns.len());
        }
    }
}
