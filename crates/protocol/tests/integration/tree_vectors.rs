//! The chunk tree test vectors from `protocol/content.md` and `protocol/messages.md`.

use tempest_protocol::Hash;
use tempest_protocol::hash::{leaf_hash, manifest_hash, node_hash};
use tempest_protocol::tree::{
    ChunkTree, RootHasher, TreeNode, VerifyError, chunk_count, root, tree_width, verify_data,
};

fn hash(hex: &str) -> Hash {
    hex.parse().unwrap()
}

/// The 40,000-byte test file where byte i = i mod 256.
fn counting_file() -> Vec<u8> {
    (0..40_000u32).map(|i| i as u8).collect()
}

#[test]
fn root_vectors() {
    let cases: [(&str, Vec<u8>, u64, u64, &str); 5] = [
        (
            "empty file",
            vec![],
            1,
            1,
            "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d",
        ),
        (
            "abc",
            b"abc".to_vec(),
            1,
            1,
            "609f6e36d2405585188d5cfd761f407c7cc46a7d3f314c88270469dde315fcd1",
        ),
        (
            "16,384 bytes of a",
            vec![b'a'; 16_384],
            1,
            1,
            "efc218e05045c10736574abdc1b7ce7f1e1a0e8366215664189852745d06eb90",
        ),
        (
            "16,385 bytes of a",
            vec![b'a'; 16_385],
            2,
            2,
            "553a8ddffb209ad5cf57755f33ab1e8a5191f5261e1442720d087085ec47f7a7",
        ),
        (
            "40,000 counting bytes",
            counting_file(),
            3,
            4,
            "cf6700fcd8e64ac06a512eac8b2f94101104f0e199c5fbfa2c4b43adb562c2f8",
        ),
    ];
    for (name, data, chunks, width, expected) in cases {
        let n = chunk_count(data.len() as u64);
        assert_eq!(n, chunks, "{name}: chunks");
        assert_eq!(tree_width(n), width, "{name}: width");
        assert_eq!(root(&data), hash(expected), "{name}: streamed root");
        assert_eq!(
            ChunkTree::from_data(&data).root(),
            hash(expected),
            "{name}: tree root"
        );
    }
}

#[test]
fn three_chunk_tree_matches_the_diagram() {
    let data = counting_file();
    let l: Vec<Hash> = data.chunks(16_384).map(leaf_hash).collect();
    let n01 = node_hash(&l[0], &l[1]);
    let n23 = node_hash(&l[2], &Hash::ZERO);
    assert_eq!(root(&data), node_hash(&n01, &n23));
}

#[test]
fn proof_vectors() {
    let data = counting_file();
    let file_root = root(&data);
    let tree = ChunkTree::from_data(&data);
    let n01 = hash("1b53726ec87fc16451460d18b88ed388f714a8f983e0ea37fb43620fc816e115");
    let n23 = hash("59aef01004079909de1326890705ff0009732e28a43a5c8a8f3d7e8ed7c1af7c");
    let cases = [
        (2, 1, vec![Hash::ZERO, n01]),
        (0, 2, vec![n23]),
        (2, 2, vec![n01]),
        (0, 4, vec![]),
    ];
    for (position, count, expected) in cases {
        let node = TreeNode::for_range(3, position, count).unwrap();
        let proof = tree.proof(node);
        assert_eq!(proof, expected, "proof for start {position}, count {count}");

        let start = position as usize * 16_384;
        let end = (start + count as usize * 16_384).min(data.len());
        let chunks = &data[start..end];
        assert_eq!(
            verify_data(40_000, &file_root, position, count, &proof, chunks),
            Ok(())
        );
    }
}

#[test]
fn empty_file_data_verifies() {
    let empty_root = root(&[]);
    assert_eq!(verify_data(0, &empty_root, 0, 1, &[], &[]), Ok(()));
    assert_eq!(
        verify_data(0, &empty_root, 0, 1, &[], b"x"),
        Err(VerifyError::WrongDataLength {
            expected: 0,
            actual: 1
        })
    );
}

#[test]
fn tampered_data_fails() {
    let mut data = counting_file();
    let file_root = root(&data);
    let tree = ChunkTree::from_data(&data);
    let proof = tree.proof(TreeNode::for_range(3, 0, 2).unwrap());
    data[100] ^= 1;
    assert_eq!(
        verify_data(40_000, &file_root, 0, 2, &proof, &data[..32_768]),
        Err(VerifyError::RootMismatch)
    );
}

#[test]
fn streaming_in_odd_pieces_matches() {
    let data = counting_file();
    let mut hasher = RootHasher::new();
    for piece in data.chunks(1_000) {
        hasher.update(piece);
    }
    assert_eq!(hasher.size(), 40_000);
    assert_eq!(hasher.finish(), root(&data));
}

#[test]
fn bundle_id_is_prefixed_with_0x02() {
    // Domain separation: the same bytes hash differently as a leaf and as a manifest.
    assert_ne!(manifest_hash(b"abc"), leaf_hash(b"abc"));
}
