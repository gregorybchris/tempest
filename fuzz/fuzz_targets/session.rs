//! Drives a session with arbitrary connections, messages, responses and timers, under arbitrary
//! strategies, as a misbehaving peer or a confused driver might. The session must never panic,
//! including on the overflow checks guarding its accounting.

#![no_main]

use std::sync::OnceLock;
use std::time::Duration;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tempest_core::config::{ChunkSelection, Endgame, Superseed, Upload, WhenComplete};
use tempest_core::content::MemoryContent;
use tempest_core::io::{Closed, ConnInfo, Direction, PathKind};
use tempest_core::{
    Config, ConnId, Inbound, LocalPeer, RequestId, Response, Session, StreamId, Time,
};
use tempest_protocol::message::{
    GoodbyeReason, Have, HaveUpdate, Hello, ManifestMessage, PeerRecord, Peers, Reject,
    RejectReason, Request, Serving, Want, split_message_frame,
};
use tempest_protocol::tree::VerifyError;
use tempest_protocol::{Hash, Message, RangeList};

#[derive(Debug, Arbitrary)]
struct Input {
    complete: bool,
    strategies: Strategies,
    ops: Vec<Op>,
}

#[derive(Debug, Arbitrary)]
struct Strategies {
    selection: u8,
    upload: u8,
    endgame: u8,
    round_robin: bool,
    superseed: bool,
    adaptive: bool,
    direct_send: bool,
}

#[derive(Debug, Arbitrary)]
enum Op {
    Connect {
        peer: u8,
        dialed: bool,
    },
    Control {
        conn: u8,
        message: Msg,
    },
    Request {
        conn: u8,
        stream: u8,
        message: Msg,
    },
    Response {
        conn: u8,
        request: u8,
        response: Resp,
    },
    Reset {
        conn: u8,
        stream: u8,
    },
    DataSent {
        conn: u8,
        stream: u8,
    },
    Closed {
        conn: u8,
    },
    DialFailed {
        peer: u8,
    },
    Wait {
        millis: u32,
    },
    Rtt {
        conn: u8,
        millis: u16,
    },
    Position(u8),
    AddPeers(u8),
}

#[derive(Debug, Arbitrary)]
enum Msg {
    Hello {
        right_bundle: bool,
        max_log: u8,
        max_open: u8,
    },
    Have(Vec<u8>),
    HaveUpdate(Vec<u8>, Vec<u8>),
    Want(bool),
    Serving(bool),
    Peers(u8),
    Request {
        start: u8,
        count: u8,
    },
    ManifestRequest,
    Raw(Vec<u8>),
}

#[derive(Debug, Arbitrary)]
enum Resp {
    Verified,
    Failed,
    Reject(u8),
    Manifest { right: bool },
}

/// A bundle of 3 + 1 + 40 chunks: a short file, an empty one and a longer one.
fn content() -> &'static MemoryContent {
    static CONTENT: OnceLock<MemoryContent> = OnceLock::new();
    CONTENT.get_or_init(|| {
        MemoryContent::new(
            "fuzz",
            vec![
                ("a".to_owned(), vec![1; 40_000]),
                ("b".to_owned(), Vec::new()),
                ("c".to_owned(), vec![2; 40 * 16 * 1024]),
            ],
        )
    })
}

fn ranges(chunks: &[u8]) -> RangeList {
    RangeList::from_chunks(chunks.iter().map(|&c| u32::from(c % 48)))
}

fn message(msg: Msg) -> Option<Message> {
    Some(match msg {
        Msg::Hello {
            right_bundle,
            max_log,
            max_open,
        } => Message::Hello(Hello {
            bundle_id: if right_bundle {
                content().manifest().bundle_id()
            } else {
                Hash::ZERO
            },
            capabilities: Vec::new(),
            client: String::new(),
            max_request_chunks: 1 << (max_log % 11),
            max_open_requests: u32::from(max_open).max(1),
            addresses: Vec::new(),
            relay: None,
        }),
        Msg::Have(chunks) => Message::Have(Have {
            ranges: ranges(&chunks),
        }),
        Msg::HaveUpdate(added, removed) => Message::HaveUpdate(HaveUpdate {
            added: ranges(&added),
            removed: ranges(&removed),
        }),
        Msg::Want(wanted) => Message::Want(Want { wanted }),
        Msg::Serving(serving) => Message::Serving(Serving { serving }),
        Msg::Peers(count) => Message::Peers(Peers {
            peers: (0..count % 51).map(record).collect(),
        }),
        Msg::Request { start, count } => Message::Request(Request {
            start: u32::from(start),
            count: u32::from(count),
        }),
        Msg::ManifestRequest => Message::ManifestRequest,
        Msg::Raw(bytes) => {
            let (frame, _) = split_message_frame(&bytes).ok()??;
            Message::decode(frame).ok()?
        }
    })
}

fn record(n: u8) -> PeerRecord {
    PeerRecord {
        peer_id: Hash::new([n % 8 + 3; 32]),
        addresses: Vec::new(),
        relay: None,
    }
}

