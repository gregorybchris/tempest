//! Property tests: encodings round-trip, decoding never panics, and every proof verifies.

use proptest::prelude::*;
use tempest_protocol::frame::{encode_frame, split_frame};
use tempest_protocol::message::split_message_frame;
use tempest_protocol::tree::{
    CHUNK_SIZE, ChunkTree, RootHasher, TreeNode, chunk_count, leaf_hashes, root, tree_width,
    verify_data, verify_data_leaves,
};
use tempest_protocol::{Address, Decode, DecodeError, Encode, Hash, RangeList, Reader, Writer};

fn any_hash() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::new)
}

fn any_address() -> impl Strategy<Value = Address> {
    any::<std::net::SocketAddr>().prop_map(Address::from)
}

/// File sizes spanning up to 9 chunks, biased toward chunk boundaries.
fn file_size() -> impl Strategy<Value = usize> {
    prop_oneof![
        0..9 * CHUNK_SIZE,
        (0..9usize, -1..=1isize).prop_map(|(k, d)| (k * CHUNK_SIZE).saturating_add_signed(d)),
    ]
}

fn file(size: usize, seed: u8) -> Vec<u8> {
    (0..size)
        .map(|i| (i as u8).wrapping_mul(31) ^ seed)
        .collect()
}

proptest! {
    #[test]
    fn primitives_round_trip(
        a in any::<u8>(), b in any::<u16>(), c in any::<u32>(), d in any::<u64>(),
        e in any::<i64>(), f in any::<bool>(), h in any_hash(),
        bytes in proptest::collection::vec(any::<u8>(), 0..64), s in ".*",
    ) {
        let mut w = Writer::new();
        w.u8(a); w.u16(b); w.u32(c); w.u64(d); w.i64(e); w.bool(f); w.hash(&h);
        w.bytes(&bytes); w.string(&s);
        let encoded = w.into_bytes();
        let mut r = Reader::new(&encoded);
        prop_assert_eq!(r.u8()?, a);
        prop_assert_eq!(r.u16()?, b);
        prop_assert_eq!(r.u32()?, c);
        prop_assert_eq!(r.u64()?, d);
        prop_assert_eq!(r.i64()?, e);
        prop_assert_eq!(r.bool()?, f);
        prop_assert_eq!(r.hash()?, h);
        prop_assert_eq!(r.bytes()?, &bytes[..]);
        prop_assert_eq!(r.string()?, &s[..]);
        prop_assert_eq!(r.finish(), Ok(()));
    }

    #[test]
    fn addresses_round_trip(addr in any_address()) {
        prop_assert_eq!(Address::from_bytes(&addr.to_bytes()), Ok(addr));
    }

    #[test]
    fn range_lists_round_trip(chunks in proptest::collection::vec(0..500u32, 0..100)) {
        let list = RangeList::from_chunks(chunks.iter().copied());
        prop_assert_eq!(RangeList::from_bytes(&list.to_bytes()), Ok(list.clone()));
        let mut expected = chunks.clone();
        expected.sort_unstable();
        expected.dedup();
        prop_assert_eq!(list.chunks().collect::<Vec<_>>(), expected.clone());
        for i in 0..500 {
            prop_assert_eq!(list.contains(i), expected.binary_search(&i).is_ok());
        }
    }

    #[test]
    fn decoding_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        let _ = Address::from_bytes(&bytes);
        let _ = RangeList::from_bytes(&bytes);
        let _ = Reader::new(&bytes).list::<Hash>();
        let _ = split_message_frame(&bytes);
    }

    #[test]
    fn decoded_range_lists_reencode_identically(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
        if let Ok(list) = RangeList::from_bytes(&bytes) {
            prop_assert_eq!(list.to_bytes(), bytes);
        }
    }

    #[test]
    fn frames_round_trip(code in any::<u8>(), body in proptest::collection::vec(any::<u8>(), 0..64)) {
        let bytes = encode_frame(code, &body);
        let limit = |_| Ok::<u32, DecodeError>(u32::MAX);
        let (frame, used) = split_frame(&bytes, limit)?.unwrap();
        prop_assert_eq!(frame.message_type, code);
        prop_assert_eq!(frame.body, &body[..]);
        prop_assert_eq!(used, bytes.len());
    }

    #[test]
    fn streaming_root_matches_tree_root(size in file_size(), seed in any::<u8>(), split in 1..40_000usize) {
        let data = file(size, seed);
        let mut hasher = RootHasher::new();
        for piece in data.chunks(split) {
            hasher.update(piece);
        }
        prop_assert_eq!(hasher.finish(), ChunkTree::from_data(&data).root());
    }

    #[test]
    fn every_valid_range_verifies(size in file_size(), seed in any::<u8>()) {
        let data = file(size, seed);
        let file_root = root(&data);
        let tree = ChunkTree::from_data(&data);
        let chunks = chunk_count(size as u64);
        let width = tree_width(chunks) as u32;
        let mut count = 1;
        while count <= width {
            let mut position = 0;
            while u64::from(position) < chunks {
                let node = TreeNode::for_range(chunks, position, count).unwrap();
                let proof = tree.proof(node);
                let start = position as usize * CHUNK_SIZE;
                let end = (start + count as usize * CHUNK_SIZE).min(size);
                let slice = &data[start..end];
                prop_assert_eq!(
                    verify_data_leaves(size as u64, &file_root, position, count, &proof, slice),
                    Ok(leaf_hashes(slice))
                );
                position += count;
            }
            count *= 2;
        }
    }

    #[test]
    fn corrupted_proofs_fail(size in 1..9 * CHUNK_SIZE, seed in any::<u8>(), flip in any::<usize>()) {
        let data = file(size, seed);
        let file_root = root(&data);
        let tree = ChunkTree::from_data(&data);
        let chunks = chunk_count(size as u64);
        let node = TreeNode::for_range(chunks, 0, 1).unwrap();
        let mut proof = tree.proof(node);
        prop_assume!(!proof.is_empty());
        let i = flip % proof.len();
        let mut bytes = *proof[i].as_bytes();
        bytes[flip % 32] ^= 1;
        proof[i] = Hash::new(bytes);
        let end = CHUNK_SIZE.min(size);
        prop_assert!(verify_data(size as u64, &file_root, 0, 1, &proof, &data[..end]).is_err());
    }

    #[test]
    fn verify_data_never_panics(
        size in any::<u64>(), position in any::<u32>(), count in any::<u32>(),
        proof in proptest::collection::vec(any_hash(), 0..40),
        chunks in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let _ = verify_data(size, &Hash::ZERO, position, count, &proof, &chunks);
    }
}

#[test]
fn hash_list_encoding_has_count_prefix() {
    let hashes = vec![Hash::ZERO, Hash::ZERO];
    let mut w = Writer::new();
    w.list(&hashes);
    let bytes = w.into_bytes();
    assert_eq!(bytes.len(), 4 + 64);
    assert_eq!(Reader::new(&bytes).list::<Hash>(), Ok(hashes));
}
