//! The engine's config file: YAML, with session settings as a profile plus overrides, as in
//! simulation scenarios (D-005, D-081).

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempest_core::Config;
use tempest_core::config::Settings;

/// The engine's settings, as read from its config file.
///
/// Every field has a default, so an empty file is a valid config.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineConfig {
    /// Session settings for every transfer: a profile plus overrides by dotted key. A transfer
    /// can override them further.
    pub settings: Settings,
    /// Where peers reach this one.
    pub peers: PeerSettings,
    /// LAN discovery (D-099).
    pub lan: LanSettings,
    /// The HTTP API and web UI (D-080, D-125).
    pub api: ApiSettings,
    /// Where state and the identity are kept. Defaults to Tempest's folder in the per-user data
    /// directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    /// Where downloads go unless a category or the add says otherwise. Defaults to the user's
    /// Downloads folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_folder: Option<PathBuf>,
    /// How many transfers run at once.
    pub queue: QueueSettings,
    /// Trackers to announce every transfer to, besides those its link names.
    pub trackers: Vec<String>,
}

/// Where peers reach this one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PeerSettings {
    /// The UDP port for QUIC (D-126), 0 for any.
    pub port: u16,
    /// Addresses to bind instead of every interface, at most one per address family.
    pub bind: Vec<IpAddr>,
}

impl Default for PeerSettings {
    fn default() -> Self {
        Self {
            port: tempest_net::DEFAULT_PORT,
            bind: Vec::new(),
        }
    }
}

/// LAN discovery settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanSettings {
    /// Whether to find peers on the local network and be found there.
    pub enabled: bool,
    /// The UDP port of the multicast groups (D-126).
    pub port: u16,
}

impl Default for LanSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: tempest_protocol::lan::LAN_PORT,
        }
    }
}

/// Where the API listens, and who may use it (D-125).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApiSettings {
    /// The address to bind. Only loopback unless remote access is on.
    pub bind: IpAddr,
    /// The TCP port (D-126), 0 for any.
    pub port: u16,
    /// Remote access: serve HTTPS and require the access token on every request.
    pub remote: bool,
    /// The HTTPS certificate chain, as PEM, for remote access. Without it the engine makes a
    /// self-signed certificate in the data directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert: Option<PathBuf>,
    /// The certificate's private key, as PEM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_key: Option<PathBuf>,
}

/// The engine API's default TCP port (D-126).
pub const DEFAULT_API_PORT: u16 = 4776;

impl Default for ApiSettings {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: DEFAULT_API_PORT,
            remote: false,
            tls_cert: None,
            tls_key: None,
        }
    }
}

/// Limits on how many transfers run at once. Transfers beyond them wait, queued, in queue
/// order. A force-started transfer runs regardless and takes no slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QueueSettings {
    /// The most incomplete transfers downloading at once, or `null` for no limit.
    pub max_active_downloads: Option<u32>,
    /// The most complete transfers sharing at once, or `null` for no limit.
    pub max_active_uploads: Option<u32>,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self {
            max_active_downloads: Some(3),
            max_active_uploads: Some(5),
        }
    }
}

/// A config that can't be read or isn't valid.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The config file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The text is not valid YAML for a config.
    #[error("invalid config: {0}")]
    Parse(String),
    /// A session setting is unknown or out of range.
    #[error("settings: {0}")]
    Session(#[from] tempest_core::config::ConfigError),
    /// An engine setting is unknown or out of range.
    #[error("{key}: {message}")]
    Invalid {
        /// The setting, as a dotted key.
        key: String,
        /// What is wrong.
        message: String,
    },
}

fn invalid(key: &str, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        key: key.to_owned(),
        message: message.into(),
    }
}

