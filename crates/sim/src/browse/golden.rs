//! Experiments checked against the golden baselines (D-077, D-171): an experiment whose
//! scenario has a baseline's name, and came from the same version of its file, is compared
//! with it on the seeds both have, as `tempest-sim baseline` would.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use duckdb::Connection;
use tempest_sim_api::data::{
    BaselineCheck, BaselineInfo, BaselineOutcome, BaselineReport, CheckInfo, Estimate, Method,
};

use super::{BrowseError, Result, measured};
use crate::baseline::{self, Baseline, BaselineError, QUICK_SEEDS};
use crate::results::Database;
use crate::stats::Bootstrap;

/// Where the golden baselines are, if anywhere. They are read afresh for every answer, so an
/// update shows without a restart.
#[derive(Debug, Clone, Default)]
pub struct Baselines {
    dir: Option<PathBuf>,
}

impl Baselines {
    /// The baselines in `dir`: every `NAME.json` file in it. With `None`, or a directory that
    /// doesn't exist, there are none.
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir }
    }

    /// Every baseline and its file, by scenario name.
    fn load(&self) -> Result<BTreeMap<String, (PathBuf, Baseline)>> {
        let Some(dir) = self.dir.as_deref().filter(|d| d.is_dir()) else {
            return Ok(BTreeMap::new());
        };
        let io = |source| BaselineError::Io {
            path: dir.display().to_string(),
            source,
        };
        let mut baselines = BTreeMap::new();
        for entry in std::fs::read_dir(dir).map_err(io)? {
            let path = entry.map_err(io)?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let baseline = Baseline::load(&path)?;
                baselines.insert(baseline.scenario.clone(), (path, baseline));
            }
        }
        Ok(baselines)
    }

    /// The scenario files that have baselines: each scenario's name and its file's hash.
    pub(super) fn scenarios(&self) -> Result<Scenarios> {
        let baselines = self.load()?;
        Ok(Scenarios(
            baselines
                .into_iter()
                .map(|(name, (_, baseline))| (name, baseline.scenario_sha256))
                .collect(),
        ))
    }
}

/// The scenario files that have baselines, by name, with their hashes.
pub(super) struct Scenarios(BTreeMap<String, String>);

impl Scenarios {
    /// Whether an experiment of the scenario `name`, from a file with hash `sha256`, has a
    /// baseline: one of the same name saved from the same file.
    pub(super) fn checks(&self, name: &str, sha256: Option<&str>) -> bool {
        self.0
            .get(name)
            .is_some_and(|saved| sha256 == Some(saved.as_str()))
    }
}

/// What keeps an experiment from being checked against the baseline of its scenario's name.
const EDITED: &str = "its scenario file differs from the one the baseline was saved from";

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// Experiment `id` checked against the baseline of its scenario's name, or `None` if it has
/// none or the baseline was saved from another version of its scenario file. A missing
/// experiment is [`BrowseError::NotFound`].
pub fn baseline_report(
    db: &Database,
    id: i64,
    baselines: &Baselines,
) -> Result<Option<BaselineReport>> {
    let conn = &*db.conn();
    let found: Option<(String, Option<String>)> = duckdb::OptionalExt::optional(conn.query_row(
        "SELECT name, scenario_sha256 FROM experiments WHERE experiment_id = ?",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ))?;
    let (name, sha256) =
        found.ok_or_else(|| BrowseError::NotFound(format!("there is no experiment {id}")))?;
    let mut all = baselines.load()?;
    let Some((path, baseline)) = all
        .remove(&name)
        .filter(|(_, b)| sha256.as_deref() == Some(b.scenario_sha256.as_str()))
    else {
        return Ok(None);
    };
    Ok(Some(report(conn, id, &name, &path, &baseline)?))
}

