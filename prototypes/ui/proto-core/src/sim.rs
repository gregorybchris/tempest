//! A deterministic simulation of one Tempest client's state.
//!
//! The numbers are invented but behave plausibly: chunks fill rarest-first,
//! rates wander, the upload strategy re-evaluates every 10 s and rotates a
//! newcomer slot every 30 s, and every decision is logged with a reason.

use std::collections::VecDeque;

use serde::Serialize;

use crate::format;

/// Columns in each bundle's chunk map.
pub const GRID_COLS: usize = 96;
/// Rows in each bundle's chunk map.
pub const GRID_ROWS: usize = 16;
const CELLS: usize = GRID_COLS * GRID_ROWS;
const CHUNK: u64 = 16_384;
const HISTORY_LEN: usize = 600;
const DECISIONS_LEN: usize = 200;
const SAMPLE_EVERY_MS: u64 = 1_000;

/// The whole client: every bundle it is handling.
#[derive(Clone, Debug, Serialize)]
pub struct World {
    pub t_ms: u64,
    pub bundles: Vec<Bundle>,
    #[serde(skip)]
    rng: Rng,
    #[serde(skip)]
    next_sample_ms: u64,
    /// While pre-filling history, rates and decisions run but chunks don't fill.
    #[serde(skip)]
    warming: bool,
}

/// One bundle and its swarm.
#[derive(Clone, Debug, Serialize)]
pub struct Bundle {
    /// 64 lowercase hex characters.
    pub id: String,
    pub name: String,
    pub size_bytes: u64,
    pub files: u32,
    pub state: BundleState,
    /// 0..=1.
    pub progress: f64,
    pub down_bps: f64,
    pub up_bps: f64,
    pub downloaded_bytes: u64,
    pub uploaded_bytes: u64,
    /// `GRID_COLS * GRID_ROWS` cells, row-major. Each cell stands for a run
    /// of consecutive chunks.
    pub cells: Vec<Cell>,
    /// One sample per second, oldest first, at most 600.
    pub history: VecDeque<Sample>,
    pub peers: Vec<Peer>,
    /// Newest first, at most 200.
    pub decisions: VecDeque<Decision>,
    #[serde(skip)]
    target_down_bps: f64,
    #[serde(skip)]
    target_up_bps: f64,
    #[serde(skip)]
    next_request_ms: u64,
    #[serde(skip)]
    next_rechoke_ms: u64,
    #[serde(skip)]
    next_rotate_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum BundleState {
    Downloading,
    /// Complete and sharing with others.
    Sharing,
    /// No peer online has any chunk we still need.
    WaitingForPeers,
}

/// One cell of a chunk map.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Cell {
    /// Fraction of this cell's chunks we have, 0..=1.
    pub have: f32,
    /// Whether any of this cell's chunks are being requested right now.
    pub in_flight: bool,
    /// How many connected peers have this cell's chunks (0..=12).
    pub availability: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Sample {
    pub t_ms: u64,
    pub down_bps: f64,
    pub up_bps: f64,
    pub peers: u32,
}

/// A connected peer.
#[derive(Clone, Debug, Serialize)]
pub struct Peer {
    /// 64 lowercase hex characters.
    pub id: String,
    pub addr: String,
    pub path: Path,
    pub client: String,
    /// The peer's progress on this bundle, 0..=1.
    pub progress: f64,
    /// The peer told us Want (yes): it wants chunks we have.
    pub wants_from_us: bool,
    /// We told the peer Serving (yes).
    pub we_serve: bool,
    /// The peer told us Serving (yes).
    pub serves_us: bool,
    /// Bytes per second we receive from this peer.
    pub down_bps: f64,
    /// Bytes per second we send to this peer.
    pub up_bps: f64,
    /// The rotating newcomer slot.
    pub newcomer_slot: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Path {
    Direct,
    Relayed,
}

/// An explained decision, as the client's decision log shows it.
#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub t_ms: u64,
    pub kind: DecisionKind,
    /// What happened, e.g. `requested chunks 41,216–41,231 from 7c21f3a0`.
    pub what: String,
    /// Why, e.g. `only 2 of 14 peers have them`.
    pub why: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DecisionKind {
    Request,
    Serve,
    StopServing,
    Verify,
    Endgame,
    Connect,
}

impl DecisionKind {
    /// A short lowercase label, e.g. `request`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Serve => "serve",
            Self::StopServing => "stop",
            Self::Verify => "verify",
            Self::Endgame => "endgame",
            Self::Connect => "connect",
        }
    }
}

