//! What a session shows a UI: its connections, and its chunks scaled to a map.

use std::time::Duration;

use tempest_protocol::PeerId;

use crate::io::{ConnId, Direction, PathKind};

/// One connection, as a UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerStatus {
    /// The driver's name for the connection.
    pub conn: ConnId,
    /// The other peer.
    pub peer: PeerId,
    /// Which side dialed.
    pub direction: Direction,
    /// Whether it is direct or relayed.
    pub path: PathKind,
    /// Whether the opening sequence is complete.
    pub open: bool,
    /// The share of the bundle's chunks the other peer has, from 0 to 1, once known.
    pub progress: Option<f64>,
    /// This peer wants chunks the other has (Want).
    pub we_want: bool,
    /// The other peer wants chunks this one has.
    pub they_want: bool,
    /// The other peer is serving this one (Serving).
    pub serving_us: bool,
    /// This peer is serving the other.
    pub we_serve: bool,
    /// The other peer sent no Data for too long while requests were open.
    pub snubbed: bool,
    /// Bytes per second of verified Data received from it, recently.
    pub download_rate: f64,
    /// Bytes per second of Data sent to it, recently.
    pub upload_rate: f64,
    /// Bytes of verified Data received from it.
    pub downloaded: u64,
    /// Bytes of Data sent to it.
    pub uploaded: u64,
    /// Its round-trip time, once measured.
    pub rtt: Option<Duration>,
}

/// What this peer knows of one chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkState {
    /// Stored.
    Have,
    /// Missing, and requested from a peer.
    InFlight,
    /// Missing, and not requested.
    Missing,
}

/// A run of consecutive chunks, as one cell of a chunk map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChunkCell {
    /// Chunks stored.
    pub have: u32,
    /// Chunks requested and not yet stored.
    pub in_flight: u32,
    /// Chunks neither stored nor requested.
    pub missing: u32,
    /// The fewest connected peers holding any one of its chunks: how rare its rarest chunk is.
    pub availability: u32,
}

impl ChunkCell {
    /// How many chunks it covers.
    pub fn chunks(&self) -> u32 {
        self.have + self.in_flight + self.missing
    }
}

/// The chunks `0..chunks` in at most `cells` cells of consecutive chunks, in order. Cells cover
/// as near equal runs as can be: their sizes differ by at most one. `chunk` gives each chunk's
/// state and how many connected peers hold it.
pub fn chunk_map(
    chunks: u32,
    cells: u32,
    chunk: impl Fn(u32) -> (ChunkState, u32),
) -> Vec<ChunkCell> {
    let cells = cells.min(chunks);
    (0..cells)
        .map(|cell| {
            let bound = |cell: u32| (u64::from(cell) * u64::from(chunks) / u64::from(cells)) as u32;
            let mut summary = ChunkCell {
                availability: u32::MAX,
                ..ChunkCell::default()
            };
            for index in bound(cell)..bound(cell + 1) {
                let (state, holders) = chunk(index);
                match state {
                    ChunkState::Have => summary.have += 1,
                    ChunkState::InFlight => summary.in_flight += 1,
                    ChunkState::Missing => summary.missing += 1,
                }
                summary.availability = summary.availability.min(holders);
            }
            summary
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn cells_summarize_runs_of_chunks() {
        let state = |index: u32| match index % 3 {
            0 => (ChunkState::Have, index),
            1 => (ChunkState::InFlight, 5),
            _ => (ChunkState::Missing, 2),
        };
        let cells = chunk_map(6, 2, state);
        assert_eq!(
            cells,
            [
                ChunkCell {
                    have: 1,
                    in_flight: 1,
                    missing: 1,
                    availability: 0
                },
                ChunkCell {
                    have: 1,
                    in_flight: 1,
                    missing: 1,
                    availability: 2
                },
            ]
        );
        // Never more cells than chunks.
        assert_eq!(chunk_map(3, 10, state).len(), 3);
        assert!(chunk_map(0, 10, state).is_empty());
        assert!(chunk_map(10, 0, state).is_empty());
    }

    proptest! {
        #[test]
        fn cells_cover_every_chunk_once_in_near_equal_runs(chunks in 0u32..5000, cells in 0u32..700) {
            let map = chunk_map(chunks, cells, |index| (ChunkState::Missing, index));
            prop_assert_eq!(map.len() as u32, cells.min(chunks));
            prop_assert_eq!(map.iter().map(ChunkCell::chunks).sum::<u32>(), if cells == 0 { 0 } else { chunks });
            let sizes: Vec<u32> = map.iter().map(ChunkCell::chunks).collect();
            if let (Some(min), Some(max)) = (sizes.iter().min(), sizes.iter().max()) {
                prop_assert!(*min >= 1 && max - min <= 1);
            }
            // Holders are the chunk's index here, so each cell's rarest is its first chunk.
            let mut first = 0;
            for cell in &map {
                prop_assert_eq!(cell.availability, first);
                first += cell.chunks();
            }
        }
    }
}
