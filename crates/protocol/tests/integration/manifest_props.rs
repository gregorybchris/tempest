//! Property tests for manifests, chunk layouts and links.

use std::collections::HashSet;

use proptest::prelude::*;
use tempest_protocol::hash::{leaf_hash, manifest_hash};
use tempest_protocol::path::{ancestors, fold_case};
use tempest_protocol::tree::{TreeNode, chunk_count};
use tempest_protocol::{
    Address, ChunkLayout, Decode, Encode, Entry, EntryKind, FileInfo, Hash, Link, Manifest, Writer,
};

/// Segments chosen to collide: case variants and non-ASCII text, plus some invalid ones.
fn segment() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        8 => prop::sample::select(vec![
            "a", "A", "b", "B", "c.txt", "C.TXT", "é", "É", "ß", "\u{1E9E}", "Σ", "ς", "x y",
        ]),
        1 => prop::sample::select(vec!["con", "a.", "e\u{301}", ""]),
    ]
}

fn path() -> impl Strategy<Value = String> {
    prop::collection::vec(segment(), 1..=3).prop_map(|segments| segments.join("/"))
}

fn target() -> impl Strategy<Value = String> {
    prop::collection::vec(prop_oneof![Just(".."), segment()], 1..=3)
        .prop_map(|segments| segments.join("/"))
}

fn file_info() -> impl Strategy<Value = FileInfo> {
    (
        prop::sample::select(vec![0u64, 1, 16_384, 40_000]),
        any::<bool>(),
        any::<bool>(),
        any::<i64>(),
    )
        .prop_map(|(size, right_root, executable, modified)| FileInfo {
            size,
            root: if size == 0 && right_root {
                leaf_hash(&[])
            } else {
                Hash::new([size as u8; 32])
            },
            executable,
            modified,
        })
}

fn entry() -> impl Strategy<Value = Entry> {
    prop_oneof![
        3 => (path(), file_info()).prop_map(|(path, info)| Entry::file(path, info)),
        1 => (path(), target()).prop_map(|(path, target)| Entry::symlink(path, target)),
        1 => path().prop_map(Entry::directory),
    ]
}

fn entries() -> impl Strategy<Value = Vec<Entry>> {
    prop::collection::vec(entry(), 0..8)
}