impl BundleState {
    /// A plain-language status, e.g. `downloading`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Downloading => "downloading",
            Self::Sharing => "sharing",
            Self::WaitingForPeers => "waiting for peers",
        }
    }
}

impl Path {
    pub fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Relayed => "relayed",
        }
    }
}

impl Bundle {
    /// Total number of chunks in the bundle.
    pub fn chunk_count(&self) -> u64 {
        self.size_bytes.div_ceil(CHUNK)
    }

    /// Chunks represented by one chunk-map cell.
    pub fn chunks_per_cell(&self) -> u64 {
        self.chunk_count().div_ceil(CELLS as u64)
    }

    /// The chunk index range `[start, end)` of a cell.
    pub fn cell_chunks(&self, cell: usize) -> (u64, u64) {
        let per = self.chunks_per_cell();
        let start = cell as u64 * per;
        (start, (start + per).min(self.chunk_count()))
    }

    /// Seconds until complete at the current rate, if downloading.
    pub fn eta_secs(&self) -> Option<u64> {
        if self.state != BundleState::Downloading || self.down_bps < 1.0 {
            return None;
        }
        let remaining = self.size_bytes as f64 * (1.0 - self.progress);
        Some((remaining / self.down_bps) as u64)
    }

    /// Uploaded divided by downloaded.
    pub fn ratio(&self) -> f64 {
        if self.downloaded_bytes == 0 {
            0.0
        } else {
            self.uploaded_bytes as f64 / self.downloaded_bytes as f64
        }
    }

    /// A plain-language explanation of the bundle's state, for the UI.
    pub fn explanation(&self) -> String {
        match self.state {
            BundleState::Downloading => {
                let serving = self.peers.iter().filter(|p| p.serves_us).count();
                format!(
                    "Downloading from {serving} of {} connected peers.",
                    self.peers.len()
                )
            }
            BundleState::Sharing => {
                let wanting = self.peers.iter().filter(|p| p.wants_from_us).count();
                format!(
                    "Complete. Sharing with {wanting} peers who still need chunks (ratio {:.2}).",
                    self.ratio()
                )
            }
            BundleState::WaitingForPeers => {
                "No one online has the chunks this bundle still needs. Tempest will keep looking."
                    .to_string()
            }
        }
    }
}

