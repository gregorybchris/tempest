//! The results database (D-075): experiments, their runs, each run's headline metrics and
//! peers, and full event logs, in one local DuckDB file.
//!
//! [`SCHEMA`] defines the tables. Each experiment adds rows with fresh IDs, so one file can
//! collect many experiments. A [`Recorder`] watches an experiment's runs (it is an
//! [`Observer`]) and writes each run's log as it is made, in batches through DuckDB's
//! appender, so a run's log never has to fit in memory.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use duckdb::{Connection, params};

use crate::engine::RunResult;
use crate::experiment::Observer;
use crate::log::{Row, Sink};
use crate::scenario::Scenario;

/// The git commit the simulator was built from, if it was built from a git checkout.
pub const GIT_COMMIT: Option<&str> = option_env!("TEMPEST_GIT_COMMIT");

/// The schema version this build writes. A database with another version is refused.
pub const SCHEMA_VERSION: i32 = 2;

/// The tables. Times in the summary tables are seconds of simulated time from the start of a
/// run; event times are nanoseconds. A run's resolved scenario is its variant's.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);

-- One per `tempest-sim run`: a scenario and the variants and seeds run in it.
CREATE TABLE IF NOT EXISTS experiments (
    experiment_id BIGINT PRIMARY KEY,
    name VARCHAR NOT NULL,
    description VARCHAR NOT NULL,
    scenario JSON NOT NULL,          -- the scenario as parsed, with its experiment
    scenario_file VARCHAR,           -- the file it was read from
    scenario_sha256 VARCHAR,         -- the SHA-256 of that file's bytes
    git_commit VARCHAR,              -- the commit the simulator was built from
    created_at TIMESTAMP NOT NULL DEFAULT current_timestamp
);

-- One per combination of the experiment's values.
CREATE TABLE IF NOT EXISTS variants (
    experiment_id BIGINT NOT NULL,
    variant INTEGER NOT NULL,        -- its index in the experiment
    name VARCHAR NOT NULL,
    vary JSON NOT NULL,              -- the value of each varied setting
    scenario JSON NOT NULL,          -- the scenario as this variant runs it
    PRIMARY KEY (experiment_id, variant)
);

-- One per variant and seed, with the run's headline metrics.
CREATE TABLE IF NOT EXISTS runs (
    run_id BIGINT PRIMARY KEY,
    experiment_id BIGINT NOT NULL,
    variant INTEGER NOT NULL,
    seed UBIGINT NOT NULL,
    end_s DOUBLE NOT NULL,           -- when the run ended
    events UBIGINT NOT NULL,         -- events the driver handled
    log_entries UBIGINT NOT NULL,
    log_sha256 VARCHAR NOT NULL,     -- the digest of the event log; the same seed gives the same
    peers INTEGER NOT NULL,
    downloaders INTEGER NOT NULL,    -- peers that started without the bundle
    completed INTEGER NOT NULL,      -- downloaders that completed
    completion_p50_s DOUBLE,         -- completion times, from arrival
    completion_p90_s DOUBLE,
    completion_p99_s DOUBLE,
    first_byte_p50_s DOUBLE,         -- from arrival to the first verified payload
    first_peer_p50_s DOUBLE,         -- from arrival to the first open connection
    upload_utilization DOUBLE,       -- payload uploaded / upload capacity while present
    fairness DOUBLE,                 -- Jain's index of upload/download ratios
    rarest_copies DOUBLE,            -- mean over samples of present peers holding the rarest chunk
    payload_bytes UBIGINT NOT NULL,  -- payload in the Data sent
    overhead_bytes UBIGINT NOT NULL, -- everything else sent
    duplicate_bytes UBIGINT NOT NULL -- payload received more than once, in endgame
);

