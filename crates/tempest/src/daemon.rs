//! `tempest daemon`: run the engine and serve its API until stopped.

use anyhow::{Context, Result, anyhow};
use serde_json::Value;
use tempest_engine::api::server::{ApiServer, TOKEN_FILE};
use tempest_engine::{Engine, EngineConfig, EngineError};
use tempest_protocol::RelayIdentity;

use crate::engine::EngineArgs;

#[derive(Debug, clap::Args)]
pub struct DaemonArgs {
    #[command(flatten)]
    engine: EngineArgs,
    /// Override a setting of the config file for this run, by dotted key, such as
    /// `--set api.port=4800`. The value is JSON, or else a string. Repeat for more.
    #[arg(long, value_name = "KEY=VALUE", value_parser = parse_setting)]
    set: Vec<(String, Value)>,
    /// A relay to pick a home relay from for this run, besides the config's `relays.known`, as
    /// `<key>@host[:port]`. Repeat for more.
    #[arg(long, value_name = "RELAY")]
    relay: Vec<RelayIdentity>,
}

pub fn parse_setting(text: &str) -> Result<(String, Value), String> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| "expected KEY=VALUE".to_owned())?;
    let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
    Ok((key.trim().to_owned(), value))
}

pub fn daemon(args: DaemonArgs) -> Result<()> {
    let mut config = args.engine.load()?;
    for (key, value) in args.set {
        config
            .set(&key, value)
            .with_context(|| format!("--set {key}"))?;
    }
    config.relays.known.extend(args.relay);
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(run(config, |_| {}))
}

/// Runs the engine and serves its API and the web UI until Ctrl-C, calling `ready` with the
/// API's URL once it serves.
pub async fn run(config: EngineConfig, ready: impl FnOnce(&str)) -> Result<()> {
    let data_dir = config.data_dir()?;
    {
        let file_config = config.clone();
        let engine = match Engine::open(config).await {
            Err(EngineError::Busy(dir)) => {
                return Err(anyhow!(
                    "another Tempest engine is using {}; is a daemon already running?",
                    dir.display()
                ));
            }
            opened => opened.context("cannot start the engine")?,
        };
        // Settings changed through the API since the file was written apply too.
        let view = engine.settings().await?;
        for (key, file) in file_config.overridden_by(&view.changed) {
            eprintln!(
                "setting   {key} is {file} in the config, overridden by {} set through the API; \
                 reset it in a UI or with DELETE /v1/settings/{key} to use the config's",
                view.changed[&key]
            );
        }
        let api = view.settings.api;
        let server = match ApiServer::bind(&api, &data_dir).await {
            Ok(server) => server,
            Err(error) => {
                engine.shutdown().await;
                return Err(error).context("cannot serve the API");
            }
        };
        eprintln!("peer      {}", engine.peer_id());
        eprintln!("data      {}", data_dir.display());
        eprintln!("api       {}", server.url());
        if let Some(fingerprint) = server.fingerprint() {
            eprintln!("cert      SHA-256 {fingerprint}");
            eprintln!("token     in {}", data_dir.join(TOKEN_FILE).display());
        }
        ready(server.url());
        eprintln!("running; press Ctrl-C to stop");
        let stopping = engine.clone();
        let shutdown = async move {
            interrupted().await;
            eprintln!("stopping");
            // Stops every transfer, tells trackers and peers, and saves where each got to.
            // Event streams end with it, so the API can finish.
            stopping.shutdown().await;
        };
        server
            .with_ui(crate::web::router())
            .serve(engine, shutdown)
            .await?;
        eprintln!("stopped");
        Ok(())
    }
}

/// Waits for Ctrl-C, or on Unix for SIGTERM too.
async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
