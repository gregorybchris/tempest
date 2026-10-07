//! What the UIs show inside one transfer: its files, its peers and a map of its chunks
//! (D-193).

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