fn config(strategies: &Strategies) -> Config {
    let mut config = Config::default();
    config.connections.max = 4;
    config.connections.max_known_peers = 6;
    config.upload.max_open_requests = 4;
    config.upload.parallel_responses = 2;
    config.strategies.chunk_selection = match strategies.selection % 4 {
        0 => ChunkSelection::RarestFirst,
        1 => ChunkSelection::Random,
        2 => ChunkSelection::Sequential,
        _ => ChunkSelection::Streaming,
    };
    config.strategies.upload = match strategies.upload % 4 {
        0 => Upload::Bittorrent,
        1 => Upload::Propshare,
        2 => Upload::Fairtorrent,
        _ => Upload::Cooperative,
    };
    config.strategies.endgame = match strategies.endgame % 3 {
        0 => Endgame::Limited,
        1 => Endgame::Classic,
        _ => Endgame::Off,
    };
    config.strategies.when_complete = if strategies.round_robin {
        WhenComplete::RoundRobin
    } else {
        WhenComplete::Unchanged
    };
    config.strategies.superseed = if strategies.superseed {
        Superseed::On
    } else {
        Superseed::Off
    };
    config.requests.adaptive = strategies.adaptive;
    config.requests.chunks = 4;
    config.lingering.direct_send = strategies.direct_send;
    config
}

fn response(resp: Resp) -> Response {
    match resp {
        Resp::Verified => Response::Data(Ok(())),
        Resp::Failed => Response::Data(Err(VerifyError::RootMismatch)),
        Resp::Reject(code) => Response::Reject(Reject {
            reason: RejectReason::from_code(u16::from(code % 4)),
            message: String::new(),
        }),
        Resp::Manifest { right } => Response::Manifest(ManifestMessage {
            manifest: if right {
                content().manifest_bytes()
            } else {
                b"junk".to_vec()
            },
        }),
    }
}

fuzz_target!(|input: Input| {
    let content = content();
    let local = LocalPeer {
        peer_id: Hash::new([5; 32]),
        addresses: Vec::new(),
        relay: None,
    };
    let config = config(&input.strategies);
    let mut session = if input.complete {
        let all = RangeList::from_chunks(0..44);
        Session::with_manifest(config, local, content.manifest().clone(), &all, 0).unwrap()
    } else {
        Session::new(config, local, content.manifest().bundle_id(), 0).unwrap()
    };

    let mut now = Time::ZERO;
    let mut next_conn = 0u64;
    for op in input.ops {
        // Connection IDs are never reused; ops address one of the four newest.
        let conn = |c: u8| ConnId(next_conn.saturating_sub(1 + u64::from(c % 4)));
        match op {
            Op::Connect { peer, dialed } => {
                let info = ConnInfo {
                    peer: Hash::new([peer % 4 + 3; 32]),
                    direction: if dialed {
                        Direction::Dialed
                    } else {
                        Direction::Accepted
                    },
                    path: PathKind::Direct,
                };
                session.handle_connected(now, ConnId(next_conn), info);
                next_conn += 1;
            }
            Op::Control {
                conn: c,
                message: m,
            } => {
                if let Some(m) = message(m) {
                    session.handle_message(now, conn(c), Inbound::Control(m));
                }
            }
            Op::Request {
                conn: c,
                stream,
                message: m,
            } => {
                if let Some(m) = message(m) {
                    let stream = StreamId(u64::from(stream));
                    session.handle_message(now, conn(c), Inbound::Request { stream, message: m });
                }
            }
            Op::Response {
                conn: c,
                request,
                response: r,
            } => {
                let request = RequestId(u64::from(request));
                let inbound = Inbound::Response {
                    request,
                    response: response(r),
                };
                session.handle_message(now, conn(c), inbound);
            }
            Op::Reset { conn: c, stream } => {
                let stream = StreamId(u64::from(stream));
                session.handle_message(now, conn(c), Inbound::RequestReset { stream });
            }
            Op::DataSent { conn: c, stream } => {
                let stream = StreamId(u64::from(stream));
                session.handle_message(now, conn(c), Inbound::DataSent { stream });
            }
            Op::Closed { conn: c } => {
                let closed = Closed::Goodbye {
                    reason: GoodbyeReason::Done,
                    message: String::new(),
                };
                session.handle_closed(now, conn(c), closed);
            }
            Op::DialFailed { peer } => {
                session.handle_dial_failed(now, Hash::new([peer % 4 + 3; 32]))
            }
            Op::Rtt { conn: c, millis } => {
                session.handle_rtt(now, conn(c), Duration::from_millis(u64::from(millis)));
            }
            Op::Position(chunk) => session.set_playback_position(now, u32::from(chunk)),
            Op::AddPeers(count) => session.add_peers(now, (0..count % 12).map(record)),
            Op::Wait { millis } => {
                now += Duration::from_millis(u64::from(millis));
                if session.next_timer().is_some_and(|t| t <= now) {
                    session.handle_timer(now);
                    // A driver calls back at the next timer, so it must move on.
                    if let Some(next) = session.next_timer() {
                        assert!(next > now, "timer at {next} not after {now}");
                    }
                }
            }
        }
        session.poll_actions();
        session.poll_events();
    }
});
