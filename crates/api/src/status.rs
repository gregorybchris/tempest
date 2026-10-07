//! A transfer's status, as the UIs show it (`specs/interfaces.md`). The engine decides it.

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
