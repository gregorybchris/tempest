//! The API's answers. Times are seconds of simulated time from the start of a run, as in the
//! results database.

use serde::{Deserialize, Serialize};

/// An experiment: one `tempest-sim run` of a scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentInfo {
    /// Its ID in the database.
    pub id: i64,
    /// The scenario's name.
    pub name: String,
    /// The scenario's description.
    pub description: String,
    /// The scenario file it was read from.
    pub scenario_file: Option<String>,
    /// The SHA-256 of that file, in hex.
    pub scenario_sha256: Option<String>,
    /// The git commit the simulator was built from.
    pub git_commit: Option<String>,
    /// When it was recorded, as `YYYY-MM-DD HH:MM:SS` in UTC.
    pub created_at: String,
    /// How many variants it compares.
    pub variants: u32,
    /// How many seeds each variant ran with.
    pub seeds: u32,
    /// How many runs it recorded.
    pub runs: u32,
    /// The most peers in any of its runs.
    pub peers: u32,
    /// Whether its runs' event logs were recorded.
    pub logged: bool,
    /// Whether a golden baseline has its scenario's name.
    pub baseline: bool,
}

/// A setting a variant varies, and its value there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    /// The setting, such as `strategies.upload`.
    pub key: String,
    /// Its value, as the scenario file would write it.
    pub value: String,
}

/// One combination of an experiment's varied settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantInfo {
    /// Its index in the experiment.
    pub index: u32,
    /// Its name, such as `strategies.upload=bittorrent`.
    pub name: String,
    /// The value of each varied setting.
    pub vary: Vec<Setting>,
    /// How many runs it recorded.
    pub runs: u32,
}

/// An experiment, its variants and its scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExperimentDetail {
    /// The experiment.
    pub experiment: ExperimentInfo,
    /// Its variants, in order.
    pub variants: Vec<VariantInfo>,
    /// The scenario as parsed, with its experiment.
    pub scenario: serde_json::Value,
}

/// A run and its headline metrics.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunInfo {
    /// Its ID in the database.
    pub id: i64,
    /// Its experiment's ID.
    pub experiment: i64,
    /// Its experiment's name.
    pub experiment_name: String,
    /// Its variant's index.
    pub variant: u32,
    /// Its variant's name.
    pub variant_name: String,
    /// Its seed.
    pub seed: u64,
    /// When it ended.
    pub end_s: f64,
    /// Events the driver handled.
    pub events: u64,
    /// Entries in its event log.
    pub log_entries: u64,
    /// The digest of its event log.
    pub log_sha256: String,
    /// Peers in the run.
    pub peers: u32,
    /// Peers that started without the bundle.
    pub downloaders: u32,
    /// Downloaders that completed.
    pub completed: u32,
    /// The median completion time, from arrival.
    pub completion_p50_s: Option<f64>,
    /// The 90th percentile completion time.
    pub completion_p90_s: Option<f64>,
    /// The 99th percentile completion time.
    pub completion_p99_s: Option<f64>,
    /// The median time from arrival to the first verified payload.
    pub first_byte_p50_s: Option<f64>,
    /// The median time from arrival to the first open connection.
    pub first_peer_p50_s: Option<f64>,
    /// Payload uploaded over upload capacity while present.
    pub upload_utilization: Option<f64>,
    /// Jain's index of upload/download ratios.
    pub fairness: Option<f64>,
    /// Present peers holding the rarest chunk, on average over samples.
    pub rarest_copies: Option<f64>,
    /// Payload bytes sent.
    pub payload_bytes: u64,
    /// Every other byte sent.
    pub overhead_bytes: u64,
    /// Payload received more than once, in endgame.
    pub duplicate_bytes: u64,
}

/// A page of runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunList {
    /// The runs on this page.
    pub runs: Vec<RunInfo>,
    /// How many runs match the filters, on every page.
    pub total: u64,
}

/// What the peers of one class did in a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassSummary {
    /// The class's name.
    pub class: String,
    /// How its peers behave, such as `free-rider`.
    pub behavior: String,
    /// `empty` or `complete`.
    pub starts: String,
    /// Its peers.
    pub peers: u32,
    /// Those that completed.
    pub completed: u32,
    /// Their median time from arrival to completion.
    pub completion_p50_s: Option<f64>,
    /// Verified payload bytes they received.
    pub downloaded: u64,
    /// Payload bytes they sent.
    pub uploaded: u64,
}

