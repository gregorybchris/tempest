//! `tempest-sim web`: the simulation results UI and the API it reads, over the DuckDB results
//! database (D-075). The UI's files are embedded at build time when they were built (D-201).
//!
//! Each request opens the database read-only and closes it again, so `tempest-sim run` can add
//! experiments while the UI is open; while a run is recording, requests answer that the
//! database is busy.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use axum::Router;
use axum::extract::{Path, RawQuery, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;
use tempest_sim::browse::{self, Baselines, BrowseError};
use tempest_sim::results::duckdb::Connection;
use tempest_sim::results::{self, Database};
use tempest_sim_api::query::{CompareQuery, ExperimentQuery, QueryError, RunQuery};
use tempest_sim_api::{ApiError, PREFIX};

use tempest_assets::{self as assets, show};

/// The UI's files, by their path in the page, as `crates/sim-web/dist` held them at build time.
static ASSETS: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/sim_web_assets.rs"));

/// What `/` shows when the binary was built without the UI.
const NOT_BUILT: &str = r#"<!DOCTYPE html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Tempest simulations</title>
<style>
  body { background: #020b05; color: #8af2a4; font: 15px/1.6 ui-monospace, monospace; padding: 8vh 6vw; }
  code { color: #d2ffe0; }
</style>
<h1>Tempest simulations</h1>
<p>The results API is running, but this <code>tempest</code> binary was built without the results UI.</p>
<p>Build the UI with <code>trunk build</code> in <code>crates/sim-web</code>,
then build <code>tempest</code> again. Meanwhile, the API answers under <code>/v1</code>, as in
<code>/v1/experiments</code>.</p>
</html>
"#;

#[derive(Debug, clap::Args)]
pub struct SimWebArgs {
    /// The DuckDB results database that `tempest-sim run` writes.
    #[arg(long, value_name = "PATH", default_value = "tempest-sim.duckdb")]
    db: PathBuf,
    /// The directory of golden baselines to check experiments against.
    #[arg(long, value_name = "DIR", default_value = "crates/sim/baselines")]
    baselines: PathBuf,
    /// The address to serve on. Port 0 picks a free one.
    #[arg(long, value_name = "ADDRESS", default_value = "127.0.0.1:7340")]
    listen: SocketAddr,
    /// Print the UI's address rather than opening it in a browser.
    #[arg(long)]
    no_open: bool,
}

/// `tempest-sim web`: serves the results UI and its API until Ctrl-C.
pub fn web(args: SimWebArgs) -> Result<()> {
    if ASSETS.is_empty() {
        eprintln!(
            "note: this binary was built without the results UI; the page says how to add it"
        );
    }
    if !args.db.exists() {
        eprintln!(
            "note: there is no results database at {} yet; `tempest-sim run` creates it",
            args.db.display()
        );
    }
    if !args.listen.ip().is_loopback() {
        eprintln!(
            "warning: listening on {}, which other machines can reach; the results UI has no \
             authentication",
            args.listen.ip()
        );
    }
    let sources = Sources {
        db: args.db,
        baselines: Baselines::new(Some(args.baselines)),
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(args.listen)
            .await
            .with_context(|| format!("cannot listen on {}", args.listen))?;
        let address = listener.local_addr()?;
        show(&format!("http://{address}/"), !args.no_open);
        axum::serve(listener, router(sources))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("serving the results UI")
    })
}

/// Where the answers come from.
#[derive(Debug, Clone)]
pub struct Sources {
    /// The results database.
    pub db: PathBuf,
    /// The golden baselines.
    pub baselines: Baselines,
}

/// The API's routes and the UI's.
pub fn router(sources: Sources) -> Router {
    let api = Router::new()
        .route("/experiments", get(experiments))
        .route("/experiments/{id}", get(experiment))
        .route("/experiments/{id}/baseline", get(baseline))
        .route("/runs", get(runs))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/peers", get(peers))
        .route("/runs/{id}/series", get(series))
        .route("/compare", get(compare))
        .route("/compare/series", get(compare_series))
        .route("/baselines", get(baselines))
        .fallback(|| async { error(StatusCode::NOT_FOUND, "not_found", "there is no such route") })
        .with_state(Arc::new(sources));
    Router::new()
        .nest(PREFIX, api)
        .merge(assets::router(ASSETS, NOT_BUILT))
}

type Shared = State<Arc<Sources>>;

/// `value` as a JSON response with `status`.
fn json(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => (status, [(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// An error answer: its status, and the API's error in the body.
struct Failure {
    status: StatusCode,
    error: ApiError,
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        json(self.status, &self.error)
    }
}

fn error(status: StatusCode, kind: &str, message: impl Into<String>) -> Failure {
    Failure {
        status,
        error: ApiError::new(kind, message),
    }
}

fn bad_query(e: QueryError) -> Response {
    error(StatusCode::BAD_REQUEST, "bad_query", e.to_string()).into_response()
}

fn id(text: &str) -> Result<i64, Failure> {
    text.parse().map_err(|_| {
        error(
            StatusCode::BAD_REQUEST,
            "bad_id",
            format!("{text:?} isn't an ID"),
        )
    })
}

/// Opens the database, runs `read` on a blocking thread and answers with its value as JSON,
/// or 404 if it is `None`.
async fn answer<T: Serialize + Send + 'static>(
    sources: Arc<Sources>,
    missing: &'static str,
    read: impl FnOnce(&Connection, &Baselines) -> Result<Option<T>, BrowseError> + Send + 'static,
) -> Response {
    let task = tokio::task::spawn_blocking(move || {
        let db = match Database::open_read_only(&sources.db) {
            Ok(db) => db,
            Err(e) => return Err(unavailable(&sources, e)),
        };
        let conn = db.connection();
        read(&conn, &sources.baselines).map_err(|e| match e {
            BrowseError::NotFound(why) => error(StatusCode::NOT_FOUND, "not_found", why),
            other => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                other.to_string(),
            ),
        })
    });
    match task.await {
        Ok(Ok(Some(value))) => json(StatusCode::OK, &value),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "not_found", missing).into_response(),
        Ok(Err(failure)) => failure.into_response(),
        Err(e) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", e.to_string()).into_response()
        }
    }
}

/// Why the database can't be read, in words that say what to do.
fn unavailable(sources: &Sources, e: results::Error) -> Failure {
    let path = sources.db.display();
    if !sources.db.exists() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_database",
            format!("there is no results database at {path} yet; `tempest-sim run` records one"),
        );
    }
    match e {
        results::Error::Schema { .. } => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "schema",
            format!("{path}: {e}"),
        ),
        // DuckDB lets one process write a file or many read it, so a recording run locks it.
        results::Error::Database(e) if e.to_string().contains("lock") => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            format!(
                "{path} is busy, probably recording a `tempest-sim run`; try again when it ends"
            ),
        ),
        other => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            format!("cannot read {path}: {other}"),
        ),
    }
}

