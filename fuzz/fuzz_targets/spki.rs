//! Parses arbitrary bytes as a raw public key and checks that whatever parses re-encodes to the
//! same bytes.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::spki::{decode_spki, encode_spki};

fuzz_target!(|bytes: &[u8]| {
    if let Ok(key) = decode_spki(bytes) {
        assert_eq!(&encode_spki(&key)[..], bytes);
    }
});
