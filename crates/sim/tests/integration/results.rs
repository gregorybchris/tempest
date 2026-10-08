//! Results in DuckDB: experiments, runs, metrics and event logs land in the database and agree
//! with the runs that made them.

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use tempest_sim::experiment::{run_all, seeds};
use tempest_sim::log::{Row, Sink};
use tempest_sim::results::duckdb::Connection;
use tempest_sim::results::duckdb::types::FromSql;
use tempest_sim::results::{Database, GIT_COMMIT, ScenarioFile};
use tempest_sim::{RunOptions, RunResult, Scenario, run, run_with_sink};

fn example_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(name)
}

fn threads() -> NonZeroUsize {
    std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN)
}

/// Runs every variant and seed of `scenario`, recording into `db`.
fn record(
    db: &Database,
    scenario: &Scenario,
    file: Option<&ScenarioFile>,
    events: bool,
) -> (i64, Vec<RunResult>) {
    let seeds = seeds(scenario);
    let recorder = db.record(scenario, file, &seeds, events).unwrap();
    let results = run_all(
        scenario,
        &seeds,
        threads(),
        RunOptions::default(),
        &recorder,
    )
    .unwrap();
    (recorder.finish().unwrap(), results)
}

fn count(db: &Database, sql: &str) -> i64 {
    db.connection()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

const SMALL: &str = "
name: small
seeds: 2
sample_interval: 500ms
bundle: { files: [{ size: 1MiB }, { size: 100KiB }] }
network: { latency: 10ms, loss: 1% }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps, down: 100Mbps } }
  - class: home
    count: 6
    bandwidth: { up: 5Mbps, down: 50Mbps }
    arrivals: { pattern: flash-crowd, decay: 2s }
  - { class: free-rider, behavior: free-rider, bandwidth: { up: 5Mbps, down: 50Mbps } }
experiment:
  vary:
    strategies.upload: [bittorrent, cooperative]
";

