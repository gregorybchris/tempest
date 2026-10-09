//! Finding the engine: its config, and its API if a daemon is running, or else an embedded
//! engine on the same state (D-079).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tempest_api::client::{ApiClient, ClientError};
use tempest_engine::config::default_data_dir;
use tempest_engine::{Engine, EngineConfig, EngineError, TransferInfo};
use tempest_protocol::BundleId;

/// The environment variable that names the config file.
pub const CONFIG_ENV: &str = "TEMPEST_CONFIG";
/// The environment variable that names the data directory.
pub const DATA_DIR_ENV: &str = "TEMPEST_DATA_DIR";

/// Which engine: its config file and data directory.
#[derive(Debug, Clone, clap::Args)]
pub struct EngineArgs {
    /// The engine's config file. Defaults to `config.yaml` in the data directory.
    #[arg(long, env = CONFIG_ENV, value_name = "FILE")]
    pub config: Option<PathBuf>,
    /// The engine's data directory, which holds its state and identity. Defaults to the
    /// config's `data_dir`, then Tempest's folder in the per-user data directory.
    #[arg(long, env = DATA_DIR_ENV, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
}

impl EngineArgs {
    /// Reads the config file, with the data directory given here.
    pub fn load(&self) -> Result<EngineConfig> {
        let data_dir = match &self.data_dir {
            Some(dir) => Some(std::path::absolute(dir).context("cannot find the data directory")?),
            None => None,
        };
        let path = match (&self.config, &data_dir) {
            // A config named outright must exist; only the default one may be missing.
            (Some(path), _) if !path.exists() => bail!("no config file at {}", path.display()),
            (Some(path), _) => path.clone(),
            (None, Some(dir)) => dir.join("config.yaml"),
            (None, None) => default_data_dir()
                .with_context(|| {
                    format!("cannot find the per-user data directory; set {DATA_DIR_ENV}")
                })?
                .join("config.yaml"),
        };
        let mut config = EngineConfig::load(&path)
            .with_context(|| format!("cannot read the config {}", path.display()))?;
        if data_dir.is_some() {
            config.data_dir = data_dir;
        }
        tracing::info!(
            "config loaded from {}{}",
            path.display(),
            if path.exists() {
                ""
            } else {
                " (no such file; defaults)"
            }
        );
        Ok(config)
    }
}

/// Why an engine can't open on `dir`: another one holds it but doesn't answer, as a daemon
/// that is still starting doesn't.
pub fn busy(dir: &std::path::Path) -> anyhow::Error {
    anyhow::anyhow!(
        "another Tempest engine is using {}, but its API isn't answering; if a daemon is \
         starting, try again in a moment",
        dir.display()
    )
}

/// An engine to send commands to.
pub enum Target {
    /// A daemon, through its API.
    Daemon(ApiClient),
    /// An engine opened here on the daemon's state, running no transfers.
    Held(Engine),
}

impl Target {
    /// The daemon if one answers, or else an engine opened here on its state. Must be called
    /// within a tokio runtime.
    pub async fn connect(config: EngineConfig) -> Result<Self> {
        if let Some(client) = Self::daemon(&config).await? {
            return Ok(Self::Daemon(client));
        }
        match Engine::open_held(config).await {
            Ok(engine) => Ok(Self::Held(engine)),
            Err(EngineError::Busy(dir)) => Err(busy(&dir)),
            Err(error) => Err(error.into()),
        }
    }

    /// Runs `command` against the engine `args` name, then closes an embedded one, saying
    /// that no daemon runs the transfers.
    pub fn run<T>(args: &EngineArgs, command: impl AsyncFnOnce(&Target) -> Result<T>) -> Result<T> {
        let config = args.load()?;
        let runtime = tokio::runtime::Runtime::new().context("cannot start the async runtime")?;
        runtime.block_on(async {
            let target = Self::connect(config).await?;
            let result = command(&target).await;
            if !target.is_daemon() {
                eprintln!(
                    "note: no daemon is running, so nothing is transferring; `tempest daemon` \
                     runs it"
                );
            }
            target.close().await;
            result
        })
    }