-- One per peer per run: what it did.
CREATE TABLE IF NOT EXISTS peers (
    run_id BIGINT NOT NULL,
    peer INTEGER NOT NULL,           -- its index in the run
    class VARCHAR NOT NULL,
    starts VARCHAR NOT NULL,         -- empty or complete
    behavior VARCHAR NOT NULL,
    upload_capacity DOUBLE,          -- bytes per second; NULL if unlimited
    download_capacity DOUBLE,
    arrived_s DOUBLE,
    first_peer_s DOUBLE,
    first_byte_s DOUBLE,
    completed_s DOUBLE,
    left_s DOUBLE,
    downloaded UBIGINT NOT NULL,     -- verified payload bytes received
    uploaded UBIGINT NOT NULL,       -- payload bytes sent
    duplicate UBIGINT NOT NULL,      -- payload bytes received of chunks it already had
    bad UBIGINT NOT NULL,            -- payload bytes received that failed verification
    intact BOOLEAN,                  -- with real payloads: whether it holds the bundle exactly
    PRIMARY KEY (run_id, peer)
);

-- One per sample per run: how many present peers held each chunk.
CREATE TABLE IF NOT EXISTS availability (
    run_id BIGINT NOT NULL,
    time_s DOUBLE NOT NULL,
    peers INTEGER NOT NULL,          -- peers present
    rarest INTEGER NOT NULL,         -- present peers holding the rarest chunk
    mean DOUBLE NOT NULL,            -- present peers holding a chunk, on average
    PRIMARY KEY (run_id, time_s)
);

-- One per kind of message per run.
CREATE TABLE IF NOT EXISTS traffic (
    run_id BIGINT NOT NULL,
    message VARCHAR NOT NULL,
    count UBIGINT NOT NULL,
    bytes UBIGINT NOT NULL,
    overhead_bytes UBIGINT NOT NULL, -- for Data, framing and proofs; otherwise all of it
    PRIMARY KEY (run_id, message)
);

-- Every arrival, departure, message sent and decision event, in order (the log's rows).
CREATE TABLE IF NOT EXISTS events (
    run_id BIGINT NOT NULL,
    seq UBIGINT NOT NULL,
    time_ns UBIGINT NOT NULL,
    peer INTEGER NOT NULL,
    kind VARCHAR NOT NULL,
    other INTEGER,
    conn UBIGINT,
    message VARCHAR,
    start_chunk UINTEGER,
    count UINTEGER,
    bytes UBIGINT,
    reason VARCHAR,
    detail VARCHAR
);
";

/// How many log rows a run buffers before appending them.
const BATCH: usize = 16_384;

/// Why results couldn't be written or read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// DuckDB failed.
    #[error("results database: {0}")]
    Database(#[from] DatabaseError),
    /// The database was written by a build with another schema.
    #[error("the results database has schema version {found}; this build writes {SCHEMA_VERSION}")]
    Schema {
        /// Its version.
        found: i32,
    },
}

impl From<duckdb::Error> for Error {
    fn from(error: duckdb::Error) -> Self {
        Self::Database(error.into())
    }
}

/// An error from DuckDB, opaque so that DuckDB stays out of this crate's API.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DatabaseError(#[from] duckdb::Error);

impl DatabaseError {
    /// Whether the database is locked, as it is while another process writes it: DuckDB lets
    /// one process write a file or many read it.
    pub fn is_locked(&self) -> bool {
        self.0.to_string().contains("lock")
    }
}

/// A results database.
pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens the database at `path`, creating it and its tables if needed.
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::init(Connection::open(path)?)
    }

    /// Opens the database at `path` to read it, without creating or changing anything. Other
    /// processes may read it at the same time, but none may write it until this is dropped.
    pub fn open_read_only(path: &Path) -> Result<Self, Error> {
        let config = duckdb::Config::default().access_mode(duckdb::AccessMode::ReadOnly)?;
        let conn = Connection::open_with_flags(path, config)?;
        let found: Option<i32> =
            conn.query_row("SELECT max(version) FROM schema_version", [], |row| {
                row.get(0)
            })?;
        match found {
            Some(SCHEMA_VERSION) => Ok(Self {
                conn: Mutex::new(conn),
            }),
            found => Err(Error::Schema {
                found: found.unwrap_or(0),
            }),
        }
    }

    /// A database in memory, for tests.
    pub fn open_in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, Error> {
        conn.execute_batch(SCHEMA)?;
        let found: Option<i32> =
            conn.query_row("SELECT max(version) FROM schema_version", [], |row| {
                row.get(0)
            })?;
        match found {
            None => {
                conn.execute("INSERT INTO schema_version VALUES (?)", [SCHEMA_VERSION])?;
            }
            Some(SCHEMA_VERSION) => {}
            Some(found) => return Err(Error::Schema { found }),
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// The connection, for queries.
    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The DuckDB connection, for tests that check the tables with SQL. Only the `testing`
    /// feature has it, so DuckDB stays out of this crate's API.
    #[cfg(feature = "testing")]
    pub fn connection(&self) -> MutexGuard<'_, Connection> {
        self.conn()
    }

    /// Starts recording an experiment: adds it and its variants, and returns the recorder to
    /// run it with. `file` is the scenario file it came from; `events` says whether to keep
    /// every run's event log.
    pub fn record<'d>(
        &'d self,
        scenario: &'d Scenario,
        file: Option<&ScenarioFile>,
        seeds: &[u64],
        events: bool,
    ) -> Result<Recorder<'d>, Error> {
        let conn = self.conn();
        let next = |table: &str, column: &str| -> Result<i64, Error> {
            let sql = format!("SELECT coalesce(max({column}), 0) + 1 FROM {table}");
            Ok(conn.query_row(&sql, [], |row| row.get(0))?)
        };
        let experiment = next("experiments", "experiment_id")?;
        let first_run = next("runs", "run_id")?;
        conn.execute(
            "INSERT INTO experiments (experiment_id, name, description, scenario, scenario_file,
                scenario_sha256, git_commit)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                experiment,
                scenario.name,
                scenario.description,
                json(scenario),
                file.map(|f| f.path.as_str()),
                file.map(|f| f.sha256.as_str()),
                GIT_COMMIT,
            ],
        )?;
        for variant in scenario.variants() {
            conn.execute(
                "INSERT INTO variants VALUES (?, ?, ?, ?, ?)",
                params![
                    experiment,
                    variant.index as i64,
                    variant.name(),
                    json(&variant.values),
                    json(&scenario.resolve(&variant)),
                ],
            )?;
        }
        Ok(Recorder {
            db: self,
            scenario,
            experiment,
            first_run,
            seeds: seeds.to_vec(),
            events,
            error: Mutex::new(None),
        })
    }
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("scenarios serialize")
}

