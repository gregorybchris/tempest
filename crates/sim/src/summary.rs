//! Summaries of runs and of each variant across its seeds.

use std::fmt::Write as _;
use std::time::Duration;

use crate::engine::RunResult;
use crate::scenario::{Scenario, Start};

impl RunResult {
    /// How long each peer that started without the bundle and completed took, from arriving to
    /// completing, shortest first.
    pub fn completion_times(&self) -> Vec<Duration> {
        let mut times: Vec<Duration> = self
            .peers
            .iter()
            .filter_map(|p| Some(p.completed?.since(p.arrived?)))
            .collect();
        times.sort();
        times
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
            let mean =
                |f: &dyn Fn(&RunResult) -> u64| runs.iter().map(|r| f(r)).sum::<u64>() / n as u64;
            Some(VariantSummary {
                name: variant.name(),
                runs: n,
                downloaders: runs.iter().map(|r| r.downloaders()).sum(),
                completed: completion_times.len(),
                mean_end: runs.iter().map(|r| r.end.since_epoch()).sum::<Duration>() / n as u32,
                mean_overhead: mean(&|r| r.overhead()),
                mean_duplicate: mean(&|r| r.duplicate()),
                completion_times,
            })
        })
        .collect()
}

fn seconds(duration: Option<Duration>) -> String {
    duration.map_or_else(|| "-".to_owned(), |d| format!("{:.1}s", d.as_secs_f64()))
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

/// A table of variant summaries: completion counts and times (pooled over seeds), and means
/// per run.
pub fn table(summaries: &[VariantSummary]) -> String {
    let header = [
        "variant",
        "runs",
        "completed",
        "p50",
        "p90",
        "p99",
        "length",
        "overhead",
        "duplicate",
    ];
    let rows: Vec<[String; 9]> = summaries
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
    fn byte_counts_read_well() {
        assert_eq!(bytes(512), "512B");
        assert_eq!(bytes(1536), "1.5KiB");
        assert_eq!(bytes(3 << 30), "3.0GiB");
    }
}
