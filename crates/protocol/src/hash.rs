//! Hashes and the domain-separated hash functions (`protocol/content.md`).
//!
//! Every kind of hash in Tempest starts its input with its own one-byte prefix, so a leaf, an
//! inner node and a manifest can never hash to the same value.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// The length of a hash in bytes.
pub const HASH_LEN: usize = 32;

/// The prefix of a leaf hash's input.
pub const LEAF_PREFIX: u8 = 0x00;
/// The prefix of an inner node hash's input.
pub const NODE_PREFIX: u8 = 0x01;
/// The prefix of a bundle ID's input.
pub const MANIFEST_PREFIX: u8 = 0x02;

/// A peer's peer ID: its 32-byte Ed25519 public key (`protocol/connections.md`).
///
/// It is not a digest, but it has the same size, encoding and text form as one.
pub type PeerId = Hash;

/// A 32-byte SHA-256 digest: a root, a tree node, a bundle ID or a peer ID.
///
/// As text, a hash is 64 lowercase hexadecimal characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Hash([u8; HASH_LEN]);

impl Hash {
    /// The all-zero hash, which is also the value of a padding leaf.
    pub const ZERO: Self = Self([0; HASH_LEN]);

    /// Wraps 32 bytes.
    pub const fn new(bytes: [u8; HASH_LEN]) -> Self {
        Self(bytes)
    }

    /// The hash's bytes.
    pub const fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }
}

fn sha256(parts: &[&[u8]]) -> Hash {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    Hash(hasher.finalize().into())
}

/// The leaf hash of a chunk: `SHA-256(0x00 ‖ chunk)`.
pub fn leaf_hash(chunk: &[u8]) -> Hash {
    sha256(&[&[LEAF_PREFIX], chunk])
}

/// The hash of an inner node: `SHA-256(0x01 ‖ left ‖ right)`.
pub fn node_hash(left: &Hash, right: &Hash) -> Hash {
    sha256(&[&[NODE_PREFIX], &left.0, &right.0])
}

/// The bundle ID of an encoded manifest: `SHA-256(0x02 ‖ manifest bytes)`.
pub fn manifest_hash(manifest: &[u8]) -> Hash {
    sha256(&[&[MANIFEST_PREFIX], manifest])
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({self})")
    }
}

/// Why text could not be parsed as a hash.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a hash must be 64 lowercase hexadecimal characters")]
pub struct ParseHashError;

impl FromStr for Hash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        fn nibble(c: u8) -> Result<u8, ParseHashError> {
            match c {
                b'0'..=b'9' => Ok(c - b'0'),
                b'a'..=b'f' => Ok(c - b'a' + 10),
                _ => Err(ParseHashError),
            }
        }

        let s = s.as_bytes();
        if s.len() != 2 * HASH_LEN {
            return Err(ParseHashError);
        }
        let mut bytes = [0; HASH_LEN];
        for (byte, pair) in bytes.iter_mut().zip(s.as_chunks::<2>().0) {
            *byte = nibble(pair[0])? << 4 | nibble(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_leaf_is_sha256_of_zero_byte() {
        assert_eq!(
            leaf_hash(&[]).to_string(),
            "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
        );
    }

    #[test]
    fn hex_round_trips() {
        let text = "609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1";
        let hash: Hash = text.parse().unwrap();
        assert_eq!(hash, leaf_hash(b"abc"));
        assert_eq!(hash.to_string(), text);
    }

    #[test]
    fn hex_rejects_uppercase_and_wrong_length() {
        let upper = "609F6E36D2405585188D5CFD761F407C7CC46A7D3F314C88270469DDE315FCD1";
        assert_eq!(upper.parse::<Hash>(), Err(ParseHashError));
        assert_eq!("00".parse::<Hash>(), Err(ParseHashError));
        assert_eq!("é".repeat(32).parse::<Hash>(), Err(ParseHashError));
    }
}
