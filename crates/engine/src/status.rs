//! A transfer's status, as the UIs show it (`specs/interfaces.md`), and the rules that compute
//! it.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What a transfer is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Running and receiving data, or about to.
    Downloading,
    /// Complete and running, serving whoever wants data.
    Sharing,
    /// Running but getting nowhere.
    Stalled {
        /// Why, in a sentence.
        reason: String,
    },
    /// Waiting for a free slot in the queue.
    Queued,
    /// Stopped by the user.
    Stopped,
    /// Complete, and stopped sharing on its own.
    Completed,
    /// Stopped by a failure.
    Errored {
        /// What failed, in a sentence.
        reason: String,
    },
    /// Its data on disk is being checked.
    Checking,
    /// Its data is being moved to another folder.
    Moving,
}

/// How long a running download may go without progress before it counts as stalled.
pub const STALL_AFTER: Duration = Duration::from_secs(20);

/// Where a transfer is in its life, as the engine tracks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Not running.
    Idle,
    /// Its storage is being opened, which may mean hashing files.
    Opening,
    /// Its session is running.
    Running,
    /// Its session has been asked to stop.
    Stopping,
}

/// What the status of a transfer depends on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StatusInput<'a> {
    pub phase: Phase,
    /// The user wants it running.
    pub wanted: bool,
    /// It completed and stopped sharing on its own.
    pub done: bool,
    /// Why it failed, if it did.
    pub error: Option<&'a str>,
    /// Every chunk is stored.
    pub complete: bool,
    /// Whether the manifest is known.
    pub manifest: bool,
    /// Open or opening connections.
    pub connections: usize,
    /// Peers known, connected or not.
    pub known_peers: usize,
    /// How long it has gone without progress, counting from when it started running.
    pub idle: Duration,
}

impl Status {
    /// The status's name, as the API's `state` field gives it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Sharing => "sharing",
            Self::Stalled { .. } => "stalled",
            Self::Queued => "queued",
            Self::Stopped => "stopped",
            Self::Completed => "completed",
            Self::Errored { .. } => "errored",
            Self::Checking => "checking",
            Self::Moving => "moving",
        }
    }

    /// The status of a transfer in the state `input` describes.
    pub(crate) fn of(input: StatusInput<'_>) -> Self {
        if let Some(reason) = input.error {
            return Self::Errored {
                reason: reason.to_owned(),
            };
        }
        match input.phase {
            Phase::Opening => Self::Checking,
            Phase::Running if input.complete => Self::Sharing,
            Phase::Running if input.idle >= STALL_AFTER => Self::Stalled {
                reason: stall_reason(&input),
            },
            Phase::Running => Self::Downloading,
            Phase::Idle | Phase::Stopping if input.done => Self::Completed,
            Phase::Idle | Phase::Stopping if !input.wanted => Self::Stopped,
            Phase::Idle | Phase::Stopping => Self::Queued,
        }
    }
}

fn peers(count: usize) -> String {
    match count {
        1 => "1 peer".to_owned(),
        n => format!("{n} peers"),
    }
}

/// Why a running download is getting nowhere.
fn stall_reason(input: &StatusInput<'_>) -> String {
    match (input.connections, input.known_peers) {
        (0, 0) => "No peers found yet.".to_owned(),
        (0, known) => format!("Found {}, but couldn't connect to any.", peers(known)),
        (connected, _) if !input.manifest => format!(
            "Connected to {}, but none has sent the bundle's description yet.",
            peers(connected)
        ),
        (connected, _) => format!(
            "Connected to {}, but none is sending the missing data.",
            peers(connected)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> StatusInput<'static> {
        StatusInput {
            phase: Phase::Running,
            wanted: true,
            done: false,
            error: None,
            complete: false,
            manifest: true,
            connections: 2,
            known_peers: 5,
            idle: Duration::ZERO,
        }
    }

    #[test]
    fn running_transfers_download_share_or_stall() {
        assert_eq!(Status::of(running()), Status::Downloading);
        let complete = StatusInput {
            complete: true,
            idle: Duration::from_secs(3600),
            ..running()
        };
        assert_eq!(Status::of(complete), Status::Sharing);

        let stalled = |connections, known_peers, manifest| {
            let input = StatusInput {
                connections,
                known_peers,
                manifest,
                idle: STALL_AFTER,
                ..running()
            };
            match Status::of(input) {
                Status::Stalled { reason } => reason,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(stalled(0, 0, false), "No peers found yet.");
        assert_eq!(
            stalled(0, 1, false),
            "Found 1 peer, but couldn't connect to any."
        );
        assert_eq!(
            stalled(3, 4, false),
            "Connected to 3 peers, but none has sent the bundle's description yet."
        );
        assert_eq!(
            stalled(1, 4, true),
            "Connected to 1 peer, but none is sending the missing data."
        );
    }

    #[test]
    fn idle_transfers_are_queued_stopped_or_completed() {
        let idle = |wanted, done| {
            Status::of(StatusInput {
                phase: Phase::Idle,
                wanted,
                done,
                ..running()
            })
        };
        assert_eq!(idle(true, false), Status::Queued);
        assert_eq!(idle(false, false), Status::Stopped);
        assert_eq!(idle(true, true), Status::Completed);
        assert_eq!(idle(false, true), Status::Completed);
        let stopping = StatusInput {
            phase: Phase::Stopping,
            wanted: false,
            ..running()
        };
        assert_eq!(Status::of(stopping), Status::Stopped);
    }

    #[test]
    fn names_match_the_serialized_state() {
        let reason = || "why".to_owned();
        for status in [
            Status::Downloading,
            Status::Sharing,
            Status::Stalled { reason: reason() },
            Status::Queued,
            Status::Stopped,
            Status::Completed,
            Status::Errored { reason: reason() },
            Status::Checking,
            Status::Moving,
        ] {
            let json = serde_json::to_value(&status).unwrap();
            assert_eq!(json["state"], status.name());
        }
    }

    #[test]
    fn errors_and_checking_come_first() {
        let errored = StatusInput {
            error: Some("disk full"),
            ..running()
        };
        assert_eq!(
            Status::of(errored),
            Status::Errored {
                reason: "disk full".into()
            }
        );
        let opening = StatusInput {
            phase: Phase::Opening,
            ..running()
        };
        assert_eq!(Status::of(opening), Status::Checking);
    }
}