    /// The daemon's API, if one answers. Must be called within a tokio runtime.
    pub async fn daemon(config: &EngineConfig) -> Result<Option<ApiClient>> {
        let Some(client) = ApiClient::for_data_dir(&config.data_dir()?)? else {
            return Ok(None);
        };
        match client.engine().await {
            Ok(_) => Ok(Some(client)),
            // A daemon that stopped without tidying up.
            Err(ClientError::Unreachable { .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether commands reach a daemon, which runs transfers.
    pub fn is_daemon(&self) -> bool {
        matches!(self, Self::Daemon(_))
    }

    pub async fn list(&self) -> Result<Vec<TransferInfo>> {
        Ok(match self {
            Self::Daemon(client) => client.list().await?,
            Self::Held(engine) => engine.list().await,
        })
    }

    pub async fn start(&self, id: BundleId, force: bool) -> Result<TransferInfo> {
        Ok(match (self, force) {
            (Self::Daemon(client), false) => client.start(id).await?,
            (Self::Daemon(client), true) => client.force_start(id).await?,
            (Self::Held(engine), false) => engine.start(id).await?,
            (Self::Held(engine), true) => engine.force_start(id).await?,
        })
    }

    pub async fn stop(&self, id: BundleId) -> Result<TransferInfo> {
        Ok(match self {
            Self::Daemon(client) => client.stop(id).await?,
            Self::Held(engine) => engine.stop(id).await?,
        })
    }

    pub async fn remove(&self, id: BundleId, delete_files: bool) -> Result<()> {
        match self {
            Self::Daemon(client) => client.remove(id, delete_files).await?,
            Self::Held(engine) => engine.remove(id, delete_files).await?,
        }
        Ok(())
    }

    /// Closes an embedded engine, saving its state.
    pub async fn close(self) {
        if let Self::Held(engine) = self {
            engine.shutdown().await;
        }
    }

    /// The transfer that `query` names: a bundle ID, a unique prefix of one, or a name.
    pub async fn find(&self, query: &str) -> Result<TransferInfo> {
        let transfers = self.list().await?;
        find(transfers, query)
    }
}

/// The transfer that `query` names: a bundle ID, a unique prefix of at least four of its
/// characters, a name, or a name in any case.
fn find(transfers: Vec<TransferInfo>, query: &str) -> Result<TransferInfo> {
    let query_lower = query.to_lowercase();
    let rules: [&dyn Fn(&TransferInfo) -> bool; 4] = [
        &|t| t.id.to_string() == query_lower,
        &|t| query.len() >= 4 && t.id.to_string().starts_with(&query_lower),
        &|t| t.name == query,
        &|t| t.name.to_lowercase() == query_lower,
    ];
    for rule in rules {
        let mut matches: Vec<_> = transfers.iter().filter(|t| rule(t)).collect();
        match matches.len() {
            0 => continue,
            1 => return Ok(matches.remove(0).clone()),
            _ => {
                let names: Vec<_> = matches
                    .iter()
                    .map(|t| format!("{} {}", short_id(&t.id), t.name))
                    .collect();
                bail!(
                    "{query:?} matches several transfers; give more of the ID:\n  {}",
                    names.join("\n  ")
                );
            }
        }
    }
    bail!("no transfer matches {query:?}; `tempest list` shows them")
}

/// The first eight characters of a bundle ID, as `tempest list` shows it.
pub fn short_id(id: &BundleId) -> String {
    id.to_string()[..8].to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tempest_engine::{Kind, Status};
    use tempest_protocol::Hash;

    use super::*;

    fn transfer(seed: u8, name: &str) -> TransferInfo {
        TransferInfo {
            id: Hash::new([seed; 32]),
            name: name.to_owned(),
            kind: Kind::Download,
            status: Status::Stopped,
            size: None,
            have: 0,
            chunks: 0,
            progress: 0.0,
            eta: None,
            ratio: None,
            downloaded: 0,
            uploaded: 0,
            seeding_time: 0,
            download_rate: 0,
            upload_rate: 0,
            connections: 0,
            relayed: 0,
            peers: 0,
            added: 0,
            completed: None,
            queue_position: 0,
            forced: false,
            tags: BTreeSet::new(),
            category: None,
            save_folder: PathBuf::from("/downloads"),
            content_dir: None,
            link: String::new(),
            settings: Default::default(),
            published: BTreeSet::new(),
            stopped_by: None,
        }
    }

    #[test]
    fn transfers_are_found_by_id_prefix_or_name() {
        let transfers = vec![
            transfer(0xab, "Album"),
            transfer(0xac, "album"),
            transfer(0x12, "Photos"),
        ];
        let found = |query: &str| find(transfers.clone(), query).map(|t| t.name);
        assert_eq!(found(&Hash::new([0x12; 32]).to_string()).unwrap(), "Photos");
        assert_eq!(found("1212").unwrap(), "Photos");
        assert_eq!(found("abab").unwrap(), "Album");
        assert_eq!(found("Album").unwrap(), "Album");
        assert_eq!(found("album").unwrap(), "album");
        assert_eq!(found("photos").unwrap(), "Photos");
        assert!(found("ALBUM").unwrap_err().to_string().contains("several"));
        assert!(found("12").unwrap_err().to_string().contains("no transfer"));
        assert!(found("Films").is_err());
    }
}