impl World {
    /// Creates the simulated client. The same seed always produces the same run.
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let bundles = vec![
            Bundle::new(
                &mut rng,
                "ubuntu-26.04-desktop-amd64.iso",
                6_412_820_480,
                1,
                BundleState::Downloading,
                0.41,
                14,
                (14_200_000.0, 1_850_000.0),
            ),
            Bundle::new(
                &mut rng,
                "openwebtext-shard-0412",
                48_031_002_112,
                2_048,
                BundleState::Downloading,
                0.08,
                9,
                (6_400_000.0, 940_000.0),
            ),
            Bundle::new(
                &mut rng,
                "holiday-photos-2026",
                1_318_402_048,
                412,
                BundleState::Sharing,
                1.0,
                3,
                (0.0, 2_600_000.0),
            ),
            Bundle::new(
                &mut rng,
                "llama-4-scout-weights",
                218_993_510_400,
                55,
                BundleState::WaitingForPeers,
                0.0,
                0,
                (0.0, 0.0),
            ),
        ];
        let mut world = Self {
            t_ms: 0,
            bundles,
            rng,
            next_sample_ms: 0,
            warming: true,
        };
        // Pre-fill ten minutes of history so charts aren't empty.
        for _ in 0..HISTORY_LEN {
            world.tick(SAMPLE_EVERY_MS);
        }
        world.warming = false;
        for bundle in &mut world.bundles {
            for cell in &mut bundle.cells {
                cell.in_flight = false;
            }
        }
        for bundle in &mut world.bundles {
            bundle.decisions.truncate(40);
        }
        world
    }

    /// Advances the simulation by `dt_ms` milliseconds.
    pub fn tick(&mut self, dt_ms: u64) {
        self.t_ms += dt_ms;
        let t = self.t_ms;
        for bundle in &mut self.bundles {
            bundle.tick(&mut self.rng, t, dt_ms, self.warming);
        }
        if t >= self.next_sample_ms {
            self.next_sample_ms = t + SAMPLE_EVERY_MS;
            for bundle in &mut self.bundles {
                bundle.history.push_back(Sample {
                    t_ms: t,
                    down_bps: bundle.down_bps,
                    up_bps: bundle.up_bps,
                    peers: bundle.peers.len() as u32,
                });
                while bundle.history.len() > HISTORY_LEN {
                    bundle.history.pop_front();
                }
            }
        }
    }

    /// Total download rate across bundles.
    pub fn down_bps(&self) -> f64 {
        self.bundles.iter().map(|b| b.down_bps).sum()
    }

    /// Total upload rate across bundles.
    pub fn up_bps(&self) -> f64 {
        self.bundles.iter().map(|b| b.up_bps).sum()
    }

    /// Total connected peers across bundles.
    pub fn peer_count(&self) -> usize {
        self.bundles.iter().map(|b| b.peers.len()).sum()
    }
}

impl Bundle {
    #[allow(clippy::too_many_arguments)]
    fn new(
        rng: &mut Rng,
        name: &str,
        size_bytes: u64,
        files: u32,
        state: BundleState,
        progress: f64,
        peer_count: usize,
        (down, up): (f64, f64),
    ) -> Self {
        // Availability varies smoothly across the bundle, like real swarms
        // where neighboring chunks tend to spread together.
        let phase = rng.f64() * 6.0;
        let cells = (0..CELLS)
            .map(|i| {
                let x = i as f64 / CELLS as f64;
                let wave =
                    ((x * 9.0 + phase).sin() + (x * 23.0 + phase * 2.0).sin() * 0.5 + 1.5) / 3.0;
                let availability = if peer_count == 0 {
                    0
                } else {
                    ((wave * 11.0) as i64 + rng.range(-1, 2)).clamp(1, 12) as u8
                };
                let filled = state == BundleState::Sharing
                    || rng.f64() < progress * (0.6 + 0.8 * (1.0 - wave));
                let have = if filled { 1.0 } else { 0.0 };
                Cell {
                    have,
                    in_flight: false,
                    availability,
                }
            })
            .collect::<Vec<_>>();
        let peers = (0..peer_count).map(|i| Peer::new(rng, i, state)).collect();
        let mut bundle = Self {
            id: rng.hex64(),
            name: name.to_string(),
            size_bytes,
            files,
            state,
            progress,
            down_bps: down,
            up_bps: up,
            downloaded_bytes: (size_bytes as f64 * progress) as u64,
            uploaded_bytes: (size_bytes as f64 * progress * 0.31) as u64,
            cells,
            history: VecDeque::with_capacity(HISTORY_LEN),
            peers,
            decisions: VecDeque::with_capacity(DECISIONS_LEN),
            target_down_bps: down,
            target_up_bps: up,
            next_request_ms: 0,
            next_rechoke_ms: 0,
            next_rotate_ms: 0,
        };
        if state == BundleState::Sharing {
            bundle.uploaded_bytes = (size_bytes as f64 * 1.7) as u64;
            bundle.downloaded_bytes = size_bytes;
        }
        bundle.recompute_progress();
        bundle
    }