impl EngineConfig {
    /// Reads and checks the config file at `path`. A missing file is the default config.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_yaml(&text),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Writes the config to `path` as YAML.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        std::fs::write(path, self.to_yaml()).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })
    }

    /// Reads and checks a config from YAML text.
    pub fn from_yaml(text: &str) -> Result<Self, ConfigError> {
        // YAML 1.2 booleans only, as in scenarios, so that values such as `off` stay strings.
        let options = serde_saphyr::options! { strict_booleans: true };
        let config: Self = serde_saphyr::from_str_with_options(text, options)
            .map_err(|e| ConfigError::Parse(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// The config as YAML text.
    pub fn to_yaml(&self) -> String {
        serde_saphyr::to_string(self).expect("every config serializes")
    }

    /// Overrides one setting by dotted key, such as `peers.port` or `queue.max_active_uploads`,
    /// and checks the result. Keys under `settings.`, other than `settings.profile`, are session
    /// settings and become overrides of the profile, such as `settings.upload.reciprocal_slots`.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let mut changed = self.clone();
        match key.strip_prefix("settings.") {
            Some(session_key) if session_key != "profile" => {
                // Checked here so the error names the key.
                Config::default().set(session_key, value.clone())?;
                changed
                    .settings
                    .overrides
                    .insert(session_key.to_owned(), value);
            }
            _ => {
                let mut tree = serde_json::to_value(&changed).expect("every config serializes");
                let no_such = || invalid(key, "there is no such setting");
                let (parents, last) = match key.rsplit_once('.') {
                    Some((parents, last)) => (Some(parents), last),
                    None => (None, key),
                };
                let mut object = tree.as_object_mut().expect("a config is an object");
                for part in parents.into_iter().flat_map(|p| p.split('.')) {
                    object = object
                        .get_mut(part)
                        .and_then(Value::as_object_mut)
                        .ok_or_else(no_such)?;
                }
                // Unset optional settings are left out of the tree; unknown ones fail to parse.
                object.insert(last.to_owned(), value);
                changed = serde_json::from_value(tree).map_err(|e| invalid(key, e.to_string()))?;
            }
        }
        changed.validate()?;
        *self = changed;
        Ok(())
    }

    /// The config with `changes`, by dotted key, applied in key order, as [`set`](Self::set)
    /// would.
    pub fn with_changes(&self, changes: &BTreeMap<String, Value>) -> Result<Self, ConfigError> {
        let mut config = self.clone();
        for (key, value) in changes {
            config.set(key, value.clone())?;
        }
        Ok(config)
    }

    /// The groups of settings that differ from `running`'s and take effect only when the
    /// engine restarts: `peers`, `lan` and `api`.
    pub fn restart_required(&self, running: &Self) -> Vec<&'static str> {
        let mut groups = Vec::new();
        if self.peers != running.peers {
            groups.push("peers");
        }
        if self.lan != running.lan {
            groups.push("lan");
        }
        if self.api != running.api {
            groups.push("api");
        }
        groups
    }

    /// Checks every setting.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.session()?;
        let mut families = [false; 2];
        for ip in &self.peers.bind {
            let family = &mut families[usize::from(ip.is_ipv6())];
            if std::mem::replace(family, true) {
                return Err(invalid(
                    "peers.bind",
                    "at most one address per address family",
                ));
            }
        }
        if !self.api.bind.is_loopback() && !self.api.remote {
            return Err(invalid(
                "api.bind",
                "must be a loopback address unless api.remote is on",
            ));
        }
        if self.api.tls_cert.is_some() != self.api.tls_key.is_some() {
            return Err(invalid(
                "api.tls_cert",
                "api.tls_cert and api.tls_key go together",
            ));
        }
        for (key, path) in [
            ("data_dir", &self.data_dir),
            ("save_folder", &self.save_folder),
            ("api.tls_cert", &self.api.tls_cert),
            ("api.tls_key", &self.api.tls_key),
        ] {
            if path.as_ref().is_some_and(|path| !path.is_absolute()) {
                return Err(invalid(key, "must be an absolute path"));
            }
        }
        for (key, limit) in [
            (
                "queue.max_active_downloads",
                self.queue.max_active_downloads,
            ),
            ("queue.max_active_uploads", self.queue.max_active_uploads),
        ] {
            if limit == Some(0) {
                return Err(invalid(key, "must be at least 1, or null for no limit"));
            }
        }
        if let Some(url) = self.trackers.iter().find(|url| !is_http_url(url)) {
            return Err(invalid(
                "trackers",
                format!("{url:?} is not an http:// or https:// URL"),
            ));
        }
        Ok(())
    }

    /// The session settings every transfer starts from.
    pub fn session(&self) -> Result<Config, ConfigError> {
        let config = self.settings.resolve()?;
        config.validate()?;
        Ok(config)
    }

    /// The session settings of a transfer with `overrides` of its own, applied after the
    /// profile and the config's overrides.
    pub fn transfer_session(
        &self,
        overrides: &BTreeMap<String, Value>,
    ) -> Result<Config, ConfigError> {
        let mut config = self.session()?;
        for (key, value) in overrides {
            config.set(key, value.clone())?;
        }
        config.validate()?;
        Ok(config)
    }

    /// Where state and the identity are kept.
    pub fn data_dir(&self) -> Result<PathBuf, ConfigError> {
        match &self.data_dir {
            Some(dir) => Ok(dir.clone()),
            None => default_data_dir()
                .ok_or_else(|| invalid("data_dir", "cannot find the per-user data directory")),
        }
    }

    /// Where downloads go by default.
    pub fn save_folder(&self) -> Result<PathBuf, ConfigError> {
        match &self.save_folder {
            Some(dir) => Ok(dir.clone()),
            None => home_dir()
                .map(|home| home.join("Downloads"))
                .ok_or_else(|| invalid("save_folder", "cannot find the home directory")),
        }
    }
}

fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://"].iter().any(|scheme| {
        lower
            .strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
}

fn home_dir() -> Option<PathBuf> {
    std::env::home_dir().filter(|path| path.is_absolute())
}

