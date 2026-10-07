//! The data of the engine's events (`GET /v1/events`), and the events as a client reads them.

use serde::{Deserialize, Serialize};
use tempest_protocol::BundleId;

use crate::TransferInfo;

/// The data of a `decision` event: a session's decision event (D-006).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionInfo {
    /// The transfer whose session decided.
    #[serde(serialize_with = "crate::display", deserialize_with = "crate::parse")]
    pub id: BundleId,
    /// When, in seconds on the session's clock.
    pub time: f64,
    /// What kind of event, such as `requested` or `closed`.
    pub kind: String,
    /// Why, for kinds that have a reason.
    pub reason: Option<String>,
    /// The event in a sentence.
    pub text: String,
}

/// The data of a `removed` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedInfo {
    /// The transfer that was removed.
    #[serde(serialize_with = "crate::display", deserialize_with = "crate::parse")]
    pub id: BundleId,
}

/// The data of a `decisions_missed` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissedInfo {
    /// The transfer whose decision events were dropped.
    #[serde(serialize_with = "crate::display", deserialize_with = "crate::parse")]
    pub id: BundleId,
    /// How many were dropped.
    pub count: u64,
}

/// The data of a `snapshot` or `reset` event: every transfer, in queue order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotInfo {
    /// The transfers.
    pub transfers: Vec<TransferInfo>,
}

/// An event from the engine's stream.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiEvent {
    /// Every transfer, first on a new stream.
    Snapshot(Vec<TransferInfo>),
    /// Every transfer, because events the client asked for are gone: replace the whole view.
    Reset(Vec<TransferInfo>),
    /// A transfer that was added or changed.
    Transfer(Box<TransferInfo>),
    /// A transfer that was removed.
    Removed(RemovedInfo),
    /// A session's decision event.
    Decision(DecisionInfo),
    /// Decision events dropped because they came too fast.
    DecisionsMissed(MissedInfo),
}

impl ApiEvent {
    /// The event named `name` carrying `data`, or `None` for a name this client doesn't know,
    /// which a later engine may send.
    pub fn decode(name: &str, data: &str) -> Result<Option<Self>, serde_json::Error> {
        Ok(Some(match name {
            "snapshot" => Self::Snapshot(serde_json::from_str::<SnapshotInfo>(data)?.transfers),
            "reset" => Self::Reset(serde_json::from_str::<SnapshotInfo>(data)?.transfers),
            "transfer" => Self::Transfer(serde_json::from_str(data)?),
            "removed" => Self::Removed(serde_json::from_str(data)?),
            "decision" => Self::Decision(serde_json::from_str(data)?),
            "decisions_missed" => Self::DecisionsMissed(serde_json::from_str(data)?),
            _ => return Ok(None),
        }))
    }
}
