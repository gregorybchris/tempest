//! Tempest's relay (`protocol/relays.md`): it lends peers behind routers a public relay address,
//! forwards the traffic sent to that address inside each peer's relay connection, and tells
//! each peer its address as seen from the internet.
//!
//! - [`server`]: the [`Relay`] itself, over QUIC.
//! - [`registry`]: who may register and which lent socket each holds, without I/O.
//! - [`permissions`]: the forwarding rule, without I/O.
//!
//! The relay never sees inside the traffic it forwards, which stays end-to-end encrypted
//! between the peers.

pub mod permissions;
pub mod registry;
pub mod server;

pub use server::{Relay, RelayConfig};
pub use tempest_protocol::relay::DEFAULT_RELAY_PORT as DEFAULT_PORT;