/// One peer of a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Its index in the run.
    pub peer: u32,
    /// Its class.
    pub class: String,
    /// `empty` or `complete`.
    pub starts: String,
    /// How it behaves.
    pub behavior: String,
    /// Its upload capacity in bytes per second, or `None` if unlimited.
    pub upload_capacity: Option<f64>,
    /// Its download capacity.
    pub download_capacity: Option<f64>,
    /// When it arrived.
    pub arrived_s: Option<f64>,
    /// When it first had an open connection.
    pub first_peer_s: Option<f64>,
    /// When it first received verified payload.
    pub first_byte_s: Option<f64>,
    /// When it completed.
    pub completed_s: Option<f64>,
    /// When it left.
    pub left_s: Option<f64>,
    /// Verified payload bytes received.
    pub downloaded: u64,
    /// Payload bytes sent.
    pub uploaded: u64,
    /// Payload bytes received of chunks it already had.
    pub duplicate: u64,
    /// Payload bytes received that failed verification.
    pub bad: u64,
}

/// One kind of message in a run's traffic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrafficInfo {
    /// The message, such as `Data`.
    pub message: String,
    /// How many were sent.
    pub count: u64,
    /// Their bytes on the wire.
    pub bytes: u64,
    /// Of those, the bytes that weren't payload.
    pub overhead_bytes: u64,
}

/// A run, its variant's settings, and what its peers and messages did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunDetail {
    /// The run.
    pub run: RunInfo,
    /// The settings its variant varies.
    pub vary: Vec<Setting>,
    /// Its peers by class, in the order they first appear.
    pub classes: Vec<ClassSummary>,
    /// Its traffic by message, most bytes first.
    pub traffic: Vec<TrafficInfo>,
    /// Whether its event log was recorded.
    pub logged: bool,
}

/// One sample of how many present peers held each chunk.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvailabilitySample {
    /// When it was taken.
    pub time_s: f64,
    /// Peers present.
    pub peers: u32,
    /// Present peers holding the rarest chunk.
    pub rarest: u32,
    /// Present peers holding a chunk, on average.
    pub mean: f64,
}

/// A run over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSeries {
    /// How the chunks' availability went.
    pub availability: Vec<AvailabilitySample>,
    /// Peers that started without the bundle.
    pub downloaders: u32,
    /// Each completion: its time, and how many downloaders had completed by then.
    pub completed: Vec<(f64, u32)>,
    /// Payload sent per second, averaged over bins of [`RunSeries::bin_s`], from the event log;
    /// empty if it wasn't recorded.
    pub throughput: Vec<(f64, f64)>,
    /// The width of the throughput's bins.
    pub bin_s: f64,
}

/// What a metric's values mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricUnit {
    /// A fraction from 0 to 1.
    Fraction,
    /// Seconds.
    Seconds,
    /// A count or index.
    Number,
}

/// A median across seeds with its confidence interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// How many values it summarizes.
    pub n: u32,
    /// Their median.
    pub median: f64,
    /// The interval's lower bound.
    pub low: f64,
    /// The interval's upper bound.
    pub high: f64,
}

/// Whether a difference is meaningful (D-167).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The interval excludes zero, and the effect is at least the minimum.
    Meaningful,
    /// The interval includes zero, or the effect is too small to matter.
    NotMeaningful,
    /// Too few paired seeds to say.
    TooFewPairs,
}

/// A variant's difference from the reference, paired by seed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Difference {
    /// The median of the per-seed differences (variant − reference), with its interval.
    pub estimate: Estimate,
    /// That median over the reference's median on the same seeds; `None` if that is 0.
    pub relative: Option<f64>,
    /// Whether it is meaningful.
    pub verdict: Verdict,
}

/// One metric of one compared variant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// The median across seeds, if any run has a value.
    pub estimate: Option<Estimate>,
    /// The difference from the reference, for other variants.
    pub difference: Option<Difference>,
}

/// One metric across the compared variants.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricRow {
    /// The metric's name, such as `completion p50`.
    pub name: String,
    /// What its values mean.
    pub unit: MetricUnit,
    /// Whether smaller values are better.
    pub lower_is_better: bool,
    /// One cell per compared variant, in their order.
    pub cells: Vec<Cell>,
}

