//! Plain-language explanations of a transfer's state (`specs/interfaces.md`: "explain, don't
//! decode"). The wording is shared with the web UI (D-119); how to apply a fix is each UI's own.

use crate::{Kind, SharingStop, Status, TransferInfo, words};

/// What the user can do about a transfer's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    /// Start it.
    Start,
    /// Start it again after a failure.
    Retry,
    /// Share it again after it stopped on its own.
    ShareAgain,
    /// Start it now, ignoring the queue.
    ForceStart,
}

/// A transfer's state in a sentence or two, and the fix it suggests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    /// What is happening, and why if it is stuck.
    pub text: String,
    /// More about it, for UIs to show on request, so the text stays short.
    pub detail: Option<String>,
    /// What would help, if anything.
    pub fix: Option<Fix>,
}

impl Explanation {
    fn new(text: impl Into<String>, fix: Option<Fix>) -> Self {
        Self {
            text: text.into(),
            detail: None,
            fix,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Explains what `transfer` is doing.
pub fn explain(transfer: &TransferInfo) -> Explanation {
    match &transfer.status {
        Status::Downloading if transfer.connections == 0 => {
            Explanation::new("Looking for peers that have it.", None)
        }
        Status::Downloading => {
            let mut text = format!("Downloading from {}.", connected(transfer));
            match transfer.eta {
                Some(eta) if eta < 60 => text.push_str(" Less than a minute left."),
                Some(eta) => text.push_str(&format!(" About {} left.", words::duration(eta))),
                None => {}
            }
            Explanation::new(text, None)
        }
        Status::Sharing if transfer.connections == 0 => {
            Explanation::new("Sharing. No one is downloading it right now.", None)
        }
        Status::Sharing => Explanation::new(format!("Sharing with {}.", connected(transfer)), None),
        Status::Stalled { reason } => {
            Explanation::new(format!("{reason} Tempest keeps trying."), None)
        }
        Status::Queued => Explanation::new(
            "Waiting for a free slot in the queue.",
            Some(Fix::ForceStart),
        ),
        Status::Stopped => Explanation::new("Stopped.", Some(Fix::Start)),
        Status::Completed => {
            Explanation::new("Complete. It stopped sharing.", Some(Fix::ShareAgain))
                .with_detail(stopped_by(transfer.stopped_by))
        }
        Status::Errored { reason } => Explanation::new(reason.clone(), Some(Fix::Retry)),
        Status::Checking if transfer.kind == Kind::Share => {
            Explanation::new("Reading its files to describe the bundle.", None)
        }
        Status::Checking => Explanation::new("Checking the data on disk against the bundle.", None),
        Status::Moving => Explanation::new(
            format!("Moving its files to {}.", transfer.save_folder.display()),
            None,
        ),
    }
}

/// Which sharing limit stopped a complete transfer, in a sentence.
fn stopped_by(stop: Option<SharingStop>) -> String {
    match stop {
        Some(SharingStop::Ratio { ratio }) => format!(
            "It reached its ratio limit: it had uploaded {} as much as it downloaded.",
            times(ratio)
        ),
        Some(SharingStop::Time { seconds }) => format!(
            "It reached its sharing time limit of {}.",
            words::duration(seconds)
        ),
        Some(SharingStop::Idle { seconds }) => format!(
            "No connected peer had wanted anything from it for {}, its idle timeout.",
            words::duration(seconds)
        ),
        None => "It reached one of its sharing limits.".to_owned(),
    }
}

/// "2×", "1.5×".
fn times(ratio: f64) -> String {
    let text = format!("{ratio:.2}");
    format!("{}×", text.trim_end_matches('0').trim_end_matches('.'))
}

/// "7 of 12 peers, 2 through relays": the connections, out of the peers known, and how many
/// are relayed.
pub fn connected(transfer: &TransferInfo) -> String {
    if transfer.peers > transfer.connections {
        format!("{} of {} peers", transfer.connections, transfer.peers)
            + &words::through_relays(transfer.relayed)
    } else {
        words::peers(transfer.connections, transfer.relayed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(status: Status, connections: usize, peers: usize, relayed: usize) -> TransferInfo {
        TransferInfo {
            status,
            connections,
            peers,
            relayed,
            ..TransferInfo::example(1, "a", 0)
        }
    }

    #[test]
    fn downloads_say_who_from_and_how_long() {
        let mut downloading = with(Status::Downloading, 7, 12, 2);
        downloading.eta = Some(170);
        assert_eq!(
            explain(&downloading).text,
            "Downloading from 7 of 12 peers, 2 through relays. About 3 minutes left."
        );
        downloading.eta = Some(59);
        assert_eq!(
            explain(&downloading).text,
            "Downloading from 7 of 12 peers, 2 through relays. Less than a minute left."
        );
        assert_eq!(
            explain(&with(Status::Downloading, 1, 1, 1)).text,
            "Downloading from 1 peer, 1 through a relay."
        );
        assert_eq!(
            explain(&with(Status::Downloading, 0, 3, 0)).text,
            "Looking for peers that have it."
        );
    }

    #[test]
    fn stuck_states_say_why_and_offer_a_fix() {
        let stalled = with(
            Status::Stalled {
                reason: "No peers found yet.".to_owned(),
            },
            0,
            0,
            0,
        );
        assert_eq!(
            explain(&stalled),
            Explanation::new("No peers found yet. Tempest keeps trying.", None)
        );
        let errored = Status::Errored {
            reason: "The disk is full.".to_owned(),
        };
        assert_eq!(
            explain(&with(errored, 0, 0, 0)),
            Explanation::new("The disk is full.", Some(Fix::Retry))
        );
        assert_eq!(
            explain(&with(Status::Queued, 0, 0, 0)).fix,
            Some(Fix::ForceStart)
        );
        assert_eq!(
            explain(&with(Status::Stopped, 0, 0, 0)).fix,
            Some(Fix::Start)
        );
    }

    #[test]
    fn a_completed_transfer_says_which_limit_stopped_it_on_request() {
        let detail = |stopped_by| {
            let transfer = TransferInfo {
                stopped_by,
                ..with(Status::Completed, 0, 0, 0)
            };
            let explanation = explain(&transfer);
            assert_eq!(explanation.text, "Complete. It stopped sharing.");
            explanation.detail.unwrap()
        };
        assert_eq!(
            detail(Some(SharingStop::Ratio { ratio: 1.5 })),
            "It reached its ratio limit: it had uploaded 1.5× as much as it downloaded."
        );
        assert!(detail(Some(SharingStop::Ratio { ratio: 2.0 })).contains(" 2× "));
        assert_eq!(
            detail(Some(SharingStop::Time { seconds: 7200 })),
            "It reached its sharing time limit of 2 hours."
        );
        assert_eq!(
            detail(Some(SharingStop::Idle { seconds: 600 })),
            "No connected peer had wanted anything from it for 10 minutes, its idle timeout."
        );
        assert_eq!(detail(None), "It reached one of its sharing limits.");
    }

    #[test]
    fn every_status_has_an_explanation_ending_a_sentence() {
        let statuses = [
            Status::Downloading,
            Status::Sharing,
            Status::Stalled {
                reason: "Found 3 peers, but couldn't connect to any.".to_owned(),
            },
            Status::Queued,
            Status::Stopped,
            Status::Completed,
            Status::Errored {
                reason: "The bundle's files were deleted.".to_owned(),
            },
            Status::Checking,
            Status::Moving,
        ];
        for status in statuses {
            for connections in [0, 2] {
                let text = explain(&with(status.clone(), connections, 4, 0)).text;
                assert!(text.ends_with('.'), "{status:?}: {text}");
            }
        }
    }
}
