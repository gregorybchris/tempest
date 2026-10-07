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
//! - [`range`]: chunk ranges and canonical range lists.
//! - [`address`]: network addresses.
//! - [`message`]: peer message types.

pub mod address;
pub mod frame;
pub mod hash;
pub mod message;
pub mod range;
pub mod tree;
pub mod wire;

pub use address::Address;
pub use hash::Hash;
pub use range::{ChunkRange, RangeList};
pub use wire::{Decode, DecodeError, Encode, Reader, Writer};
