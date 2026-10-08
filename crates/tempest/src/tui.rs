//! `tempest tui`: the terminal UI, on the daemon if one answers, or else on an engine started
//! here that stops when the TUI quits, or hands its active transfers to a daemon if asked to
//! (D-228).

use anyhow::{Context, Result, anyhow};
use tempest_engine::api::client::ApiClient;
use tempest_engine::api::server::ApiServer;
use tempest_engine::{Engine, EngineError};
use tempest_tui::app::{Exit, Origin};
use tokio::sync::oneshot;

use crate::engine::{EngineArgs, Target};
use crate::handoff::Daemon;

#[derive(Debug, clap::Args)]
pub struct TuiArgs {
    #[command(flatten)]
    engine: EngineArgs,
}

pub fn tui(args: TuiArgs) -> Result<()> {
    let config = args.engine.load()?;
    let data_dir = config.data_dir()?;
    let daemon = Daemon {
        engine: args.engine.clone(),
        set: Vec::new(),
    };
    let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
    runtime.block_on(async {
        if let Some(client) = Target::daemon(&config).await? {
            tempest_tui::run(client, Origin::Daemon).await?;
            return Ok(());
        }

        // No daemon: run the engine here, serving its API like a daemon would, so the TUI
        // takes one path either way and the CLI can reach the engine while the TUI runs.
        let engine = match Engine::open(config.clone()).await {
            Err(EngineError::Busy(dir)) => return Err(crate::engine::busy(&dir)),
            opened => opened.context("cannot start the engine")?,
        };
        let api = engine.settings().await?.settings.api;
        let server = match ApiServer::bind(&api, &data_dir).await {
            Ok(server) => server,
            Err(error) => {
                engine.shutdown().await;
                return Err(error).context("cannot serve the engine's API");
            }
        };
        let (stop, stopped) = oneshot::channel::<()>();
        let stopping = engine.clone();
        let serving = tokio::spawn(server.with_ui(crate::web::router()).serve(
            engine.clone(),
            async move {
                let _ = stopped.await;
                stopping.shutdown().await;
            },
        ));
        let result = match ApiClient::for_data_dir(&data_dir) {
            Ok(Some(client)) => tempest_tui::run(client, Origin::Embedded)
                .await
                .map_err(anyhow::Error::from),
            Ok(None) => Err(anyhow!("the engine's API didn't record where it is")),
            Err(error) => Err(error.into()),
        };
        let handing = match &result {
            Ok(Exit::HandOff) => engine
                .list()
                .await
                .iter()
                .filter(|t| t.status.is_active())
                .count(),
            Ok(Exit::Stop) => {
                crate::handoff::stop_active(&engine).await;
                0
            }
            _ => 0,
        };
        eprintln!("stopping the engine");
        let _ = stop.send(());
        serving.await.context("the API server failed")??;
        result?;
        if handing > 0 {
            let url = daemon.start(&config).await?;
            crate::handoff::report(handing, &url);
        }
        Ok(())
    })
}
