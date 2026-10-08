//! The headline metrics of runs (`specs/simulation.md`), and summaries of each variant across
//! its seeds.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use tempest_core::Time;

use crate::engine::{PeerOutcome, RunResult};
use crate::scenario::{Scenario, Start};

impl RunResult {
    /// For each peer that started without the bundle and reached `at`, the time from arriving
    /// to `at`, shortest first.
    fn times_since_arrival(&self, at: impl Fn(&PeerOutcome) -> Option<Time>) -> Vec<Duration> {
        let mut times: Vec<Duration> = self
            .peers
            .iter()
            .filter(|p| p.starts == Start::Empty)
            .filter_map(|p| Some(at(p)?.since(p.arrived?)))
            .collect();
        times.sort();
        times
    }

    /// How long each peer that started without the bundle and completed took, from arriving to
    /// completing, shortest first.
    pub fn completion_times(&self) -> Vec<Duration> {
        self.times_since_arrival(|p| p.completed)
    }

    /// How long each peer that started without the bundle took from arriving to receiving its
    /// first verified payload, shortest first.
    pub fn first_byte_times(&self) -> Vec<Duration> {
        self.times_since_arrival(|p| p.first_byte)
    }

    /// How long each peer that started without the bundle took from arriving to opening its
    /// first connection, shortest first.
    pub fn first_peer_times(&self) -> Vec<Duration> {
        self.times_since_arrival(|p| p.first_peer)
    }

    /// The fraction of the swarm's upload capacity that carried payload: payload bytes uploaded
    /// over the capacity of every peer with a limited upload, for as long as it was present.
    /// `None` if no such peer was present.
    pub fn upload_utilization(&self, scenario: &Scenario) -> Option<f64> {
        let (mut uploaded, mut capacity) = (0.0, 0.0);
        let classes = scenario.classes(self.variant);
        for peer in &self.peers {
            let rate = classes[peer.class].bandwidth.up.get();
            let Some(arrived) = peer.arrived else {
                continue;
            };
            if rate.is_finite() {
                let present = peer.left.unwrap_or(self.end).since(arrived);
                uploaded += peer.uploaded as f64;
                capacity += rate * present.as_secs_f64();
            }
        }
        (capacity > 0.0).then(|| uploaded / capacity)
    }

    /// Jain's fairness index of the upload/download ratios of the peers that downloaded
    /// anything: 1 when every peer gave back in the same proportion, approaching 1/n when one
    /// peer did all the giving. `None` if nobody downloaded, or nobody uploaded.
    pub fn fairness(&self) -> Option<f64> {
        let ratios: Vec<f64> = self
            .peers
            .iter()
            .filter(|p| p.downloaded > 0)
            .map(|p| p.uploaded as f64 / p.downloaded as f64)
            .collect();
        jain(&ratios)
    }

    /// The mean, over the run's samples, of how many present peers held the rarest chunk.
    /// `None` if the run took no samples.
    pub fn rarest_copies(&self) -> Option<f64> {
        let samples = &self.availability;
        (!samples.is_empty()).then(|| {
            samples.iter().map(|s| f64::from(s.rarest)).sum::<f64>() / samples.len() as f64
        })
    }

    /// Overhead bytes by message: for Data, its framing and proofs; for everything else, all of
    /// it.
    pub fn overhead_by_message(&self) -> BTreeMap<&'static str, u64> {
        self.traffic
            .iter()
            .map(|(&name, traffic)| {
                let payload = if name == "Data" { self.payload_sent } else { 0 };
                (name, traffic.bytes - payload)
            })
            .collect()
    }

    /// The run's headline metrics.
    pub fn metrics(&self, scenario: &Scenario) -> RunMetrics {
        let completion = self.completion_times();
        RunMetrics {
            downloaders: self.downloaders(),
            completed: completion.len(),
            completion_p50: percentile(&completion, 0.5),
            completion_p90: percentile(&completion, 0.9),
            completion_p99: percentile(&completion, 0.99),
            first_byte_p50: percentile(&self.first_byte_times(), 0.5),
            first_peer_p50: percentile(&self.first_peer_times(), 0.5),
            upload_utilization: self.upload_utilization(scenario),
            fairness: self.fairness(),
            rarest_copies: self.rarest_copies(),
            overhead: self.overhead(),
            duplicate: self.duplicate(),
        }
    }

    /// The number of peers that started without the bundle.
    pub fn downloaders(&self) -> usize {
        self.peers
            .iter()
            .filter(|p| p.starts == Start::Empty)
            .count()
    }

    /// Bytes sent that weren't chunk payload: every other message, and Data's framing and
    /// proofs.
    pub fn overhead(&self) -> u64 {
        let total: u64 = self.traffic.values().map(|t| t.bytes).sum();
        total - self.payload_sent
    }

    /// Payload bytes received more than once (endgame duplicates).
    pub fn duplicate(&self) -> u64 {
        self.peers.iter().map(|p| p.duplicate).sum()
    }
}

