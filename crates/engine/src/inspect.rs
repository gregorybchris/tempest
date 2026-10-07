//! What the UIs show inside one transfer: its files, its peers and a map of its chunks.

use serde::{Deserialize, Serialize};
use tempest_core::io::{Direction, PathKind};
use tempest_core::{ChunkCell, ChunkState, inspect};
use tempest_net::PeerDetail;
use tempest_protocol::{ChunkLayout, Manifest, RangeList};

/// The most cells a chunk map has, whatever is asked.
pub const MAX_CELLS: u32 = 65_536;

/// One file of a transfer's bundle, and how much of it is stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileInfo {
    /// Its path within the bundle, with `/` between segments.
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its chunks.
    pub chunks: u32,
    /// Its chunks stored.
    pub have: u32,
    /// The share of its chunks stored, from 0 to 1.
    pub progress: f64,
}

/// One connection of a running transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// The other peer's ID.
    pub peer_id: String,
    /// The address the connection goes to: the other peer's, or its relay's.
    pub address: String,
    /// `direct` or `relayed`.
    pub path: String,
    /// `dialed` (by this peer) or `accepted`.
    pub direction: String,
    /// Whether the opening sequence is complete.
    pub open: bool,
    /// The share of the bundle the other peer has, once known.
    pub progress: Option<f64>,
    /// This peer wants chunks the other has (Want).
    pub we_want: bool,
    /// The other peer wants chunks this one has.
    pub they_want: bool,
    /// The other peer is serving this one (Serving).
    pub serving_us: bool,
    /// This peer is serving the other.
    pub we_serve: bool,
    /// The other peer sent no data for too long while requests were open.
    pub snubbed: bool,
    /// Bytes per second received from it, recently.
    pub download_rate: u64,
    /// Bytes per second sent to it, recently.
    pub upload_rate: u64,
    /// Bytes received from it on this connection.
    pub downloaded: u64,
    /// Bytes sent to it on this connection.
    pub uploaded: u64,
    /// Its round-trip time in milliseconds, once measured.
    pub rtt_ms: Option<u64>,
}

/// A transfer's chunks, scaled to a number of cells.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkMapInfo {
    /// The bundle's chunk count; 0 before the manifest is known.
    pub chunks: u32,
    /// Whether it is running, so that chunks can be in flight and availability counts its
    /// connected peers. While it isn't, every availability is 0.
    pub running: bool,
    /// Runs of consecutive chunks in order, their sizes differing by at most one.
    pub cells: Vec<CellInfo>,
}

/// One cell of a chunk map, serialized as `[have, in_flight, missing, availability]` to keep
/// large maps small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "[u32; 4]", into = "[u32; 4]")]
pub struct CellInfo {
    /// Chunks stored.
    pub have: u32,
    /// Chunks requested and not yet stored.
    pub in_flight: u32,
    /// Chunks neither stored nor requested.
    pub missing: u32,
    /// The fewest connected peers holding any one of its chunks.
    pub availability: u32,
}

impl CellInfo {
    /// How many chunks it covers.
    pub fn chunks(&self) -> u32 {
        self.have + self.in_flight + self.missing
    }
}

impl From<[u32; 4]> for CellInfo {
    fn from([have, in_flight, missing, availability]: [u32; 4]) -> Self {
        Self {
            have,
            in_flight,
            missing,
            availability,
        }
    }
}

impl From<CellInfo> for [u32; 4] {
    fn from(cell: CellInfo) -> Self {
        [cell.have, cell.in_flight, cell.missing, cell.availability]
    }
}

impl From<ChunkCell> for CellInfo {
    fn from(cell: ChunkCell) -> Self {
        Self {
            have: cell.have,
            in_flight: cell.in_flight,
            missing: cell.missing,
            availability: cell.availability,
        }
    }
}

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
            .map(CellInfo::from)
            .collect(),
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
    fn cells_serialize_as_arrays() {
        let cell = CellInfo {
            have: 1,
            in_flight: 2,
            missing: 3,
            availability: 4,
        };
        assert_eq!(serde_json::to_string(&cell).unwrap(), "[1,2,3,4]");
        assert_eq!(serde_json::from_str::<CellInfo>("[1,2,3,4]").unwrap(), cell);
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
