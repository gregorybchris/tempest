//! Tempest's network layer: QUIC connections between peers.
//!
//! - [`addresses`]: the direct addresses a peer advertises.
//! - [`identity`]: a peer's Ed25519 key pair, stored once per installation (D-021).
//! - [`tls`]: TLS 1.3 with raw public keys, so the key is the peer's identity (D-020).
//! - [`endpoint`]: QUIC endpoints with one UDP socket per address family, and authenticated
//!   connections (D-018).
//! - [`stream`]: framed messages on QUIC streams.
//! - [`storage`]: a bundle's chunks on disk, read and written by a storage task (D-086).
//! - [`driver`]: a [`Node`] running `tempest-core` sessions over those connections and storage.
//! - [`tracker`]: finding a bundle's peers through trackers.
//! - [`lan`]: finding peers on the local network through multicast.
//!
//! The protocol and strategy logic stays in `tempest-core`'s sans-IO state machines (D-068). This
//! crate moves their messages over real connections.

pub mod addresses;
pub mod driver;
pub mod endpoint;
pub mod identity;
pub mod lan;
pub mod storage;
pub mod stream;
pub mod tls;
pub mod tracker;

pub use addresses::direct_addresses;
pub use driver::{
    BundleHandle, Destination, DriverError, Node, NodeConfig, PeerSink, Status, Summary,
};
pub use endpoint::{
    Connection, DEFAULT_PORT, Endpoint, EndpointConfig, HandshakeError, Incoming, closed_cause,
};
pub use identity::{Identity, IdentityError};
pub use lan::{Lan, LanConfig};
pub use storage::{Storage, StorageConfig, StorageError, StorageStats, Written};
pub use stream::{ReadMessageError, read_message, read_message_within, write_message};
pub use tls::UnexpectedPeer;
pub use tracker::{
    AnnounceConfig, Announcer, TrackerClient, TrackerError, TrackerState, public_addresses,
};
