//! Made-up live data for the gallery: a few transfers, one of them downloading into a
//! 1,536-cell chunk map, and its rates.

use std::collections::VecDeque;

use tempest_ui_kit::ChunkCell;

/// The chunk map's size, the live view the benchmark guards (specs/interfaces.md).
pub const CELLS: usize = 1536;

/// How long rate history is kept, in milliseconds.
const HISTORY_MS: u64 = 300_000;

/// A transfer in the gallery's table.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    /// Its name.
    pub name: &'static str,
    /// Its size in bytes.
    pub size: u64,
    /// How far along it is.
    pub progress: f64,
    /// Bytes per second down.
    pub down: f64,
    /// Bytes per second up.
    pub up: f64,
    /// Its status word.
    pub status: &'static str,
    /// Its tags.
    pub tags: &'static [&'static str],
}

/// One rate sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// When, in milliseconds.
    pub t: u64,
    /// Bytes per second down.
    pub down: f64,
    /// Bytes per second up.
    pub up: f64,
}

/// The gallery's world.
#[derive(Clone, Debug, PartialEq)]
pub struct Demo {
    rng: u64,
    /// Milliseconds since the start.
    pub now: u64,
    /// The transfers.
    pub transfers: Vec<Transfer>,
    /// The first transfer's chunk map.
    pub cells: Vec<ChunkCell>,
    /// The first transfer's rates, oldest first.
    pub history: VecDeque<Sample>,
}

impl Demo {
    /// A world that has already run for a while, so charts have something to show.
    pub fn new(seed: u64) -> Self {
        let mut demo = Self {
            rng: seed | 1,
            now: 0,
            transfers: vec![
                transfer(
                    "ensemble-2026.tar",
                    4_200_000_000,
                    0.42,
                    "downloading",
                    &["audio", "archive"],
                ),
                transfer("field-recordings/", 812_000_000, 1.0, "sharing", &["audio"]),
                transfer(
                    "corpus-v3.bundle",
                    1_900_000_000,
                    0.31,
                    "stalled",
                    &["datasets"],
                ),
                transfer("satellite-tiles-2025/", 61_000_000_000, 0.0, "queued", &[]),
                transfer(
                    "lecture-notes.pdf",
                    18_400_000,
                    0.07,
                    "errored",
                    &["reading"],
                ),
                transfer(
                    "raw-footage-b/",
                    286_000_000_000,
                    0.66,
                    "stopped",
                    &["video"],
                ),
            ],
            cells: Vec::new(),
            history: VecDeque::new(),
        };
        demo.cells = (0..CELLS)
            .map(|i| {
                let availability = demo.below(9) as u32;
                if i < CELLS * 2 / 5 || demo.below(100) < 8 {
                    ChunkCell {
                        have: 28,
                        availability,
                        ..ChunkCell::default()
                    }
                } else {
                    ChunkCell {
                        missing: 28,
                        availability,
                        ..ChunkCell::default()
                    }
                }
            })
            .collect();
        for _ in 0..240 {
            demo.tick(250);
        }
        demo
    }

    /// Moves the world on by `dt` milliseconds.
    pub fn tick(&mut self, dt: u64) {
        self.now += dt;
        // Finish about a quarter of the downloads in flight, and start a few more.
        for i in 0..CELLS {
            if self.cells[i].in_flight > 0 && self.below(4) == 0 {
                let cell = &mut self.cells[i];
                cell.have += cell.in_flight;
                cell.in_flight = 0;
            }
        }
        for _ in 0..6 {
            let i = self.below(CELLS as u64) as usize;
            let cell = &mut self.cells[i];
            if cell.missing > 0 && cell.availability > 0 {
                cell.in_flight = cell.missing;
                cell.missing = 0;
            }
        }
        let have: u32 = self.cells.iter().map(|c| c.have).sum();
        let chunks: u32 = self.cells.iter().map(ChunkCell::chunks).sum();
        let phase = self.now as f64 / 47_000.0;
        let down = 9.5e6 + 3.2e6 * phase.sin() + self.below(1_400_000) as f64;
        let up = 1.2e6 + 0.6e6 * (phase * 1.7).cos() + self.below(300_000) as f64;
        let first = &mut self.transfers[0];
        first.progress = f64::from(have) / f64::from(chunks.max(1));
        first.down = down;
        first.up = up;
        self.transfers[1].up = 0.4e6 + self.below(200_000) as f64;
        if self.now.is_multiple_of(1000) {
            self.history.push_back(Sample {
                t: self.now,
                down,
                up,
            });
            while self
                .history
                .front()
                .is_some_and(|s| s.t + HISTORY_MS < self.now)
            {
                self.history.pop_front();
            }
        }
    }

    /// A number below `n`, from a xorshift generator.
    fn below(&mut self, n: u64) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng % n.max(1)
    }
}

fn transfer(
    name: &'static str,
    size: u64,
    progress: f64,
    status: &'static str,
    tags: &'static [&'static str],
) -> Transfer {
    Transfer {
        name,
        size,
        progress,
        down: 0.0,
        up: 0.0,
        status,
        tags,
    }
}