async fn experiments(State(sources): Shared, RawQuery(query): RawQuery) -> Response {
    let query = match ExperimentQuery::parse(&query.unwrap_or_default()) {
        Ok(query) => query,
        Err(e) => return bad_query(e),
    };
    answer(sources, "", move |conn, baselines| {
        browse::experiments(conn, &query, baselines).map(Some)
    })
    .await
}

async fn experiment(State(sources): Shared, Path(text): Path<String>) -> Response {
    let id = match id(&text) {
        Ok(id) => id,
        Err(failure) => return failure.into_response(),
    };
    answer(
        sources,
        "there is no such experiment",
        move |conn, baselines| browse::experiment(conn, id, baselines),
    )
    .await
}

async fn baseline(State(sources): Shared, Path(text): Path<String>) -> Response {
    let id = match id(&text) {
        Ok(id) => id,
        Err(failure) => return failure.into_response(),
    };
    answer(
        sources,
        "no golden baseline has this experiment's scenario name",
        move |conn, baselines| browse::baseline_report(conn, id, baselines),
    )
    .await
}

async fn runs(State(sources): Shared, RawQuery(query): RawQuery) -> Response {
    let query = match RunQuery::parse(&query.unwrap_or_default()) {
        Ok(query) => query,
        Err(e) => return bad_query(e),
    };
    answer(sources, "", move |conn, _| {
        browse::runs(conn, &query).map(Some)
    })
    .await
}

async fn run(State(sources): Shared, Path(text): Path<String>) -> Response {
    let id = match id(&text) {
        Ok(id) => id,
        Err(failure) => return failure.into_response(),
    };
    answer(sources, "there is no such run", move |conn, _| {
        browse::run(conn, id)
    })
    .await
}

async fn peers(State(sources): Shared, Path(text): Path<String>) -> Response {
    let id = match id(&text) {
        Ok(id) => id,
        Err(failure) => return failure.into_response(),
    };
    answer(sources, "there is no such run", move |conn, _| {
        browse::peers(conn, id)
    })
    .await
}

async fn series(State(sources): Shared, Path(text): Path<String>) -> Response {
    let id = match id(&text) {
        Ok(id) => id,
        Err(failure) => return failure.into_response(),
    };
    answer(sources, "there is no such run", move |conn, _| {
        browse::series(conn, id)
    })
    .await
}

async fn compare(State(sources): Shared, RawQuery(query): RawQuery) -> Response {
    let query = match CompareQuery::parse(&query.unwrap_or_default()) {
        Ok(query) => query,
        Err(e) => return bad_query(e),
    };
    answer(sources, "", move |conn, _| {
        browse::compare(conn, &query).map(Some)
    })
    .await
}

async fn compare_series(State(sources): Shared, RawQuery(query): RawQuery) -> Response {
    let query = match CompareQuery::parse(&query.unwrap_or_default()) {
        Ok(query) => query,
        Err(e) => return bad_query(e),
    };
    answer(sources, "", move |conn, _| {
        browse::compare_series(conn, &query).map(Some)
    })
    .await
}

