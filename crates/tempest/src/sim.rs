//! `tempest sim`: runs simulation scenarios.

use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context as _, Result};
use clap::Subcommand;
use tempest_sim::experiment::{Observer, run_all, seeds};
use tempest_sim::log::Sink;
use tempest_sim::results::{Database, Recorder, ScenarioFile};
use tempest_sim::stats::{self, Bootstrap, Meaningful, Samples};
use tempest_sim::summary::{summarize, table};
use tempest_sim::{RunOptions, RunResult, Scenario};

#[derive(Debug, Subcommand)]
pub enum SimCommand {
    /// Run every variant of a scenario with each of its seeds, record the results and print a
    /// summary.
    Run(RunArgs),
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// The scenario file (YAML).
    scenario: PathBuf,
    /// Run seeds 1 to N instead of the scenario's count.
    #[arg(long, value_name = "N", conflicts_with = "seed", value_parser = clap::value_parser!(u32).range(1..))]
    seeds: Option<u32>,
    /// Run only this seed.
    #[arg(long)]
    seed: Option<u64>,
    /// How many runs at once. Defaults to the number of CPUs.
    #[arg(long, short)]
    jobs: Option<NonZeroUsize>,
    /// The DuckDB results database to add the experiment to. It is created if missing.
    #[arg(long, value_name = "PATH", default_value = "tempest-sim.duckdb")]
    db: PathBuf,
    /// Don't record results.
    #[arg(long, conflicts_with = "db")]
    no_db: bool,
    /// Record summaries only, not every run's event log.
    #[arg(long)]
    no_events: bool,
    /// Write each run's variant, seed and event-log digest to this file, one run per line.
    #[arg(long, value_name = "FILE")]
    digests: Option<PathBuf>,
    /// The variant the others are compared with: its name, or a part of the name only it has.
    /// Defaults to the first.
    #[arg(long, value_name = "VARIANT")]
    reference: Option<String>,
}

pub fn sim(command: SimCommand) -> Result<()> {
    match command {
        SimCommand::Run(args) => run(args),
    }
}

/// Prints each run as it finishes and passes it on to the recorder.
struct Progress<'a> {
    recorder: Option<&'a Recorder<'a>>,
    names: &'a [String],
    total: usize,
    finished: Mutex<usize>,
}

impl Observer for Progress<'_> {
    fn sink(&self, variant: usize, seed: u64) -> Option<Box<dyn Sink + '_>> {
        self.recorder?.sink(variant, seed)
    }

    fn finished(&self, result: &RunResult) {
        if let Some(recorder) = self.recorder {
            recorder.finished(result);
        }
        let mut finished = self.finished.lock().unwrap_or_else(|e| e.into_inner());
        *finished += 1;
        eprintln!(
            "[{finished}/{}] {} seed {}: {}/{} completed, {:.1}s simulated, {} events",
            self.total,
            self.names[result.variant],
            result.seed,
            result.completion_times().len(),
            result.downloaders(),
            result.end.since_epoch().as_secs_f64(),
            result.events,
        );
    }
}

fn run(args: RunArgs) -> Result<()> {
    let (mut scenario, text) = Scenario::load_text(&args.scenario)?;
    if let Some(count) = args.seeds {
        scenario.seeds = count;
    }
    let seeds = match args.seed {
        Some(seed) => vec![seed],
        None => seeds(&scenario),
    };
    let jobs = args
        .jobs
        .or_else(|| std::thread::available_parallelism().ok())
        .unwrap_or(NonZeroUsize::MIN);
    let variants = scenario.variants();
    let names: Vec<String> = variants.iter().map(|v| v.name()).collect();
    let reference = match &args.reference {
        Some(wanted) => find_variant(&names, wanted)?,
        None => 0,
    };
    let total = variants.len() * seeds.len();

    println!(
        "scenario  {} ({} peers)",
        scenario.name,
        scenario.peer_count()
    );
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    println!(
        "runs      {} × {} = {}, {} at a time",
        plural(variants.len(), "variant"),
        plural(seeds.len(), "seed"),
        plural(total, "run"),
        jobs.get().min(total)
    );

    let db = if args.no_db {
        None
    } else {
        Some(Database::open(&args.db).with_context(|| format!("opening {}", args.db.display()))?)
    };
    let file = ScenarioFile::new(&args.scenario, &text);
    let recorder = db
        .as_ref()
        .map(|db| db.record(&scenario, Some(&file), &seeds, !args.no_events))
        .transpose()?;
    let progress = Progress {
        recorder: recorder.as_ref(),
        names: &names,
        total,
        finished: Mutex::new(0),
    };
    let started = Instant::now();
    let results = run_all(&scenario, &seeds, jobs, RunOptions::default(), &progress)?;
    println!("took      {:.1}s", started.elapsed().as_secs_f64());
    if let Some(recorder) = recorder {
        let experiment = recorder.finish()?;
        println!("results   {} (experiment {experiment})", args.db.display());
    }
    if let Some(path) = &args.digests {
        let mut out = String::new();
        for result in &results {
            let name = &names[result.variant];
            writeln!(out, "{name}\t{}\t{}", result.seed, result.digest)?;
        }
        std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    }
    println!();
    print!("{}", table(&summarize(&scenario, &results)));
    if seeds.len() > 1 {
        let samples = stats::samples(&scenario, &results);
        print!("{}", comparison(&samples, &names[reference]));
    }
    Ok(())
}

/// The variant `wanted` names: exactly, or as a part of only its name.
fn find_variant(names: &[String], wanted: &str) -> Result<usize> {
    if let Some(index) = names.iter().position(|n| n == wanted) {
        return Ok(index);
    }
    let matches: Vec<usize> = (0..names.len())
        .filter(|&i| names[i].contains(wanted))
        .collect();
    match matches[..] {
        [index] => Ok(index),
        _ => anyhow::bail!(
            "--reference {wanted:?} must name exactly one of the variants: {}",
            names.join(", ")
        ),
    }
}

/// Medians across seeds with their confidence intervals, and each variant's paired difference
/// from `reference` (D-073).
fn comparison(samples: &[Samples], reference: &str) -> String {
    let bootstrap = Bootstrap::default();
    let reference = samples
        .iter()
        .position(|s| s.name == reference)
        .unwrap_or(0);
    let rows = stats::compare(samples, reference, bootstrap, Meaningful::default());
    format!(
        "\nacross seeds: medians, and differences from the reference paired by seed\n\n{}",
        stats::table(&rows, bootstrap.confidence)
    )
}