/// The headline metrics of one run (`specs/simulation.md`). Times are from each peer's arrival.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunMetrics {
    /// Peers that started without the bundle.
    pub downloaders: usize,
    /// Those that completed.
    pub completed: usize,
    /// The median completion time.
    pub completion_p50: Option<Duration>,
    /// The 90th percentile completion time.
    pub completion_p90: Option<Duration>,
    /// The 99th percentile completion time.
    pub completion_p99: Option<Duration>,
    /// The median time to the first verified payload.
    pub first_byte_p50: Option<Duration>,
    /// The median time to the first open connection.
    pub first_peer_p50: Option<Duration>,
    /// See [`RunResult::upload_utilization`].
    pub upload_utilization: Option<f64>,
    /// See [`RunResult::fairness`].
    pub fairness: Option<f64>,
    /// See [`RunResult::rarest_copies`].
    pub rarest_copies: Option<f64>,
    /// Bytes sent that weren't chunk payload.
    pub overhead: u64,
    /// Payload bytes received more than once (endgame duplicates).
    pub duplicate: u64,
}

/// Jain's fairness index of `values`: `(Σx)² / (n · Σx²)`. `None` if there are none, or all
/// are zero.
pub fn jain(values: &[f64]) -> Option<f64> {
    let sum: f64 = values.iter().sum();
    let squares: f64 = values.iter().map(|x| x * x).sum();
    (squares > 0.0).then(|| sum * sum / (values.len() as f64 * squares))
}

/// The value below which a fraction `p` of `sorted` lies, by the nearest-rank method.
pub fn percentile(sorted: &[Duration], p: f64) -> Option<Duration> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// One variant's results across its seeds.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantSummary {
    /// The variant's name.
    pub name: String,
    /// The number of runs.
    pub runs: usize,
    /// Peers that started without the bundle, over all runs.
    pub downloaders: usize,
    /// Those that completed.
    pub completed: usize,
    /// Completion times of every peer that completed in any run, shortest first.
    pub completion_times: Vec<Duration>,
    /// Times to first byte of every downloader in any run, shortest first.
    pub first_byte_times: Vec<Duration>,
    /// The mean upload utilization of the runs that have one.
    pub mean_utilization: Option<f64>,
    /// The mean fairness index of the runs that have one.
    pub mean_fairness: Option<f64>,
    /// The mean rarest-chunk availability of the runs that have one.
    pub mean_rarest: Option<f64>,
    /// The mean simulated length of a run.
    pub mean_end: Duration,
    /// The mean overhead bytes per run.
    pub mean_overhead: u64,
    /// The mean duplicate payload bytes per run.
    pub mean_duplicate: u64,
}

