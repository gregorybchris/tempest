//! Decodes manifests from arbitrary bytes, checks that whatever decodes re-encodes to the same
//! bytes, and lays out its chunks.

#![no_main]

use libfuzzer_sys::fuzz_target;
use tempest_protocol::{ChunkLayout, Decode, Encode, Manifest};

fuzz_target!(|data: &[u8]| {
    if let Ok(manifest) = Manifest::from_bytes(data) {
        assert_eq!(manifest.to_bytes(), data);
        assert_eq!(manifest.encoded_len(), data.len());
        let layout = ChunkLayout::new(&manifest);
        if let Some(last) = layout.chunk_count().checked_sub(1) {
            assert!(layout.locate(last).is_some());
        }
    }
});
