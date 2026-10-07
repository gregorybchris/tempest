//! `tempest sim`: runs simulation scenarios.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Result;
use clap::Subcommand;
use tempest_sim::experiment::{run_all, seeds};
use tempest_sim::summary::{summarize, table};
use tempest_sim::{RunOptions, RunResult, Scenario};

#[derive(Debug, Subcommand)]
pub enum SimCommand {
    /// Run every variant of a scenario with each of its seeds and print a summary.
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
}

pub fn sim(command: SimCommand) -> Result<()> {
    match command {
        SimCommand::Run(args) => run(args),
    }
}

fn run(args: RunArgs) -> Result<()> {
    let mut scenario = Scenario::load(&args.scenario)?;
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
    let names: Vec<String> = variants.iter().map(|v| v.name()).collect();
    let started = Instant::now();
    let finished = Mutex::new(0);
    let progress = |result: &RunResult| {
        let mut finished = finished.lock().expect("never poisoned");
        *finished += 1;
        let completed = result.completion_times().len();
        eprintln!(
            "[{finished}/{total}] {} seed {}: {completed}/{} completed, {:.1}s simulated, {} events",
            names[result.variant],
            result.seed,
            result.downloaders(),
            result.end.since_epoch().as_secs_f64(),
            result.events,
        );
    };
    let results = run_all(&scenario, &seeds, jobs, RunOptions::default(), &progress)?;
    println!("took      {:.1}s\n", started.elapsed().as_secs_f64());
    print!("{}", table(&summarize(&scenario, &results)));
    Ok(())
}
