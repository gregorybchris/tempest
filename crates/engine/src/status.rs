//! The rules that decide a transfer's [`Status`].

use std::time::Duration;

pub use tempest_api::Status;

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
    /// Its files are being moved.
    Moving,
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

/// The status of a transfer in the state `input` describes.
pub(crate) fn of(input: StatusInput<'_>) -> Status {
    if let Some(reason) = input.error {
        return Status::Errored {
            reason: reason.to_owned(),
        };
    }
    match input.phase {
        Phase::Opening => Status::Checking,
        Phase::Moving => Status::Moving,
        Phase::Running if input.complete => Status::Sharing,
        Phase::Running if input.idle >= STALL_AFTER => Status::Stalled {
            reason: stall_reason(&input),
        },
        Phase::Running => Status::Downloading,
        Phase::Idle | Phase::Stopping if input.done => Status::Completed,
        Phase::Idle | Phase::Stopping if !input.wanted => Status::Stopped,
        Phase::Idle | Phase::Stopping => Status::Queued,
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
        assert_eq!(of(running()), Status::Downloading);
        let complete = StatusInput {
            complete: true,
            idle: Duration::from_secs(3600),
            ..running()
        };
        assert_eq!(of(complete), Status::Sharing);

        let stalled = |connections, known_peers, manifest| {
            let input = StatusInput {
                connections,
                known_peers,
                manifest,
                idle: STALL_AFTER,
                ..running()
            };
            match of(input) {
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
            of(StatusInput {
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
        assert_eq!(of(stopping), Status::Stopped);
    }

    #[test]
    fn errors_and_checking_come_first() {
        let errored = StatusInput {
            error: Some("disk full"),
            ..running()
        };
        assert_eq!(
            of(errored),
            Status::Errored {
                reason: "disk full".into()
            }
        );
        let opening = StatusInput {
            phase: Phase::Opening,
            ..running()
        };
        assert_eq!(of(opening), Status::Checking);
        let moving = StatusInput {
            phase: Phase::Moving,
            ..running()
        };
        assert_eq!(of(moving), Status::Moving);
    }
}