/// A variant chosen for a comparison.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComparedVariant {
    /// Its experiment's ID.
    pub experiment: i64,
    /// Its experiment's name.
    pub experiment_name: String,
    /// Its index in the experiment.
    pub variant: u32,
    /// Its name.
    pub name: String,
    /// The seeds of its runs.
    pub seeds: Vec<u64>,
}

/// Variants compared metric by metric with a reference (D-073, D-167).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    /// The variants, in the order asked for.
    pub variants: Vec<ComparedVariant>,
    /// The position of the reference among them.
    pub reference: usize,
    /// The confidence level of the intervals, such as 0.95.
    pub confidence: f64,
    /// The fewest paired seeds for a verdict.
    pub min_pairs: u32,
    /// The smallest effect worth reporting, relative to the reference.
    pub min_effect: f64,
    /// Each metric.
    pub metrics: Vec<MetricRow>,
}

/// A median across seeds at one moment, with its interval.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BandPoint {
    /// The moment.
    pub t: f64,
    /// The median across seeds.
    pub median: f64,
    /// The interval's lower bound.
    pub low: f64,
    /// The interval's upper bound.
    pub high: f64,
}

/// One quantity over time, for each compared variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesBands {
    /// A stable name: `completed`, `rarest` or `present`.
    pub key: String,
    /// What it shows, in words.
    pub title: String,
    /// What its values mean.
    pub unit: MetricUnit,
    /// One curve per compared variant, in their order.
    pub variants: Vec<Vec<BandPoint>>,
}

/// The compared variants over time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComparisonSeries {
    /// The variants, in the order asked for.
    pub variants: Vec<ComparedVariant>,
    /// The confidence level of the bands.
    pub confidence: f64,
    /// Each quantity.
    pub series: Vec<SeriesBands>,
}

/// How runs are compared with a baseline (D-171).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Means over the seeds, as a quick check does.
    Mean,
    /// Seed by seed, with the median change and its interval, as a long check does.
    Paired,
}

/// One metric compared with its baseline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CheckInfo {
    /// The metric.
    pub metric: String,
    /// The baseline over the seeds compared.
    pub baseline: Option<f64>,
    /// The runs, summarized the same way.
    pub current: Option<f64>,
    /// The worst value that passes.
    pub limit: Option<f64>,
    /// With [`Method::Paired`], the median per-seed change (current − baseline).
    pub change: Option<Estimate>,
    /// Whether it got worse by more than its tolerance.
    pub regressed: bool,
    /// Whether it got better by more than its tolerance, so the baseline is out of date.
    pub improved: bool,
}

/// One variant of an experiment checked against its scenario's golden baseline (D-077).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaselineCheck {
    /// The variant's index.
    pub variant: u32,
    /// Its name.
    pub variant_name: String,
    /// How the runs were compared.
    pub method: Method,
    /// The seeds compared.
    pub seeds: Vec<u64>,
    /// Seeds the runs have and the baseline lacks, which were left out.
    pub missing: Vec<u64>,
    /// Each metric.
    pub checks: Vec<CheckInfo>,
}

/// An experiment checked against the golden baseline of the same name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaselineReport {
    /// The scenario.
    pub scenario: String,
    /// The baseline file.
    pub file: String,
    /// The confidence level of paired changes.
    pub confidence: f64,
    /// Each variant.
    pub variants: Vec<BaselineCheck>,
}

/// How an experiment fared against its baseline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineOutcome {
    /// The experiment's ID.
    pub experiment: i64,
    /// When it was recorded.
    pub created_at: String,
    /// The commit it was built from.
    pub git_commit: Option<String>,
    /// Metrics that regressed, over every variant.
    pub regressed: u32,
    /// Metrics that improved beyond their tolerance.
    pub improved: u32,
    /// Why it couldn't be checked, such as no seed in common.
    pub problem: Option<String>,
}

/// A golden baseline, and the experiments of its scenario.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaselineInfo {
    /// The scenario.
    pub scenario: String,
    /// The baseline file.
    pub file: String,
    /// The seeds it saved.
    pub seeds: Vec<u64>,
    /// The experiments recorded with its scenario's name, newest first.
    pub experiments: Vec<BaselineOutcome>,
}
