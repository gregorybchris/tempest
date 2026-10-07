//! Tempest's sans-IO peer logic: sessions, strategies and decision events.
//!
//! The core is a set of plain state machines (D-068). It never touches sockets, clocks, async
//! runtimes or global randomness: a driver tells it what happened and when, and carries out the
//! actions it asks for. The real client and the simulator drive the same code
//! (`specs/simulation.md`, One brain, two drivers).
//!
//! - [`session`]: [`Session`], one peer's participation in one bundle's swarm.
//! - [`io`]: what drivers report to a session and the [`Action`]s it asks for.
//! - [`config`]: settings, strategy choices and profiles.
//! - [`select`]: chunk selection strategies, over the [`availability`] index.
//! - [`pipeline`]: how many requests to keep open with each provider.
//! - [`upload`]: upload strategies.
//! - [`event`]: decision events with their reasons (D-006).
//! - [`content`]: an in-memory bundle store for tests and simulation.

pub mod availability;
pub mod chunks;
pub mod config;
pub mod content;
pub mod event;
pub mod io;
pub mod meter;
pub mod pipeline;
pub mod select;
pub mod session;
pub mod time;
pub mod upload;

pub use chunks::ChunkSet;
pub use config::{Config, Profile, Settings};
pub use event::{Event, EventKind};
pub use io::{Action, ConnId, ConnInfo, Inbound, Reply, RequestId, Response, StreamId};
pub use session::{LocalPeer, Session, SessionError};
pub use time::Time;

/// The random number generator every session draws from, seeded by its driver.
///
/// ChaCha8 gives the same sequence on every platform, so a seed replays a simulation exactly.
pub type Rng = rand_chacha::ChaCha8Rng;