/// Summarizes `results` by variant, in variant order. Variants without results are left out.
pub fn summarize(scenario: &Scenario, results: &[RunResult]) -> Vec<VariantSummary> {
    scenario
        .variants()
        .iter()
        .filter_map(|variant| {
            let runs: Vec<&RunResult> = results
                .iter()
                .filter(|r| r.variant == variant.index)
                .collect();
            let n = runs.len();
            if n == 0 {
                return None;
            }
            let mut completion_times: Vec<Duration> =
                runs.iter().flat_map(|r| r.completion_times()).collect();
            completion_times.sort();
            let mut first_byte_times: Vec<Duration> =
                runs.iter().flat_map(|r| r.first_byte_times()).collect();
            first_byte_times.sort();
            let mean =
                |f: &dyn Fn(&RunResult) -> u64| runs.iter().map(|r| f(r)).sum::<u64>() / n as u64;
            let mean_of = |values: Vec<f64>| {
                (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
            };
            Some(VariantSummary {
                name: variant.name(),
                runs: n,
                downloaders: runs.iter().map(|r| r.downloaders()).sum(),
                completed: completion_times.len(),
                mean_end: runs.iter().map(|r| r.end.since_epoch()).sum::<Duration>() / n as u32,
                mean_overhead: mean(&|r| r.overhead()),
                mean_duplicate: mean(&|r| r.duplicate()),
                completion_times,
                first_byte_times,
                mean_utilization: mean_of(
                    runs.iter()
                        .filter_map(|r| r.upload_utilization(scenario))
                        .collect(),
                ),
                mean_fairness: mean_of(runs.iter().filter_map(|r| r.fairness()).collect()),
                mean_rarest: mean_of(runs.iter().filter_map(|r| r.rarest_copies()).collect()),
            })
        })
        .collect()
}

fn seconds(duration: Option<Duration>) -> String {
    duration.map_or_else(|| "-".to_owned(), |d| format!("{:.1}s", d.as_secs_f64()))
}

fn ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"))
}

fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n}B")
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

/// A table of variant summaries: completion counts and times and times to first byte (pooled
/// over seeds), and means per run.
pub fn table(summaries: &[VariantSummary]) -> String {
    let header = [
        "variant",
        "runs",
        "completed",
        "p50",
        "p90",
        "p99",
        "first byte",
        "utilization",
        "fairness",
        "rarest",
        "length",
        "overhead",
        "duplicate",
    ];
    let rows: Vec<[String; 13]> = summaries
        .iter()
        .map(|s| {
            let times = &s.completion_times;
            [
                s.name.clone(),
                s.runs.to_string(),
                format!("{}/{}", s.completed, s.downloaders),
                seconds(percentile(times, 0.5)),
                seconds(percentile(times, 0.9)),
                seconds(percentile(times, 0.99)),
                seconds(percentile(&s.first_byte_times, 0.5)),
                ratio(s.mean_utilization),
                ratio(s.mean_fairness),
                s.mean_rarest
                    .map_or_else(|| "-".to_owned(), |v| format!("{v:.1}")),
                seconds(Some(s.mean_end)),
                bytes(s.mean_overhead),
                bytes(s.mean_duplicate),
            ]
        })
        .collect();
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([header[i].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    let mut line = |cells: Vec<&str>| {
        let mut text = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i == 0 {
                let _ = write!(text, "{cell:<width$}", width = widths[i]);
            } else {
                let _ = write!(text, "  {cell:>width$}", width = widths[i]);
            }
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    line(header.to_vec());
    for row in &rows {
        line(row.iter().map(String::as_str).collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let secs: Vec<Duration> = (1..=10).map(Duration::from_secs).collect();
        assert_eq!(percentile(&secs, 0.5), Some(Duration::from_secs(5)));
        assert_eq!(percentile(&secs, 0.9), Some(Duration::from_secs(9)));
        assert_eq!(percentile(&secs, 0.99), Some(Duration::from_secs(10)));
        assert_eq!(percentile(&secs, 0.0), Some(Duration::from_secs(1)));
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn jains_index() {
        assert_eq!(jain(&[]), None);
        assert_eq!(jain(&[0.0, 0.0]), None);
        assert_eq!(jain(&[2.0, 2.0, 2.0]), Some(1.0));
        assert_eq!(jain(&[1.0, 0.0, 0.0, 0.0]), Some(0.25));
        let j = jain(&[1.0, 2.0]).unwrap();
        assert!((j - 0.9).abs() < 1e-12, "{j}");
    }

    #[test]
    fn byte_counts_read_well() {
        assert_eq!(bytes(512), "512B");
        assert_eq!(bytes(1536), "1.5KiB");
        assert_eq!(bytes(3 << 30), "3.0GiB");
    }
}
