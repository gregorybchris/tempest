//! Ed25519 public keys as SubjectPublicKeyInfo (`protocol/connections.md`, Peer identity).
//!
//! Peers and relays present their key in TLS as a raw public key, which is a DER-encoded
//! SubjectPublicKeyInfo. For Ed25519 ([RFC 8410](https://www.rfc-editor.org/rfc/rfc8410)) DER
//! allows exactly one form: a fixed 12-byte prefix followed by the 32-byte key.

use crate::hash::{HASH_LEN, PeerId};

/// The bytes before the key: `SEQUENCE { SEQUENCE { OID 1.3.101.112 }, BIT STRING (0 unused) }`.
pub const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The length of an Ed25519 SubjectPublicKeyInfo.
pub const ED25519_SPKI_LEN: usize = ED25519_SPKI_PREFIX.len() + HASH_LEN;

/// Encodes a peer ID (an Ed25519 public key) as a SubjectPublicKeyInfo.
pub fn encode_spki(key: &PeerId) -> [u8; ED25519_SPKI_LEN] {
    let mut spki = [0; ED25519_SPKI_LEN];
    spki[..ED25519_SPKI_PREFIX.len()].copy_from_slice(&ED25519_SPKI_PREFIX);
    spki[ED25519_SPKI_PREFIX.len()..].copy_from_slice(key.as_bytes());
    spki
}

/// A SubjectPublicKeyInfo that is not an Ed25519 public key in DER form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a DER-encoded Ed25519 SubjectPublicKeyInfo")]
pub struct NotEd25519Spki;

/// Extracts the peer ID from a SubjectPublicKeyInfo, accepting only the DER form of an Ed25519
/// key.
///
/// This checks the encoding, not that the key is a valid curve point. The TLS handshake's
/// signature check proves that.
pub fn decode_spki(spki: &[u8]) -> Result<PeerId, NotEd25519Spki> {
    let key = spki
        .strip_prefix(&ED25519_SPKI_PREFIX)
        .and_then(|key| <[u8; HASH_LEN]>::try_from(key).ok())
        .ok_or(NotEd25519Spki)?;
    Ok(PeerId::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rfc_8410_example_decodes() {
        // The public key example from RFC 8410 §10.1, in DER.
        let spki = [
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 0x19, 0xbf,
            0x44, 0x09, 0x69, 0x84, 0xcd, 0xfe, 0x85, 0x41, 0xba, 0xc1, 0x67, 0xdc, 0x3b, 0x96,
            0xc8, 0x50, 0x86, 0xaa, 0x30, 0xb6, 0xb6, 0xcb, 0x0c, 0x5c, 0x38, 0xad, 0x70, 0x31,
            0x66, 0xe1,
        ];
        let key = decode_spki(&spki).unwrap();
        assert_eq!(
            key.to_string(),
            "19bf44096984cdfe8541bac167dc3b96c85086aa30b6b6cb0c5c38ad703166e1"
        );
        assert_eq!(encode_spki(&key), spki);
    }

    #[test]
    fn other_forms_are_rejected() {
        let good = encode_spki(&PeerId::new([7; 32]));
        assert_eq!(decode_spki(&good[..43]), Err(NotEd25519Spki));
        assert_eq!(
            decode_spki(&[&good[..], &[0]].concat()),
            Err(NotEd25519Spki)
        );
        // The X25519 OID (1.3.101.110) instead of Ed25519's.
        let mut x25519 = good;
        x25519[8] = 0x6e;
        assert_eq!(decode_spki(&x25519), Err(NotEd25519Spki));
        assert_eq!(decode_spki(&[]), Err(NotEd25519Spki));
    }

    proptest! {
        #[test]
        fn round_trips(key: [u8; 32]) {
            let key = PeerId::new(key);
            prop_assert_eq!(decode_spki(&encode_spki(&key)), Ok(key));
        }

        #[test]
        fn anything_that_decodes_re_encodes_identically(bytes: Vec<u8>) {
            if let Ok(key) = decode_spki(&bytes) {
                prop_assert_eq!(&encode_spki(&key)[..], &bytes[..]);
            }
        }
    }
}