/// Encodes a manifest with the given fields without checking any rule.
fn encode_unchecked(created: i64, entries: &[Entry]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(1);
    w.string("bundle");
    w.string("");
    w.i64(created);
    w.string("tempest");
    w.list(entries);
    w.into_bytes()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Creating and decoding apply the same rules, and valid manifests round-trip.
    #[test]
    fn creation_and_decoding_agree(created in any::<i64>(), mut entries in entries()) {
        let created_manifest = Manifest::new("bundle", "", created, "tempest", entries.clone());
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let bytes = encode_unchecked(created, &entries);
        let decoded = Manifest::from_bytes(&bytes);
        match created_manifest {
            Ok(manifest) => {
                prop_assert_eq!(manifest.to_bytes(), bytes.clone());
                prop_assert_eq!(manifest.encoded_len(), bytes.len());
                prop_assert_eq!(manifest.bundle_id(), manifest_hash(&bytes));
                prop_assert_eq!(decoded, Ok(manifest));
            }
            Err(_) => prop_assert!(decoded.is_err()),
        }
    }

    /// Every valid manifest satisfies the structural rules.
    #[test]
    fn valid_manifests_keep_the_rules(entries in entries()) {
        let Ok(manifest) = Manifest::new("bundle", "", 0, "tempest", entries) else {
            return Ok(());
        };
        let mut folded = HashSet::new();
        let mut originals = std::collections::HashMap::new();
        for entry in manifest.entries() {
            prop_assert!(folded.insert(fold_case(&entry.path)));
            for path in ancestors(&entry.path).chain([entry.path.as_str()]) {
                let original = originals.entry(fold_case(path)).or_insert(path);
                prop_assert_eq!(*original, path, "paths differ only by case");
            }
        }
        for entry in manifest.entries() {
            for dir in ancestors(&entry.path) {
                let container = manifest.entries().iter().find(|e| e.path == dir);
                prop_assert!(container.is_none(), "{} is inside entry {}", entry.path, dir);
            }
            if let EntryKind::File(file) = &entry.kind
                && file.size == 0
            {
                prop_assert_eq!(file.root, leaf_hash(&[]));
            }
        }
        prop_assert!(manifest.entries().windows(2).all(|w| w[0].path < w[1].path));
    }

    /// The order entries are given in doesn't matter.
    #[test]
    fn entry_order_does_not_matter(entries in entries(), seed in any::<u64>()) {
        let Ok(manifest) = Manifest::new("bundle", "", 0, "tempest", entries.clone()) else {
            return Ok(());
        };
        let mut shuffled = entries;
        let len = shuffled.len();
        for i in 0..len {
            shuffled.swap(i, (seed as usize).wrapping_mul(i + 7) % len);
        }
        let reordered = Manifest::new("bundle", "", 0, "tempest", shuffled).unwrap();
        prop_assert_eq!(reordered.bundle_id(), manifest.bundle_id());
    }

    /// Whatever decodes from a corrupted manifest re-encodes to the same bytes.
    #[test]
    fn corrupted_manifests_never_decode_differently(
        entries in entries(),
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..4),
    ) {
        let Ok(manifest) = Manifest::new("bundle", "", 0, "tempest", entries) else {
            return Ok(());
        };
        let mut bytes = manifest.to_bytes();
        for (index, value) in flips {
            let i = index.index(bytes.len());
            bytes[i] = value;
        }
        if let Ok(decoded) = Manifest::from_bytes(&bytes) {
            prop_assert_eq!(decoded.to_bytes(), bytes);
        }
    }

    /// Every chunk index maps to a position in exactly one file, and Requests name the same tree
    /// node as the file's own range check.
    #[test]
    fn chunk_layout_is_consistent(
        sizes in prop::collection::vec(0u64..200_000, 1..6),
        start in 0u32..64,
        count_log in 0u32..5,
    ) {
        let entries = sizes.iter().enumerate().map(|(i, &size)| {
            let root = if size == 0 { leaf_hash(&[]) } else { Hash::ZERO };
            Entry::file(
                format!("f{i}"),
                FileInfo { size, root, executable: false, modified: 0 },
            )
        });
        let manifest = Manifest::new("bundle", "", 0, "tempest", entries.collect()).unwrap();
        let layout = ChunkLayout::new(&manifest);
        let total: u64 = sizes.iter().map(|&size| chunk_count(size)).sum();
        prop_assert_eq!(u64::from(layout.chunk_count()), total);

        let mut index = 0;
        for (file_index, file) in layout.files().iter().enumerate() {
            prop_assert_eq!(file.first_chunk, index);
            prop_assert_eq!(u64::from(file.chunk_count), chunk_count(sizes[file_index]));
            for position in 0..file.chunk_count {
                let location = layout.locate(index + position).unwrap();
                prop_assert_eq!((location.file, location.position), (file_index, position));
            }
            index = file.end();
        }
        prop_assert!(layout.locate(index).is_none());

        let count = 1 << count_log;
        match layout.locate(start) {
            None => prop_assert!(layout.request_node(start, count).is_err()),
            Some(location) => {
                let file = layout.files()[location.file];
                let expected = TreeNode::for_range(file.chunk_count.into(), location.position, count)
                    .map(|node| (location.file, node));
                prop_assert_eq!(layout.request_node(start, count), expected);
            }
        }
    }
}

fn any_hash() -> impl Strategy<Value = Hash> {
    any::<[u8; 32]>().prop_map(Hash::new)
}

fn any_link() -> impl Strategy<Value = Link> {
    (
        any_hash(),
        prop::option::of((
            any_hash(),
            prop::collection::vec(any::<std::net::SocketAddr>().prop_map(Address::from), 0..3),
            prop::option::of(any::<std::net::SocketAddr>().prop_map(Address::from)),
        )),
        prop::collection::vec(".+", 0..3),
        prop::option::of(".*"),
    )
        .prop_map(|(bundle, peer, trackers, name)| {
            let mut link = Link::new(bundle);
            if let Some((peer, addrs, relay)) = peer {
                link.peer = Some(peer);
                link.addrs = addrs;
                link.relay = relay;
            }
            link.trackers = trackers;
            link.name = name;
            link
        })
}

proptest! {
    #[test]
    fn links_round_trip(link in any_link()) {
        let text = link.to_string();
        prop_assert_eq!(text.parse::<Link>(), Ok(link));
        prop_assert!(text.is_ascii());
    }

    #[test]
    fn link_parsing_never_panics(text in "tempest:\\?.*") {
        if let Ok(link) = text.parse::<Link>() {
            prop_assert_eq!(link.to_string().parse::<Link>(), Ok(link));
        }
    }
}
