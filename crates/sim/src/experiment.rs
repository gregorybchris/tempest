//! Running an experiment: every variant with every seed.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::engine::{RunOptions, RunResult, run};
use crate::scenario::{Scenario, ScenarioError};

/// The seeds a scenario asks for: 1 to `seeds`.
pub fn seeds(scenario: &Scenario) -> Vec<u64> {
    (1..=u64::from(scenario.seeds)).collect()
}

/// Runs every variant of `scenario` with each of `seeds`, on up to `threads` threads at once.
///
/// Runs are independent and each is deterministic, so the results are the same however many
/// threads run them. They come back ordered by variant, then seed. `done` is called as each run
/// finishes, in whatever order they finish.
pub fn run_all(
    scenario: &Scenario,
    seeds: &[u64],
    threads: NonZeroUsize,
    options: RunOptions,
    done: &(dyn Fn(&RunResult) + Sync),
) -> Result<Vec<RunResult>, ScenarioError> {
    let variants = scenario.variants();
    let jobs: Vec<(usize, u64)> = (0..variants.len())
        .flat_map(|v| seeds.iter().map(move |&seed| (v, seed)))
        .collect();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(jobs.len()));
    std::thread::scope(|scope| {
        for _ in 0..threads.get().min(jobs.len()) {
            scope.spawn(|| {
                while let Some(&(variant, seed)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let result = run(scenario, &variants[variant], seed, options);
                    if let Ok(result) = &result {
                        done(result);
                    }
                    results.lock().expect("no run panics").push(result);
                }
            });
        }
    });
    let mut results = results
        .into_inner()
        .expect("no run panics")
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    results.sort_by_key(|r| (r.variant, r.seed));
    Ok(results)
}
