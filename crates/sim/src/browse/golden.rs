//! Experiments checked against the golden baselines (D-077, D-171): an experiment whose
//! scenario has a baseline's name is compared with it on the seeds both have, as
//! `tempest sim baseline` would.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use duckdb::Connection;
use tempest_sim_api::data::{
    BaselineCheck, BaselineInfo, BaselineOutcome, BaselineReport, CheckInfo, Estimate, Method,
};

use super::{BrowseError, Result, measured};
use crate::baseline::{self, Baseline, BaselineError, QUICK_SEEDS};
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

    /// The scenarios that have baselines.
    pub fn names(&self) -> Result<BTreeSet<String>> {
        Ok(self.load()?.into_keys().collect())
    }
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// Experiment `id` checked against the baseline of its scenario's name, or `None` if it has
/// none. A missing experiment is [`BrowseError::NotFound`].
pub fn baseline_report(
    conn: &Connection,
    id: i64,
    baselines: &Baselines,
) -> Result<Option<BaselineReport>> {
    let name: Option<String> = duckdb::OptionalExt::optional(conn.query_row(
        "SELECT name FROM experiments WHERE experiment_id = ?",
        [id],
        |row| row.get(0),
    ))?;
    let name = name.ok_or_else(|| BrowseError::NotFound(format!("there is no experiment {id}")))?;
    let mut all = baselines.load()?;
    let Some((path, baseline)) = all.remove(&name) else {
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
        // As `tempest sim baseline` does: a quick check's few seeds by their means, more seed
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
pub fn baselines(conn: &Connection, baselines: &Baselines) -> Result<Vec<BaselineInfo>> {
    let mut statement = conn.prepare(
        "SELECT experiment_id, strftime(created_at, '%Y-%m-%d %H:%M:%S'), git_commit
         FROM experiments WHERE name = ? ORDER BY experiment_id DESC",
    )?;
    let mut infos = Vec::new();
    for (scenario, (path, baseline)) in baselines.load()? {
        let found: Vec<(i64, String, Option<String>)> = statement
            .query_map([&scenario], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<duckdb::Result<_>>()?;
        let mut experiments = Vec::new();
        for (id, created_at, git_commit) in found {
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
