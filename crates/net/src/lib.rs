//! Tempest's network layer: QUIC connections between peers.
//!
//! - [`identity`]: a peer's Ed25519 key pair, stored once per installation (D-021).
//! - [`tls`]: TLS 1.3 with raw public keys, so the key is the peer's identity (D-020).
//! - [`endpoint`]: QUIC endpoints with one UDP socket per address family, and authenticated
//!   connections (D-018).
//! - [`stream`]: framed messages on QUIC streams.
//! - [`storage`]: a bundle's chunks on disk, read and written by a storage task (D-086).
//!
//! The protocol and strategy logic stays in `tempest-core`'s sans-IO state machines (D-068). This
//! crate moves their messages over real connections.

pub mod endpoint;
pub mod identity;
pub mod storage;
pub mod stream;
pub mod tls;

pub use endpoint::{
    Connection, DEFAULT_PORT, Endpoint, EndpointConfig, HandshakeError, Incoming, closed_cause,
};
pub use identity::{Identity, IdentityError};
pub use storage::{Storage, StorageConfig, StorageError, StorageStats, Written};
pub use stream::{ReadMessageError, read_message, read_message_within, write_message};
pub use tls::UnexpectedPeer;