async fn baselines(State(sources): Shared) -> Response {
    answer(sources, "", move |conn, baselines| {
        browse::baselines(conn, baselines).map(Some)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use axum::body::Body;
    use axum::http::Request;
    use serde_json::Value;
    use tempest_sim::experiment::{run_all, seeds};
    use tempest_sim::{RunOptions, Scenario};
    use tower::ServiceExt as _;

    use super::*;

    const SCENARIO: &str = "
name: pair
seeds: 3
bundle: { files: [{ size: 256KiB }] }
peers:
  - { class: origin, starts: complete, bandwidth: { up: 20Mbps, down: 100Mbps } }
  - { class: home, count: 2, bandwidth: { up: 5Mbps, down: 50Mbps } }
experiment:
  vary:
    strategies.upload: [bittorrent, cooperative]
";

    /// A results database with one experiment of two variants and three seeds, in `dir`.
    fn fixture(dir: &std::path::Path) -> Sources {
        let path = dir.join("results.duckdb");
        let db = Database::open(&path).unwrap();
        let scenario = Scenario::from_yaml(SCENARIO).unwrap();
        let seeds = seeds(&scenario);
        let recorder = db.record(&scenario, None, &seeds, true).unwrap();
        let threads = NonZeroUsize::new(2).unwrap();
        run_all(&scenario, &seeds, threads, RunOptions::default(), &recorder).unwrap();
        recorder.finish().unwrap();
        Sources {
            db: path,
            baselines: Baselines::new(Some(dir.join("baselines"))),
        }
    }

    async fn get(router: &Router, path: &str) -> (StatusCode, Value) {
        let request = Request::get(path).body(Body::empty()).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn the_api_answers_from_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let router = router(fixture(dir.path()));

        let (status, experiments) = get(&router, "/v1/experiments?q=PAIR").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(experiments[0]["name"], "pair");
        assert_eq!(experiments[0]["runs"], 6);
        let (_, detail) = get(&router, "/v1/experiments/1").await;
        assert_eq!(detail["variants"][1]["vary"][0]["value"], "cooperative");

        let (_, runs) = get(&router, "/v1/runs?variant=1&sort=seed&order=desc&limit=2").await;
        assert_eq!(runs["total"], 3);
        assert_eq!(runs["runs"][0]["seed"], 3);
        assert_eq!(runs["runs"].as_array().unwrap().len(), 2);
        let (_, run) = get(&router, "/v1/runs/1").await;
        assert_eq!(run["run"]["experiment_name"], "pair");
        let (_, peers) = get(&router, "/v1/runs/1/peers").await;
        assert_eq!(peers.as_array().unwrap().len(), 3);
        let (_, series) = get(&router, "/v1/runs/1/series").await;
        assert!(!series["throughput"].as_array().unwrap().is_empty());

        let (status, compared) = get(&router, "/v1/compare?variants=1:0,1:1&reference=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(compared["reference"], 1);
        assert_eq!(
            compared["metrics"][0]["cells"][1]["difference"],
            Value::Null
        );
        let (_, bands) = get(&router, "/v1/compare/series?variants=1:0,1:1").await;
        assert_eq!(bands["series"][0]["key"], "completed");
        let (status, baselines) = get(&router, "/v1/baselines").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(baselines, Value::Array(Vec::new()), "no baseline directory");
    }

    #[tokio::test]
    async fn errors_say_what_is_wrong_in_json() {
        let dir = tempfile::tempdir().unwrap();
        let router = router(fixture(dir.path()));
        for (path, status, kind) in [
            ("/v1/runs?sort=bogus", StatusCode::BAD_REQUEST, "bad_query"),
            ("/v1/compare", StatusCode::BAD_REQUEST, "bad_query"),
            ("/v1/runs/x", StatusCode::BAD_REQUEST, "bad_id"),
            ("/v1/runs/42", StatusCode::NOT_FOUND, "not_found"),
            (
                "/v1/experiments/1/baseline",
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            (
                "/v1/compare?variants=1:9",
                StatusCode::NOT_FOUND,
                "not_found",
            ),
            ("/v1/nothing", StatusCode::NOT_FOUND, "not_found"),
        ] {
            let (got, body) = get(&router, path).await;
            assert_eq!(
                (got, body["error"].as_str()),
                (status, Some(kind)),
                "{path}"
            );
            assert!(!body["message"].as_str().unwrap().is_empty());
        }

        let missing = router_for(dir.path().join("none.duckdb"));
        let (status, body) = get(&missing, "/v1/experiments").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], "no_database");
    }

    fn router_for(db: PathBuf) -> Router {
        router(Sources {
            db,
            baselines: Baselines::default(),
        })
    }

    #[tokio::test]
    async fn the_page_is_served_beside_the_api() {
        let router = router_for(PathBuf::from("unused.duckdb"));
        let request = Request::get("/").body(Body::empty()).unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("<html"));
    }
}
