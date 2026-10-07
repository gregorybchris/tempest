//! What the UIs show inside one transfer: its files, its peers and a map of its chunks.

pub use tempest_api::inspect::{CellInfo, ChunkMapInfo, FileInfo, PeerInfo};

use tempest_core::io::{Direction, PathKind};
use tempest_core::{ChunkCell, ChunkState, inspect};
use tempest_net::PeerDetail;
use tempest_protocol::{ChunkLayout, Manifest, RangeList};

/// The most cells a chunk map has, whatever is asked.
pub const MAX_CELLS: u32 = 65_536;

/// Every file of `manifest`, with how much of it `have` holds.
pub(crate) fn files(manifest: &Manifest, have: &RangeList) -> Vec<FileInfo> {
    let layout = ChunkLayout::new(manifest);
    let mut ranges = have.ranges().iter().peekable();
    layout
        .files()
        .iter()
        .map(|span| {
            let entry = &manifest.entries()[span.entry];
            let mut stored = 0;
            // Ranges are in order, so each file starts where the last left off; a range that
            // spans files is kept for the next.
            while let Some(range) = ranges.peek() {
                if range.start >= span.end() {
                    break;
                }
                stored += range.end.min(span.end()) - range.start.max(span.first_chunk);
                if range.end > span.end() {
                    break;
                }
                ranges.next();
            }
            FileInfo {
                path: entry.path.to_string(),
                size: entry.as_file().map_or(0, |file| file.size),
                chunks: span.chunk_count,
                have: stored,
                progress: f64::from(stored) / f64::from(span.chunk_count),
            }
        })
        .collect()
}

/// The chunk map of a bundle of `chunks` chunks that isn't running, holding `have`.
pub(crate) fn stored_chunk_map(chunks: u32, have: &RangeList, cells: u32) -> ChunkMapInfo {
    let state = |chunk| match have.contains(chunk) {
        true => (ChunkState::Have, 0),
        false => (ChunkState::Missing, 0),
    };
    ChunkMapInfo {
        chunks,
        running: false,
        cells: inspect::chunk_map(chunks, cells.min(MAX_CELLS), state)
            .into_iter()
            .map(cell)
            .collect(),
    }
}

/// A chunk map's cell as the API shows it.
pub(crate) fn cell(cell: ChunkCell) -> CellInfo {
    CellInfo {
        have: cell.have,
        in_flight: cell.in_flight,
        missing: cell.missing,
        availability: cell.availability,
    }
}

/// A connection as the API shows it.
pub(crate) fn peer(detail: PeerDetail) -> PeerInfo {
    let status = detail.status;
    PeerInfo {
        peer_id: status.peer.to_string(),
        address: detail.address.to_string(),
        path: match status.path {
            PathKind::Direct => "direct",
            PathKind::Relayed => "relayed",
        }
        .to_owned(),
        direction: match status.direction {
            Direction::Dialed => "dialed",
            Direction::Accepted => "accepted",
        }
        .to_owned(),
        open: status.open,
        progress: status.progress,
        we_want: status.we_want,
        they_want: status.they_want,
        serving_us: status.serving_us,
        we_serve: status.we_serve,
        snubbed: status.snubbed,
        download_rate: status.download_rate as u64,
        upload_rate: status.upload_rate as u64,
        downloaded: status.downloaded,
        uploaded: status.uploaded,
        rtt_ms: status.rtt.map(|rtt| rtt.as_millis() as u64),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempest_protocol::Hash;
    use tempest_protocol::hash::leaf_hash;
    use tempest_protocol::manifest::{Entry, FileInfo as Info};
    use tempest_protocol::tree::CHUNK_SIZE;

    use super::*;

    fn manifest(sizes: &[u64]) -> Manifest {
        let entries = sizes
            .iter()
            .enumerate()
            .map(|(n, size)| {
                let info = Info {
                    size: *size,
                    root: match size {
                        0 => leaf_hash(&[]),
                        _ => Hash::new([n as u8; 32]),
                    },
                    executable: false,
                    modified: 0,
                };
                Entry::file(format!("f{n}"), info)
            })
            .chain([Entry::directory("empty-folder")])
            .collect();
        Manifest::new("bundle", "", 0, "", entries).unwrap()
    }

    #[test]
    fn files_count_their_stored_chunks() {
        let chunk = CHUNK_SIZE as u64;
        // Chunks 0-2 are f0's, 3-4 f1's, 5 f2's (empty, so one chunk).
        let manifest = manifest(&[3 * chunk, 2 * chunk, 0]);
        let have = RangeList::from_chunks([1, 2, 3, 5]);
        let files = files(&manifest, &have);
        let summary: Vec<_> = files
            .iter()
            .map(|f| (f.path.as_str(), f.have, f.chunks))
            .collect();
        assert_eq!(summary, [("f0", 2, 3), ("f1", 1, 2), ("f2", 1, 1)]);
        assert_eq!(files[1].size, 2 * chunk);
        assert_eq!(files[1].progress, 0.5);
    }

    #[test]
    fn a_stored_map_has_no_chunks_in_flight() {
        let map = stored_chunk_map(10, &RangeList::from_chunks(0..5), 2);
        assert!(!map.running);
        assert_eq!(
            map.cells,
            [CellInfo::from([5, 0, 0, 0]), CellInfo::from([0, 0, 5, 0])]
        );
    }

    proptest! {
        #[test]
        fn files_add_up_to_the_chunks_stored(
            sizes in prop::collection::vec(0u64..200_000, 1..6),
            stored in prop::collection::vec(any::<bool>(), 0..80),
        ) {
            let manifest = manifest(&sizes);
            let count = ChunkLayout::new(&manifest).chunk_count();
            let have = RangeList::from_chunks(
                (0..count).filter(|chunk| stored.get(*chunk as usize).copied().unwrap_or(false)),
            );
            let files = files(&manifest, &have);
            prop_assert_eq!(files.iter().map(|f| u64::from(f.have)).sum::<u64>(), have.chunk_count());
            prop_assert!(files.iter().all(|f| f.have <= f.chunks));
        }
    }
}