#[test]
fn an_experiment_lands_in_the_database() {
    let path = example_path("two-peers.yaml");
    let (scenario, text) = Scenario::load_text(&path).unwrap();
    let file = ScenarioFile::new(&path, &text);
    let db = Database::open_in_memory().unwrap();
    let (experiment, results) = record(&db, &scenario, Some(&file), true);
    assert_eq!(experiment, 1);

    let conn = db.connection();
    let (name, sha256, commit): (String, String, Option<String>) = conn
        .query_row(
            "SELECT name, scenario_sha256, git_commit FROM experiments",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(name, "two-peers");
    assert_eq!(sha256, file.sha256);
    assert_eq!(sha256.len(), 64);
    assert_eq!(commit.as_deref(), GIT_COMMIT);
    let payload: String = conn
        .query_row("SELECT scenario->>'payload' FROM variants", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(payload, "real");

    let runs = |column: &str| format!("SELECT {column} FROM runs ORDER BY run_id");
    let each = |f: &dyn Fn(&RunResult) -> u64| results.iter().map(f).collect::<Vec<_>>();
    let total = 3 * 1024 * 1024 + 2 * 100 * 1024;
    assert_eq!(column::<u64>(&conn, &runs("seed")), each(&|r| r.seed));
    assert_eq!(
        column::<u64>(&conn, &runs("log_entries")),
        each(&|r| r.log_len)
    );
    assert_eq!(
        column::<String>(&conn, &runs("log_sha256")),
        results.iter().map(|r| r.digest.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        column::<i64>(&conn, &runs("completed")),
        vec![1; results.len()],
        "the downloader completed"
    );
    assert_eq!(
        column::<u64>(&conn, &runs("payload_bytes")),
        vec![total; results.len()]
    );
    assert_eq!(
        column::<u64>(&conn, &runs("overhead_bytes")),
        each(&|r| r.overhead())
    );
    assert_eq!(
        column::<f64>(&conn, &runs("completion_p50_s")),
        results
            .iter()
            .map(|r| r.completion_times()[0].as_secs_f64())
            .collect::<Vec<_>>()
    );

    // The event log is complete and in order, and the downloader received the whole bundle
    // as verified Data.
    let entries: u64 = results.iter().map(|r| r.log_len).sum();
    assert_eq!(
        count_in(&conn, "SELECT count(*) FROM events"),
        entries as i64
    );
    assert_eq!(
        count_in(
            &conn,
            "SELECT count(*) FROM (SELECT run_id, max(seq) + 1 = count(*) AS dense
             FROM events GROUP BY run_id) WHERE dense"
        ),
        results.len() as i64
    );
    let received: i64 = conn
        .query_row(
            "SELECT sum(bytes) FROM events
             WHERE kind = 'received' AND peer = 1 AND run_id = (SELECT min(run_id) FROM runs)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(received, total as i64);
    let intact = count_in(&conn, "SELECT count(*) FROM peers WHERE intact");
    assert_eq!(intact, results.len() as i64);
}

fn count_in(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// The first column of every row `sql` returns.
fn column<T: FromSql>(conn: &Connection, sql: &str) -> Vec<T> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn metrics_match_the_runs() {
    let scenario = Scenario::from_yaml(SMALL).unwrap();
    let db = Database::open_in_memory().unwrap();
    let (_, results) = record(&db, &scenario, None, false);
    assert_eq!(results.len(), 4);
    assert_eq!(count(&db, "SELECT count(*) FROM variants"), 2);
    assert_eq!(
        count(&db, "SELECT count(*) FROM events"),
        0,
        "events were off"
    );
    assert_eq!(count(&db, "SELECT count(*) FROM peers"), 4 * 8);

    // Run IDs follow variant, then seed, whichever run finished first.
    let conn = db.connection();
    let runs = |column: &str| format!("SELECT {column} FROM runs ORDER BY run_id");
    let variants: Vec<i64> = results.iter().map(|r| r.variant as i64).collect();
    let seeds: Vec<u64> = results.iter().map(|r| r.seed).collect();
    assert_eq!(column::<i64>(&conn, &runs("variant")), variants);
    assert_eq!(column::<u64>(&conn, &runs("seed")), seeds);
    let metrics: Vec<_> = results.iter().map(|r| r.metrics(&scenario)).collect();
    let fairness: Vec<Option<f64>> = metrics.iter().map(|m| m.fairness).collect();
    let utilization: Vec<Option<f64>> = metrics.iter().map(|m| m.upload_utilization).collect();
    let first_byte: Vec<Option<f64>> = metrics
        .iter()
        .map(|m| m.first_byte_p50.map(|d| d.as_secs_f64()))
        .collect();
    let duplicate: Vec<u64> = metrics.iter().map(|m| m.duplicate).collect();
    assert_eq!(column::<Option<f64>>(&conn, &runs("fairness")), fairness);
    assert_eq!(
        column::<Option<f64>>(&conn, &runs("upload_utilization")),
        utilization
    );
    assert_eq!(
        column::<Option<f64>>(&conn, &runs("first_byte_p50_s")),
        first_byte
    );
    assert_eq!(column::<u64>(&conn, &runs("duplicate_bytes")), duplicate);
    let rarest: Vec<Option<f64>> = metrics.iter().map(|m| m.rarest_copies).collect();
    assert_eq!(column::<Option<f64>>(&conn, &runs("rarest_copies")), rarest);
    let samples: usize = results.iter().map(|r| r.availability.len()).sum();
    assert!(samples > 0);
    assert_eq!(
        count_in(&conn, "SELECT count(*) FROM availability") as usize,
        samples
    );
    for m in &metrics {
        let fairness = m.fairness.unwrap();
        assert!(fairness > 0.0 && fairness <= 1.0, "{fairness}");
        let utilization = m.upload_utilization.unwrap();
        assert!(utilization > 0.0 && utilization <= 1.0, "{utilization}");
    }

    // The free rider uploaded nothing, and the traffic table accounts for every byte sent.
    let free_riders = count_in(
        &conn,
        "SELECT count(*) FROM peers WHERE behavior = 'free-rider' AND uploaded = 0",
    );
    assert_eq!(free_riders, 4);
    let mismatched = count_in(
        &conn,
        "SELECT count(*) FROM runs r JOIN (
             SELECT run_id, sum(overhead_bytes) AS overhead, sum(bytes) AS bytes
             FROM traffic GROUP BY run_id) t USING (run_id)
         WHERE t.overhead != r.overhead_bytes OR t.bytes != r.overhead_bytes + r.payload_bytes",
    );
    assert_eq!(mismatched, 0);
}

#[test]
fn experiments_get_fresh_ids_in_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.duckdb");
    let scenario = Scenario::from_yaml(SMALL).unwrap();
    {
        let db = Database::open(&path).unwrap();
        assert_eq!(record(&db, &scenario, None, true).0, 1);
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(record(&db, &scenario, None, true).0, 2);
    assert_eq!(count(&db, "SELECT count(DISTINCT run_id) FROM runs"), 8);
    assert_eq!(count(&db, "SELECT count(DISTINCT run_id) FROM events"), 8);
    // The same seeds replay the same logs in both experiments.
    assert_eq!(count(&db, "SELECT count(DISTINCT log_sha256) FROM runs"), 4);
}

#[test]
fn another_schema_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("results.duckdb");
    drop(Database::open(&path).unwrap());
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE schema_version SET version = 99", [])
            .unwrap();
    }
    let error = Database::open(&path).err().unwrap();
    assert!(error.to_string().contains("version 99"), "{error}");
}

/// Collects rows.
struct Rows(Vec<Row>);

impl Sink for Rows {
    fn row(&mut self, row: &Row) {
        self.0.push(row.clone());
    }
}

#[test]
fn the_sink_sees_every_entry_and_changes_nothing() {
    let scenario = Scenario::from_yaml(SMALL).unwrap();
    let variant = &scenario.variants()[0];
    let plain = run(&scenario, variant, 5, RunOptions { keep_log: true }).unwrap();
    let mut rows = Rows(Vec::new());
    let sunk = run_with_sink(
        &scenario,
        variant,
        5,
        RunOptions::default(),
        Some(&mut rows),
    )
    .unwrap();
    assert_eq!(plain.digest, sunk.digest);
    assert_eq!(rows.0.len(), plain.log.len());
    for (seq, (row, entry)) in rows.0.iter().zip(&plain.log).enumerate() {
        assert_eq!(row.seq, seq as u64);
        assert_eq!((row.time, row.peer), (entry.time, entry.peer));
    }
    // Decision rows carry the other peer's index and a readable detail.
    let request = rows.0.iter().find(|r| r.kind == "requested").unwrap();
    assert!(request.other.is_some() && request.start.is_some() && request.count.is_some());
    assert!(request.reason.is_some());
    assert!(
        request
            .detail
            .as_deref()
            .unwrap()
            .contains("requested chunks")
    );
    let sent = rows.0.iter().find(|r| r.kind == "sent").unwrap();
    assert!(sent.message.is_some() && sent.bytes.is_some() && sent.detail.is_none());
}

/// The scenario as a variant runs it gives every class the same settings.
#[test]
fn resolved_scenarios_keep_their_settings() {
    let scenario = Scenario::from_yaml(SMALL).unwrap();
    for variant in scenario.variants() {
        let resolved = scenario.resolve(&variant);
        assert_eq!(resolved.variants().len(), 1);
        let plain = &resolved.variants()[0];
        for class in 0..scenario.peers.len() {
            assert_eq!(
                scenario.config(class, &variant).unwrap(),
                resolved.config(class, plain).unwrap()
            );
        }
    }
}

/// The 1,000-peer swarm (M2's done criterion) completes, replays exactly, and its results land
/// in DuckDB. Slow unoptimized, so CI runs it in release (`cargo test --release -- --ignored`).
#[test]
#[ignore = "slow without optimizations; CI runs it in release"]
fn a_thousand_peer_swarm_replays_into_duckdb() {
    let path = example_path("swarm-1000.yaml");
    let (scenario, text) = Scenario::load_text(&path).unwrap();
    assert_eq!(scenario.peer_count(), 1000);
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(&dir.path().join("swarm.duckdb")).unwrap();
    let file = ScenarioFile::new(&path, &text);
    let (_, results) = record(&db, &scenario, Some(&file), true);
    let [result] = results.as_slice() else {
        panic!("one run");
    };

    let downloaders = result.downloaders();
    assert_eq!(downloaders, 996);
    assert_eq!(result.completion_times().len(), downloaders);
    let again = run(&scenario, &scenario.variants()[0], 1, RunOptions::default()).unwrap();
    assert_eq!(
        again.digest, result.digest,
        "the same seed replays the same log"
    );

    assert_eq!(count(&db, "SELECT completed FROM runs"), downloaders as i64);
    assert_eq!(
        count(&db, "SELECT count(*) FROM events"),
        result.log_len as i64
    );
    assert_eq!(
        count(
            &db,
            "SELECT count(*) FROM peers WHERE completed_s IS NOT NULL"
        ),
        downloaders as i64
    );
    // Every downloader received the whole bundle as verified payload.
    let short = count(
        &db,
        "SELECT count(*) FROM peers WHERE starts = 'empty' AND downloaded < 8 * 1024 * 1024",
    );
    assert_eq!(short, 0);
}
