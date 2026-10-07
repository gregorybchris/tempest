//! The Tempest protocol: wire types, encoding and decoding, chunk trees and proofs.
//!
//! This crate implements the specification in the repository's `protocol/` folder and is meant
//! to be read alongside it. It is sans-IO: it never touches sockets, clocks, async runtimes or
//! global randomness.
//!
//! - [`wire`]: the binary primitives every message is built from.
//! - [`frame`]: how messages are framed on streams, with size limits.
//! - [`hash`]: hashes and the domain-separated hash functions.
//! - [`tree`]: chunks, chunk trees, roots, proofs and Data verification.
//! - [`manifest`]: manifests, their validation and bundle IDs.
//! - [`path`]: the rules for manifest paths and symlink targets.
//! - [`layout`]: bundle-wide chunk numbering.
//! - [`link`]: `tempest:` links.
//! - [`range`]: chunk ranges and canonical range lists.
//! - [`address`]: network addresses.
//! - [`message`]: peer messages.
//! - [`lan`]: LAN discovery messages.
//! - [`relay`]: relay control messages, Forward and Forwarded datagrams, and relay identities.
//! - [`tracker`]: the tracker API's JSON types.

pub mod address;
pub mod frame;
pub mod hash;
pub mod lan;
pub mod layout;
pub mod link;
pub mod manifest;
pub mod message;
pub mod path;
pub mod range;
pub mod relay;
mod text;
pub mod tracker;
pub mod tree;
pub mod wire;

pub use address::Address;
pub use hash::{Hash, PeerId};
pub use lan::LanMessage;
pub use layout::ChunkLayout;
pub use link::Link;
pub use manifest::{BundleId, Entry, EntryKind, FileInfo, Manifest};
pub use message::Message;
pub use range::{ChunkRange, RangeList};
pub use relay::{RelayDatagram, RelayIdentity, RelayMessage};
pub use wire::{Decode, DecodeError, Encode, Reader, Writer};
