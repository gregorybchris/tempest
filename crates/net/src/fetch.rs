//! Fetching a bundle's manifest from one peer without joining its swarm, as a tracker does to
//! list a bundle (`protocol/messages.md`, ManifestRequest).
//!
//! The fetcher dials the peer, opens the connection as any peer would with an empty Have, asks
//! for the manifest on a request stream, checks it against the bundle ID and closes the
//! connection. It never serves or requests chunks.

use std::time::Duration;

use tempest_core::io::PathKind;
use tempest_protocol::hash::manifest_hash;
use tempest_protocol::message::{GoodbyeReason, Have, Hello, MessageType, PeerRecord};
use tempest_protocol::wire::Decode;
use tempest_protocol::{Address, BundleId, Manifest, Message, RangeList};

use crate::endpoint::{Connection, Endpoint, HandshakeError};
use crate::stream::{ReadMessageError, read_message_within, write_message};

/// How a manifest fetch is bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchLimits {
    /// The largest encoded manifest accepted. A larger one is refused before it is read.
    pub max_manifest: u32,
    /// How long dialing one address may take.
    pub dial_timeout: Duration,
    /// How long the whole fetch may take, dialing included.
    pub timeout: Duration,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            max_manifest: 4 * 1024 * 1024,
            dial_timeout: Duration::from_secs(5),
            timeout: Duration::from_secs(30),
        }
    }
}

/// Why a manifest could not be fetched from a peer.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// No address of the peer answered.
    #[error("cannot reach the peer: {0}")]
    Unreachable(String),
    /// The connection failed partway.
    #[error("the connection failed: {0}")]
    Connection(String),
    /// The peer answered with something other than a Manifest, such as a Reject.
    #[error("the peer did not send the manifest: {0}")]
    Refused(String),
    /// The manifest is larger than [`FetchLimits::max_manifest`].
    #[error("the manifest is larger than {0} bytes")]
    TooLarge(u32),
    /// The manifest doesn't match the bundle ID, or doesn't decode.
    #[error("the peer sent a bad manifest: {0}")]
    BadManifest(String),
    /// The fetch took longer than [`FetchLimits::timeout`].
    #[error("the fetch timed out")]
    TimedOut,
}

/// Fetches `bundle`'s manifest from `peer`: its direct addresses in turn, then its relay
/// address.
pub async fn fetch_manifest(
    endpoint: &Endpoint,
    bundle: BundleId,
    peer: &PeerRecord,
    limits: FetchLimits,
) -> Result<Manifest, FetchError> {
    let fetch = async {
        let connection = dial(endpoint, peer, limits.dial_timeout).await?;
        let result = request(&connection, bundle, limits.max_manifest).await;
        let reason = match &result {
            Err(FetchError::BadManifest(_)) => GoodbyeReason::BadData,
            _ => GoodbyeReason::Done,
        };
        connection.close(reason, "");
        result
    };
    tokio::time::timeout(limits.timeout, fetch)
        .await
        .unwrap_or(Err(FetchError::TimedOut))
}

/// Dials the first of `peer`'s addresses that answers, accepting only its peer ID.
async fn dial(
    endpoint: &Endpoint,
    peer: &PeerRecord,
    timeout: Duration,
) -> Result<Connection, FetchError> {
    let direct = peer.addresses.iter().map(|a| (*a, PathKind::Direct));
    let relayed = peer.relay.iter().map(|a| (*a, PathKind::Relayed));
    let mut last = String::from("it has no address");
    for (address, path) in direct.chain(relayed) {
        match tokio::time::timeout(timeout, dial_one(endpoint, address, path, peer)).await {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => last = format!("{address}: {error}"),
            Err(_) => last = format!("{address}: timed out"),
        }
    }
    Err(FetchError::Unreachable(last))
}

async fn dial_one(
    endpoint: &Endpoint,
    address: Address,
    path: PathKind,
    peer: &PeerRecord,
) -> Result<Connection, HandshakeError> {
    let addr = address.socket_addr();
    match path {
        PathKind::Direct => endpoint.dial(addr, Some(peer.peer_id)).await,
        PathKind::Relayed => endpoint.dial_relayed(addr, Some(peer.peer_id)).await,
    }
}

/// Opens the connection for `bundle` and asks for its manifest.
async fn request(
    connection: &Connection,
    bundle: BundleId,
    max_manifest: u32,
) -> Result<Manifest, FetchError> {
    let failed = |error: &dyn std::fmt::Display| FetchError::Connection(error.to_string());
    let quic = connection.quic();

    // The opening sequence: this side holds nothing and accepts no Requests worth serving.
    let (mut control, _control_recv) = quic.open_bi().await.map_err(|e| failed(&e))?;
    let hello = Hello {
        bundle_id: bundle,
        capabilities: Vec::new(),
        client: tempest_protocol::CLIENT.to_owned(),
        max_request_chunks: 1,
        max_open_requests: 1,
        addresses: Vec::new(),
        relay: None,
    };
    let have = Have {
        ranges: RangeList::new(),
    };
    for message in [Message::Hello(hello), Message::Have(have)] {
        write_message(&mut control, &message)
            .await
            .map_err(|e| failed(&e))?;
    }

    // The provider holds a request stream that overtakes the opening, so there's no waiting.
    let (mut send, mut recv) = quic.open_bi().await.map_err(|e| failed(&e))?;
    write_message(&mut send, &Message::ManifestRequest)
        .await
        .map_err(|e| failed(&e))?;
    send.finish().map_err(|e| failed(&e))?;

    let limit = |message_type| match message_type {
        MessageType::Manifest => 1 + 4 + max_manifest,
        other => other.max_frame_length(),
    };
    let message = match read_message_within(&mut recv, limit).await {
        Ok(Some(message)) => message,
        Ok(None) => return Err(FetchError::Refused("no answer".to_owned())),
        Err(ReadMessageError::Decode(error)) if is_too_long(&error) => {
            return Err(FetchError::TooLarge(max_manifest));
        }
        Err(error) => return Err(failed(&error)),
    };
    let bytes = match message {
        Message::Manifest(manifest) => manifest.manifest,
        Message::Reject(reject) => {
            let reason = format!("{:?} {}", reject.reason, reject.message);
            return Err(FetchError::Refused(reason.trim().to_owned()));
        }
        other => {
            let message = format!("{:?} on a request stream", other.message_type());
            return Err(FetchError::Refused(message));
        }
    };
    if manifest_hash(&bytes) != bundle {
        let message = "it doesn't match the bundle ID".to_owned();
        return Err(FetchError::BadManifest(message));
    }
    Manifest::from_bytes(&bytes).map_err(|error| FetchError::BadManifest(error.to_string()))
}

fn is_too_long(error: &tempest_protocol::DecodeError) -> bool {
    matches!(error, tempest_protocol::DecodeError::FrameTooLong { .. })
}