    fn tick(&mut self, rng: &mut Rng, t: u64, dt_ms: u64, warming: bool) {
        if self.state == BundleState::WaitingForPeers {
            self.down_bps = 0.0;
            self.up_bps = 0.0;
            return;
        }
        let dt = dt_ms as f64 / 1000.0;

        // Rates wander around a slowly drifting target.
        self.target_down_bps *= 1.0 + rng.normal() * 0.02;
        self.target_up_bps *= 1.0 + rng.normal() * 0.02;
        self.down_bps += (self.target_down_bps * (0.85 + rng.f64() * 0.3) - self.down_bps) * 0.3;
        self.up_bps += (self.target_up_bps * (0.8 + rng.f64() * 0.4) - self.up_bps) * 0.3;
        if self.state == BundleState::Sharing {
            self.down_bps = 0.0;
        }

        self.tick_peers(rng);
        if self.state == BundleState::Downloading && !warming {
            self.tick_chunks(rng, t, dt);
        } else if self.state == BundleState::Downloading && t >= self.next_request_ms {
            self.next_request_ms = t + 900 + (rng.f64() * 1400.0) as u64;
            self.log_request(rng, t);
        }
        self.uploaded_bytes += (self.up_bps * dt) as u64;
        if t >= self.next_rechoke_ms {
            self.next_rechoke_ms = t + 10_000;
            self.rechoke(rng, t);
        }
        if t >= self.next_rotate_ms {
            self.next_rotate_ms = t + 30_000;
            self.rotate_newcomer(rng, t);
        }
    }

    fn tick_peers(&mut self, rng: &mut Rng) {
        let serving_us: f64 = self.peers.iter().filter(|p| p.serves_us).count().max(1) as f64;
        let we_serve: f64 = self.peers.iter().filter(|p| p.we_serve).count().max(1) as f64;
        for peer in &mut self.peers {
            peer.down_bps = if peer.serves_us && self.state == BundleState::Downloading {
                self.down_bps / serving_us * (0.6 + rng.f64() * 0.8)
            } else {
                0.0
            };
            peer.up_bps = if peer.we_serve {
                self.up_bps / we_serve * (0.6 + rng.f64() * 0.8)
            } else {
                0.0
            };
            if peer.progress < 1.0 {
                peer.progress = (peer.progress + rng.f64() * 0.0008).min(1.0);
            }
        }
    }

    fn tick_chunks(&mut self, rng: &mut Rng, t: u64, dt: f64) {
        let gained = self.down_bps * dt;
        self.downloaded_bytes += gained as u64;
        let cell_bytes = self.size_bytes as f64 / CELLS as f64;

        // Keep up to six cells in flight, chosen rarest-first.
        let in_flight = self.cells.iter().filter(|c| c.in_flight).count();
        for _ in in_flight..6 {
            if let Some(i) = self.rarest_missing(rng) {
                self.cells[i].in_flight = true;
            }
        }

        // Spread the bytes over the in-flight cells.
        let flying: Vec<usize> = (0..CELLS).filter(|&i| self.cells[i].in_flight).collect();
        if flying.is_empty() {
            self.state = BundleState::Sharing;
            self.down_bps = 0.0;
            self.push_decision(
                t,
                DecisionKind::Verify,
                "all chunks verified".into(),
                "the bundle is complete; switching to round-robin sharing".into(),
            );
            return;
        }
        let per_cell = gained / flying.len() as f64 / cell_bytes;
        for &i in &flying {
            let cell = &mut self.cells[i];
            cell.have = (cell.have + per_cell as f32 * (0.5 + rng.f64() as f32)).min(1.0);
            if cell.have >= 1.0 {
                cell.in_flight = false;
                if rng.f64() < 0.35 {
                    let (start, end) = self.cell_chunks(i);
                    self.push_decision(
                        t,
                        DecisionKind::Verify,
                        format!(
                            "verified chunks {}–{}",
                            format::count(start),
                            format::count(end - 1)
                        ),
                        "every proof matched the file's root".into(),
                    );
                }
            }
        }
        self.recompute_progress();

        let missing = self.cells.iter().filter(|c| c.have < 1.0).count();
        if (1..=6).contains(&missing) && rng.f64() < 0.05 {
            self.push_decision(
                t,
                DecisionKind::Endgame,
                format!("asked a second peer for the last {missing} ranges"),
                "every missing chunk is already requested; duplicates beat waiting on a slow peer"
                    .into(),
            );
        }

        if t >= self.next_request_ms {
            self.next_request_ms = t + 900 + (rng.f64() * 1400.0) as u64;
            self.log_request(rng, t);
        }
    }

