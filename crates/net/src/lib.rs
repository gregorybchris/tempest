//! Tempest's network layer: QUIC connections between peers.
//!
//! - [`addresses`]: the direct addresses a peer advertises.
//! - [`identity`]: a peer's Ed25519 key pair, stored once per installation (D-021).
//! - [`tls`]: TLS 1.3 with raw public keys, so the key is the peer's identity (D-020).
//! - [`endpoint`]: QUIC endpoints with one UDP socket per address family, and authenticated
//!   connections (D-018).
//! - [`home`]: registering with a home relay, which lends this peer a relay address.
//! - [`wrap`]: the layer beneath QUIC that carries packets through the home relay.
//! - [`portmap`]: asking the router to forward a port to this peer.
//! - [`stream`]: framed messages on QUIC streams.
//! - [`storage`]: a bundle's chunks on disk, read and written by a storage task (D-086).
//! - [`driver`]: a [`Node`] running `tempest-core` sessions over those connections and storage.
//! - [`limit`]: speed limits for a node and for each bundle.
//! - [`tracker`]: finding a bundle's peers through trackers.
//! - [`catalog`]: signing in to trackers' catalogs and listing bundles there.
//! - [`fetch`]: fetching a manifest from one peer without joining its swarm, as a tracker does
//!   to list a bundle.
//! - [`lan`]: finding peers on the local network through multicast.
//!
//! The protocol and strategy logic stays in `tempest-core`'s sans-IO state machines (D-068). This
//! crate moves their messages over real connections.

pub mod addresses;
pub mod catalog;
pub mod driver;
pub mod endpoint;
pub mod fetch;
pub mod home;
pub mod identity;
pub mod lan;
pub mod limit;
pub mod portmap;
pub mod storage;
pub mod stream;
pub mod tls;
pub mod tracker;
pub mod wrap;

pub use addresses::direct_addresses;
pub use driver::{
    BundleHandle, Destination, DriverError, Inspector, Node, NodeConfig, PeerDetail, PeerSink,
    Status, Summary,
};
pub use endpoint::{
    Connection, DEFAULT_PORT, Endpoint, EndpointConfig, HandshakeError, Incoming, closed_cause,
};
pub use fetch::{FetchError, FetchLimits, fetch_manifest};
pub use home::{HomeRelay, RelayChoice, RelayError, RelayFailure, RelayStatus};
pub use identity::{Identity, IdentityError};
pub use lan::{Lan, LanConfig};
pub use limit::{RateLimit, SpeedLimits};
pub use portmap::{
    MappingError, MappingFailure, MappingProtocol, PortMapping, PortMappingConfig,
    PortMappingStatus,
};
pub use storage::{
    Checkpoint, CheckpointError, Storage, StorageConfig, StorageError, StorageStats, Written,
};
pub use stream::{
    ReadMessageError, read_message, read_message_within, read_relay_message, write_message,
    write_relay_message,
};
pub use tls::UnexpectedPeer;
pub use tracker::{
    AnnounceConfig, Announcer, TrackerClient, TrackerError, TrackerState, public_addresses,
};