/// A scenario file: where it is and the hash of its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioFile {
    /// The path it was read from.
    pub path: String,
    /// The SHA-256 of its bytes, in hex.
    pub sha256: String,
}

impl ScenarioFile {
    /// The file at `path` with contents `text`.
    pub fn new(path: &Path, text: &str) -> Self {
        Self {
            path: path.display().to_string(),
            sha256: Scenario::file_sha256(text),
        }
    }
}

/// Writes an experiment's runs to a [`Database`] as they finish.
pub struct Recorder<'d> {
    db: &'d Database,
    scenario: &'d Scenario,
    experiment: i64,
    first_run: i64,
    seeds: Vec<u64>,
    events: bool,
    /// The first error, which ends recording.
    error: Mutex<Option<duckdb::Error>>,
}

impl Recorder<'_> {
    /// The experiment's ID.
    pub fn experiment(&self) -> i64 {
        self.experiment
    }

    /// The ID of the run of `variant` with `seed`: fixed by their positions, so that it
    /// doesn't depend on which run finishes first.
    pub fn run_id(&self, variant: usize, seed: u64) -> i64 {
        let position = self
            .seeds
            .iter()
            .position(|&s| s == seed)
            .expect("the recorder knows every seed");
        self.first_run + (variant * self.seeds.len() + position) as i64
    }

    /// Ends recording: the experiment's ID, or the first error.
    pub fn finish(self) -> Result<i64, Error> {
        match self.error.into_inner().unwrap_or_else(|e| e.into_inner()) {
            Some(error) => Err(error.into()),
            None => Ok(self.experiment),
        }
    }

    /// Runs `write` with the connection unless an earlier write failed, and keeps its error.
    fn write(&self, write: impl FnOnce(&Connection) -> duckdb::Result<()>) {
        let mut error = self.error.lock().unwrap_or_else(|e| e.into_inner());
        if error.is_none()
            && let Err(e) = write(&self.db.conn())
        {
            *error = Some(e);
        }
    }

    fn write_run(&self, conn: &Connection, result: &RunResult) -> duckdb::Result<()> {
        let run = self.run_id(result.variant, result.seed);
        let metrics = result.metrics(self.scenario);
        let secs = |d: Option<std::time::Duration>| d.map(|d| d.as_secs_f64());
        conn.execute(
            "INSERT INTO runs VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                run,
                self.experiment,
                result.variant as i64,
                result.seed,
                result.end.since_epoch().as_secs_f64(),
                result.events,
                result.log_len,
                result.digest,
                result.peers.len() as i64,
                metrics.downloaders as i64,
                metrics.completed as i64,
                secs(metrics.completion_p50),
                secs(metrics.completion_p90),
                secs(metrics.completion_p99),
                secs(metrics.first_byte_p50),
                secs(metrics.first_peer_p50),
                metrics.upload_utilization,
                metrics.fairness,
                metrics.rarest_copies,
                result.payload_sent,
                metrics.overhead,
                metrics.duplicate,
            ],
        )?;

        let mut peers = conn.appender("peers")?;
        let classes = self.scenario.classes(result.variant);
        for (index, peer) in result.peers.iter().enumerate() {
            let class = &classes[peer.class];
            let at = |t: Option<tempest_core::Time>| t.map(|t| t.since_epoch().as_secs_f64());
            let finite = |r: f64| r.is_finite().then_some(r);
            peers.append_row(params![
                run,
                index as i64,
                class.class,
                json_name(&peer.starts),
                json_name(&class.behavior),
                finite(class.bandwidth.up.get()),
                finite(class.bandwidth.down.get()),
                at(peer.arrived),
                at(peer.first_peer),
                at(peer.first_byte),
                at(peer.completed),
                at(peer.left),
                peer.downloaded,
                peer.uploaded,
                peer.duplicate,
                peer.bad,
                peer.intact,
            ])?;
        }
        peers.flush()?;

        let mut availability = conn.appender("availability")?;
        for sample in &result.availability {
            availability.append_row(params![
                run,
                sample.time.since_epoch().as_secs_f64(),
                sample.peers,
                sample.rarest,
                sample.mean,
            ])?;
        }
        availability.flush()?;

        let overhead = result.overhead_by_message();
        let mut traffic = conn.appender("traffic")?;
        for (&message, t) in &result.traffic {
            traffic.append_row(params![run, message, t.count, t.bytes, overhead[message]])?;
        }
        traffic.flush()
    }
}