fn report(
    conn: &Connection,
    id: i64,
    scenario: &str,
    path: &Path,
    baseline: &Baseline,
) -> Result<BaselineReport> {
    let mut statement = conn
        .prepare("SELECT variant, name FROM variants WHERE experiment_id = ? ORDER BY variant")?;
    let variants: Vec<(u32, String)> = statement
        .query_map([id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<duckdb::Result<_>>()?;
    let bootstrap = Bootstrap::default();
    let mut checked = Vec::new();
    for (variant, variant_name) in variants {
        let (current, missing): (Vec<_>, Vec<_>) = measured(conn, id, variant)?
            .into_iter()
            .partition(|m| baseline.runs.contains_key(&m.seed));
        let current: BTreeMap<u64, baseline::Metrics> = current
            .iter()
            .map(|m| (m.seed, baseline::Metrics::from(&m.metrics)))
            .collect();
        // As `tempest-sim baseline` does: a quick check's few seeds by their means, more seed
        // by seed.
        let (method, how) = if current.len() as u32 <= QUICK_SEEDS {
            (Method::Mean, baseline::Method::Mean)
        } else {
            (Method::Paired, baseline::Method::Paired(bootstrap))
        };
        let checks = match current.is_empty() {
            true => Vec::new(),
            false => baseline.compare(&current, how)?,
        };
        checked.push(BaselineCheck {
            variant,
            variant_name,
            method,
            seeds: current.keys().copied().collect(),
            missing: missing.iter().map(|m| m.seed).collect(),
            checks: checks
                .into_iter()
                .map(|c| CheckInfo {
                    metric: c.metric.to_owned(),
                    baseline: c.baseline,
                    current: c.current,
                    limit: c.limit,
                    change: c.change.map(|e| Estimate {
                        n: u32::try_from(e.n).unwrap_or(u32::MAX),
                        median: e.median,
                        low: e.interval.low,
                        high: e.interval.high,
                    }),
                    regressed: c.regressed,
                    improved: c.improved,
                })
                .collect(),
        });
    }
    Ok(BaselineReport {
        scenario: scenario.to_owned(),
        file: display(path),
        confidence: bootstrap.confidence,
        variants: checked,
    })
}

/// Every golden baseline, with the experiments of its scenario and how each fared.
pub fn baselines(db: &Database, baselines: &Baselines) -> Result<Vec<BaselineInfo>> {
    let conn = &*db.conn();
    let mut statement = conn.prepare(
        "SELECT experiment_id, strftime(created_at, '%Y-%m-%d %H:%M:%S'), git_commit,
            scenario_sha256
         FROM experiments WHERE name = ? ORDER BY experiment_id DESC",
    )?;
    let mut infos = Vec::new();
    for (scenario, (path, baseline)) in baselines.load()? {
        type Found = (i64, String, Option<String>, Option<String>);
        let found: Vec<Found> = statement
            .query_map([&scenario], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?
            .collect::<duckdb::Result<_>>()?;
        let mut experiments = Vec::new();
        for (id, created_at, git_commit, sha256) in found {
            if sha256.as_deref() != Some(baseline.scenario_sha256.as_str()) {
                experiments.push(BaselineOutcome {
                    experiment: id,
                    created_at,
                    git_commit,
                    regressed: 0,
                    improved: 0,
                    problem: Some(EDITED.to_owned()),
                });
                continue;
            }
            let report = report(conn, id, &scenario, &path, &baseline)?;
            let checks = || report.variants.iter().flat_map(|v| &v.checks);
            let compared = report.variants.iter().any(|v| !v.seeds.is_empty());
            experiments.push(BaselineOutcome {
                experiment: id,
                created_at,
                git_commit,
                regressed: checks().filter(|c| c.regressed).count() as u32,
                improved: checks().filter(|c| c.improved).count() as u32,
                problem: (!compared).then(|| "it has no seed the baseline saved".to_owned()),
            });
        }
        infos.push(BaselineInfo {
            scenario,
            file: display(&path),
            seeds: baseline.runs.keys().copied().collect(),
            experiments,
        });
    }
    Ok(infos)
}