    fn rarest_missing(&self, rng: &mut Rng) -> Option<usize> {
        let mut best: Option<(u8, f64, usize)> = None;
        for (i, cell) in self.cells.iter().enumerate() {
            if cell.have >= 1.0 || cell.in_flight || cell.availability == 0 {
                continue;
            }
            let key = (cell.availability, rng.f64(), i);
            if best.is_none_or(|b| (key.0, key.1) < (b.0, b.1)) {
                best = Some(key);
            }
        }
        best.map(|b| b.2)
    }

    fn log_request(&mut self, rng: &mut Rng, t: u64) {
        let flying: Vec<usize> = (0..CELLS).filter(|&i| self.cells[i].in_flight).collect();
        let serving: Vec<usize> = (0..self.peers.len())
            .filter(|&i| self.peers[i].serves_us)
            .collect();
        if flying.is_empty() || serving.is_empty() {
            return;
        }
        let cell = flying[rng.below(flying.len())];
        let peer = &self.peers[serving[rng.below(serving.len())]];
        let (start, _) = self.cell_chunks(cell);
        let offset = rng.below(self.chunks_per_cell().max(16) as usize / 16) as u64 * 16;
        let first = start + offset;
        let availability = self.cells[cell].availability;
        let what = format!(
            "requested chunks {}–{} from {}",
            format::count(first),
            format::count(first + 15),
            format::short_id(&peer.id)
        );
        let why = format!(
            "rarest first: only {availability} of {} peers have them",
            self.peers.len()
        );
        self.push_decision(t, DecisionKind::Request, what, why);
    }

    /// Every 10 s: serve the 4 peers that uploaded to us fastest (or, when
    /// complete, rotate round-robin).
    fn rechoke(&mut self, rng: &mut Rng, t: u64) {
        if self.peers.is_empty() {
            return;
        }
        let mut order: Vec<usize> = (0..self.peers.len())
            .filter(|&i| self.peers[i].wants_from_us)
            .collect();
        if self.state == BundleState::Sharing {
            let shift = rng.below(order.len().max(1));
            order.rotate_left(shift);
        } else {
            order.sort_by(|&a, &b| self.peers[b].down_bps.total_cmp(&self.peers[a].down_bps));
        }
        let chosen: Vec<usize> = order.into_iter().take(4).collect();
        for i in 0..self.peers.len() {
            let should = chosen.contains(&i) || self.peers[i].newcomer_slot;
            if should != self.peers[i].we_serve {
                self.peers[i].we_serve = should;
                let id = format::short_id(&self.peers[i].id).to_string();
                if should {
                    let why = if self.state == BundleState::Sharing {
                        "its turn in round-robin sharing".to_string()
                    } else {
                        format!(
                            "one of the 4 fastest uploaders to us ({})",
                            format::rate(self.peers[i].down_bps)
                        )
                    };
                    self.push_decision(t, DecisionKind::Serve, format!("serving {id}"), why);
                } else {
                    self.push_decision(
                        t,
                        DecisionKind::StopServing,
                        format!("stopped serving {id}"),
                        "no longer among the 4 fastest uploaders to us".into(),
                    );
                }
            }
        }
    }