/// The name serde gives a unit variant, such as `free-rider`.
fn json_name(value: &impl serde::Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        other => panic!("not a unit variant: {other:?}"),
    }
}

impl Observer for Recorder<'_> {
    fn sink(&self, variant: usize, seed: u64) -> Option<Box<dyn Sink + '_>> {
        self.events.then(|| {
            Box::new(EventWriter {
                recorder: self,
                run: self.run_id(variant, seed),
                rows: Vec::with_capacity(BATCH),
            }) as Box<dyn Sink + '_>
        })
    }

    fn finished(&self, result: &RunResult) {
        self.write(|conn| self.write_run(conn, result));
    }
}

/// Appends a run's log rows to the events table in batches.
struct EventWriter<'r> {
    recorder: &'r Recorder<'r>,
    run: i64,
    rows: Vec<Row>,
}

impl EventWriter<'_> {
    fn flush(&mut self) {
        let (run, rows) = (self.run, &self.rows);
        self.recorder.write(|conn| {
            let mut events = conn.appender("events")?;
            for row in rows {
                events.append_row(params![
                    run,
                    row.seq,
                    row.time.as_nanos(),
                    row.peer,
                    row.kind,
                    row.other,
                    row.conn,
                    row.message,
                    row.start,
                    row.count,
                    row.bytes,
                    row.reason,
                    row.detail,
                ])?;
            }
            events.flush()
        });
        self.rows.clear();
    }
}

impl Sink for EventWriter<'_> {
    fn row(&mut self, row: &Row) {
        self.rows.push(row.clone());
        if self.rows.len() == BATCH {
            self.flush();
        }
    }
}

impl Drop for EventWriter<'_> {
    fn drop(&mut self) {
        if !self.rows.is_empty() {
            self.flush();
        }
    }
}