/// Tempest's folder in the per-user data directory: the local application data folder on
/// Windows, Application Support on macOS, and the XDG data directory elsewhere (D-175).
pub fn default_data_dir() -> Option<PathBuf> {
    let absolute = |path: PathBuf| Some(path).filter(|path| path.is_absolute());
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).and_then(absolute);
    let data_dir = if cfg!(windows) {
        env("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        home_dir().map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_DATA_HOME").or_else(|| home_dir().map(|home| home.join(".local/share")))
    };
    let tempest = if cfg!(any(windows, target_os = "macos")) {
        "Tempest"
    } else {
        "tempest"
    };
    Some(data_dir?.join(tempest))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempest_core::config::Profile;

    use super::*;

    #[test]
    fn an_empty_file_is_the_default_config() {
        let config = EngineConfig::from_yaml("{}").unwrap();
        assert_eq!(config, EngineConfig::default());
        assert_eq!(config.peers.port, 4773);
        assert_eq!(config.lan.port, 4774);
        assert_eq!(config.api.port, 4776);
        assert!(config.api.bind.is_loopback());
    }

    #[test]
    fn a_config_round_trips_through_yaml() {
        let config = EngineConfig::from_yaml(
            r"
settings:
  profile: cooperative
  overrides:
    upload.reciprocal_slots: 6
    strategies.chunk_selection: sequential
peers: { port: 5000, bind: [127.0.0.1] }
lan: { enabled: false }
data_dir: /var/lib/tempest
save_folder: /srv/downloads
queue: { max_active_downloads: null, max_active_uploads: 2 }
trackers: [https://tracker.example.org]
",
        )
        .unwrap();
        assert_eq!(config.settings.profile, Profile::Cooperative);
        assert_eq!(config.queue.max_active_downloads, None);
        assert!(!config.lan.enabled);
        let text = config.to_yaml();
        assert_eq!(EngineConfig::from_yaml(&text).unwrap(), config, "{text}");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        assert_eq!(EngineConfig::load(&path).unwrap(), EngineConfig::default());
        config.save(&path).unwrap();
        assert_eq!(EngineConfig::load(&path).unwrap(), config);
    }

    #[test]
    fn overrides_beat_the_profile_and_transfer_overrides_beat_both() {
        let mut config = EngineConfig::from_yaml(
            "settings: { profile: background, overrides: { upload.reciprocal_slots: 7 } }",
        )
        .unwrap();
        let background = Profile::Background.config();
        let session = config.session().unwrap();
        // The profile sets what isn't overridden.
        assert_eq!(session.connections.max, background.connections.max);
        assert_eq!(session.upload.reciprocal_slots, 7);

        let transfer = BTreeMap::from([("upload.reciprocal_slots".to_owned(), json!(9))]);
        let session = config.transfer_session(&transfer).unwrap();
        assert_eq!(session.upload.reciprocal_slots, 9);
        assert_eq!(session.connections.max, background.connections.max);

        // `set` overrides the file, for both kinds of setting.
        config
            .set("settings.upload.reciprocal_slots", json!(3))
            .unwrap();
        config.set("queue.max_active_uploads", json!(1)).unwrap();
        config.set("settings.profile", json!("classic")).unwrap();
        assert_eq!(config.session().unwrap().upload.reciprocal_slots, 3);
        assert_eq!(config.settings.profile, Profile::Classic);
        assert_eq!(config.queue.max_active_uploads, Some(1));

        // Unset optional settings can be set too.
        config.set("save_folder", json!("/srv/downloads")).unwrap();
        assert_eq!(config.save_folder, Some(PathBuf::from("/srv/downloads")));

        let changes = BTreeMap::from([
            ("api.port".to_owned(), json!(5000)),
            ("queue.max_active_downloads".to_owned(), json!(null)),
        ]);
        let changed = config.with_changes(&changes).unwrap();
        assert_eq!(changed.queue.max_active_downloads, None);
        assert_eq!(changed.restart_required(&config), ["api"]);
    }

    #[test]
    fn invalid_settings_are_explained() {
        let error = |yaml: &str| EngineConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error("settings: { profile: fast }").contains("fast"));
        assert!(
            error("settings: { overrides: { upload.nonsense: 1 } }").contains("upload.nonsense")
        );
        assert!(error("peers: { prt: 1 }").contains("prt"));
        assert!(error("peers: { bind: [127.0.0.1, 10.0.0.1] }").contains("peers.bind"));
        assert!(error("api: { bind: 0.0.0.0 }").contains("api.bind"));
        assert!(EngineConfig::from_yaml("api: { bind: 0.0.0.0, remote: true }").is_ok());
        assert!(error("api: { tls_cert: /etc/cert.pem }").contains("api.tls_key"));
        assert!(error("data_dir: relative/path").contains("data_dir"));
        assert!(error("queue: { max_active_downloads: 0 }").contains("max_active_downloads"));
        assert!(error("trackers: [ftp://example.org]").contains("trackers"));
        assert!(error("lan: { enabled: off }").contains("invalid config"));

        let mut config = EngineConfig::default();
        let before = config.clone();
        assert!(config.set("peers.nope", json!(1)).is_err());
        assert!(config.set("peers.port", json!("many")).is_err());
        assert!(config.set("settings.upload.nonsense", json!(1)).is_err());
        assert!(config.set("queue.max_active_uploads", json!(0)).is_err());
        assert_eq!(config, before, "a failed set changes nothing");
    }
}