    /// Every 30 s: give the newcomer slot to a random interested peer.
    fn rotate_newcomer(&mut self, rng: &mut Rng, t: u64) {
        let candidates: Vec<usize> = (0..self.peers.len())
            .filter(|&i| self.peers[i].wants_from_us && !self.peers[i].we_serve)
            .collect();
        if candidates.is_empty() {
            return;
        }
        for peer in &mut self.peers {
            peer.newcomer_slot = false;
        }
        let i = candidates[rng.below(candidates.len())];
        self.peers[i].newcomer_slot = true;
        self.peers[i].we_serve = true;
        let id = format::short_id(&self.peers[i].id).to_string();
        self.push_decision(
            t,
            DecisionKind::Serve,
            format!("serving {id} in the newcomer slot"),
            "rotating slot, so peers with nothing to trade yet can get started".into(),
        );
    }

    fn push_decision(&mut self, t_ms: u64, kind: DecisionKind, what: String, why: String) {
        self.decisions.push_front(Decision {
            t_ms,
            kind,
            what,
            why,
        });
        self.decisions.truncate(DECISIONS_LEN);
    }

    fn recompute_progress(&mut self) {
        self.progress = self.cells.iter().map(|c| c.have as f64).sum::<f64>() / CELLS as f64;
    }
}

impl Peer {
    fn new(rng: &mut Rng, i: usize, state: BundleState) -> Self {
        const CLIENTS: [&str; 4] = [
            "tempest/0.1.0",
            "tempest/0.1.0",
            "tempest/0.0.9",
            "gale/0.3.2",
        ];
        let relayed = rng.f64() < 0.2;
        let addr = if relayed {
            format!(
                "198.51.100.{}:{}",
                1 + rng.below(250),
                40_000 + rng.below(2000)
            )
        } else if rng.f64() < 0.3 {
            format!(
                "[2001:db8:{:x}::{:x}]:4433",
                rng.below(0xffff),
                rng.below(0xffff)
            )
        } else {
            format!("203.0.{}.{}:4433", rng.below(255), 1 + rng.below(250))
        };
        Self {
            id: rng.hex64(),
            addr,
            path: if relayed { Path::Relayed } else { Path::Direct },
            client: CLIENTS[rng.below(CLIENTS.len())].to_string(),
            progress: if state == BundleState::Sharing {
                rng.f64() * 0.7
            } else {
                rng.f64()
            },
            wants_from_us: rng.f64() < 0.8 || state == BundleState::Sharing,
            we_serve: false,
            serves_us: state == BundleState::Downloading && (i < 5 || rng.f64() < 0.3),
            down_bps: 0.0,
            up_bps: 0.0,
            newcomer_slot: false,
        }
    }
}

/// A small deterministic random number generator (xorshift64*).
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [0, n).
    fn below(&mut self, n: usize) -> usize {
        (self.f64() * n as f64) as usize
    }

    /// Uniform in [lo, hi).
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.f64() * (hi - lo) as f64) as i64
    }

    /// Roughly standard normal.
    fn normal(&mut self) -> f64 {
        (0..6).map(|_| self.f64()).sum::<f64>() - 3.0
    }

    fn hex64(&mut self) -> String {
        (0..4).map(|_| format!("{:016x}", self.next())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic() {
        let mut a = World::new(7);
        let mut b = World::new(7);
        for _ in 0..100 {
            a.tick(250);
            b.tick(250);
        }
        assert_eq!(a.bundles[0].progress, b.bundles[0].progress);
        assert_eq!(a.bundles[0].decisions.len(), b.bundles[0].decisions.len());
    }

    #[test]
    fn progresses() {
        let mut w = World::new(1);
        let before = w.bundles[0].progress;
        assert!(
            (before - 0.41).abs() < 0.08,
            "pre-fill must not advance progress: {before}"
        );
        for _ in 0..200 {
            w.tick(500);
        }
        assert!(w.bundles[0].progress > before);
        assert_eq!(w.bundles[0].history.len(), HISTORY_LEN);
        assert!(!w.bundles[0].decisions.is_empty());
    }
}
