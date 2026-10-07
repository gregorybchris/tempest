//! The manifest test vector from `protocol/content.md`.

use tempest_protocol::hash::leaf_hash;
use tempest_protocol::{ChunkLayout, Decode, Encode, Entry, FileInfo, Manifest};

const MANIFEST_HEX: &str = "01000000076578616d706c65000000000000018bcfe568000000000d74656d7065737420302e312e30000000040000000005612e7478740000000000000003609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1000000018bcfe56800000000000e646f63732f656d7074792e74787400000000000000006e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d000000018bcfe568000100000009646f63732f6c696e6b000000082e2e2f612e7478740200000009656d7074792d646972";

const BUNDLE_ID: &str = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";

const TIME: i64 = 1_700_000_000_000;

fn example() -> Manifest {
    let file = |data: &[u8]| FileInfo {
        size: data.len() as u64,
        root: leaf_hash(data),
        executable: false,
        modified: TIME,
    };
    Manifest::new(
        "example",
        "",
        TIME,
        "tempest 0.1.0",
        vec![
            Entry::directory("empty-dir"),
            Entry::symlink("docs/link", "../a.txt"),
            Entry::file("docs/empty.txt", file(b"")),
            Entry::file("a.txt", file(b"abc")),
        ],
    )
    .unwrap()
}

#[test]
fn manifest_encodes_to_the_vector() {
    let bytes = example().to_bytes();
    assert_eq!(bytes.len(), 212);
    assert_eq!(hex::encode(&bytes), MANIFEST_HEX);
    assert_eq!(example().bundle_id().to_string(), BUNDLE_ID);
}

#[test]
fn vector_decodes_to_the_manifest() {
    let bytes = hex::decode(MANIFEST_HEX).unwrap();
    assert_eq!(Manifest::from_bytes(&bytes), Ok(example()));
}

#[test]
fn vector_chunk_layout() {
    let layout = ChunkLayout::new(&example());
    // a.txt has one chunk, docs/empty.txt one empty chunk.
    assert_eq!(layout.chunk_count(), 2);
    assert_eq!(layout.files()[1].entry, 1);
    assert_eq!(layout.files()[1].first_chunk, 1);
}
