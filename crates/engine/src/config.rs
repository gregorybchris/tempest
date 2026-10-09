//! The engine's config file: YAML, with session settings as a profile plus overrides, as in
//! simulation scenarios (D-005, D-081).

use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use tempest_core::Config;
use tempest_core::config::{LingerSettings, Settings};
pub use tempest_net::dirs::default_data_dir;
use tempest_net::dirs::home_dir;
use tempest_net::{PortMappingConfig, RelayChoice};
use tempest_protocol::RelayIdentity;

pub use tempest_api::ui::{SavedView, UiSettings};

use crate::schedule::{LocalTime, Schedule};
use crate::store::Kind;

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
    /// Trackers that list every new share (D-129). Shares announce to them too.
    pub publish: Vec<String>,
    /// Relays to pick a home relay from (D-029: none by default).
    pub relays: RelaySettings,
    /// Asking the router to forward the peer port.
    pub port_mapping: PortMappingSettings,
    /// Speed limits over every transfer together.
    pub limits: SpeedLimitSettings,
    /// When a complete transfer stops sharing. A transfer can override them.
    pub sharing: SharingLimits,
    /// What to run when a transfer completes or fails.
    pub notifications: NotificationSettings,
    /// How the TUI and the web UI look, kept here so every client shares it (D-195).
    pub ui: UiSettings,
}

/// Speed limits over every transfer together, in bytes per second, `null` for none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpeedLimitSettings {
    /// Data received.
    pub download: Option<u64>,
    /// Data sent.
    pub upload: Option<u64>,
    /// Limits that replace these while switched on or while their schedule says so.
    pub alternative: AlternativeLimits,
}

/// Alternative speed limits, such as lower ones for working hours.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlternativeLimits {
    /// Data received, in bytes per second, `null` for none.
    pub download: Option<u64>,
    /// Data sent, in bytes per second, `null` for none.
    pub upload: Option<u64>,
    /// Switches them on, whatever the schedule says.
    pub enabled: bool,
    /// When they apply on their own, in local time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<Schedule>,
}

/// Download and upload limits in bytes per second, `null` for none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RateLimits {
    /// Data received.
    pub download: Option<u64>,
    /// Data sent.
    pub upload: Option<u64>,
}

impl SpeedLimitSettings {
    /// Whether the alternative limits apply at `at`.
    pub fn alternative_active(&self, at: LocalTime) -> bool {
        let alternative = &self.alternative;
        alternative.enabled
            || alternative
                .schedule
                .as_ref()
                .is_some_and(|s| s.contains(at))
    }

    /// The limits in effect at `at`.
    pub fn in_effect(&self, at: LocalTime) -> RateLimits {
        match self.alternative_active(at) {
            true => RateLimits {
                download: self.alternative.download,
                upload: self.alternative.upload,
            },
            false => RateLimits {
                download: self.download,
                upload: self.upload,
            },
        }
    }
}

/// When a complete transfer stops sharing: at a ratio, after a time sharing, after no peer has
/// wanted anything for a while, or whichever comes first (D-224). With none it shares until
/// stopped.
///
/// The engine counts the ratio and the time across runs. The idle timeout is the session's
/// (D-065), which counts only within a run and applies only to a download that finished in it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SharingLimits {
    /// Bytes sent per byte received (or per byte of the bundle, if it received none).
    pub ratio: Option<f64>,
    /// Time spent sharing while complete, such as `"12h"`.
    #[serde(with = "optional_duration")]
    pub time: Option<Duration>,
    /// How long no connected peer may want anything before a finished download stops.
    #[serde(with = "optional_duration")]
    pub idle_timeout: Option<Duration>,
    /// What happens at the limit.
    pub action: SharingAction,
}

/// A finished download shares until the swarm has no use for it: no ratio or time limit, only
/// the idle timeout (D-255).
impl Default for SharingLimits {
    fn default() -> Self {
        Self {
            ratio: None,
            time: None,
            idle_timeout: Some(Duration::from_secs(600)),
            action: SharingAction::Stop,
        }
    }
}

/// What happens to a transfer that reaches a sharing limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharingAction {
    /// It stops and shows as completed.
    #[default]
    Stop,
    /// It is removed, keeping its files.
    Remove,
}

impl SharingLimits {
    /// No limits: a share starts with these, so a link keeps working (D-156, D-224).
    pub const NONE: Self = Self {
        ratio: None,
        time: None,
        idle_timeout: None,
        action: SharingAction::Stop,
    };

    /// Whether a transfer that has sent `ratio` and shared for `time` has reached a limit.
    pub fn reached(&self, ratio: Option<f64>, time: Duration) -> bool {
        let by_ratio = matches!((self.ratio, ratio), (Some(limit), Some(ratio)) if ratio >= limit);
        let by_time = self.time.is_some_and(|limit| time >= limit);
        by_ratio || by_time
    }
}

/// A command run when a transfer completes or fails.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationSettings {
    /// The program and its arguments, run without a shell; empty for none. Variables named
    /// `TEMPEST_*` describe the transfer.
    pub command: Vec<String>,
}

/// A transfer's settings: the config's, with its own over them.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferConfig {
    /// Session settings, with the idle timeout from `sharing`.
    pub session: Config,
    /// Its own speed limits, within the limits over every transfer.
    pub limits: RateLimits,
    /// When it stops sharing.
    pub sharing: SharingLimits,
}

/// The settings a transfer can override besides session settings, under these prefixes.
const TRANSFER_PREFIXES: [&str; 2] = ["limits.", "sharing."];

/// What a transfer can override besides session settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferOverrides {
    limits: RateLimits,
    sharing: SharingLimits,
}

/// `Option<Duration>` as a duration string or `null`.
mod optional_duration {
    use super::*;

    pub fn serialize<S: Serializer>(
        duration: &Option<Duration>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match duration {
            Some(duration) => {
                serializer.serialize_str(&tempest_core::config::format_duration(*duration))
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Duration>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|text| {
                tempest_core::config::parse_duration(&text).map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

/// The relays to pick a home relay from: the one that answers fastest, which lends this peer a
/// relay address for peers that can't reach it directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RelaySettings {
    /// Relays as `<key>@host:port`.
    #[serde(with = "relay_identities")]
    pub known: Vec<RelayIdentity>,
    /// Also pick from the relays that trackers recommend.
    pub from_trackers: bool,
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self {
            known: Vec::new(),
            from_trackers: true,
        }
    }
}

impl RelaySettings {
    /// The relays as the endpoint takes them.
    pub fn choice(&self) -> RelayChoice {
        RelayChoice {
            known: self.known.clone(),
            from_trackers: self.from_trackers,
        }
    }
}

/// Relay identities as text.
mod relay_identities {
    use super::*;

    pub fn serialize<S: Serializer>(
        relays: &[RelayIdentity],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(relays.iter().map(ToString::to_string))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<RelayIdentity>, D::Error> {
        Vec::<String>::deserialize(deserializer)?
            .iter()
            .map(|text| {
                text.parse()
                    .map_err(|e| serde::de::Error::custom(format!("{text:?}: {e}")))
            })
            .collect()
    }
}

/// Port mapping settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortMappingSettings {
    /// Whether to ask the router to forward the peer port, with PCP, NAT-PMP or UPnP-IGD.
    pub enabled: bool,
}

impl Default for PortMappingSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl PortMappingSettings {
    /// The settings as the endpoint takes them.
    pub fn config(&self) -> PortMappingConfig {
        PortMappingConfig {
            enabled: self.enabled,
            ..PortMappingConfig::default()
        }
    }
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
            max_active_uploads: None,
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
            _ => changed = set_in(&changed, key, value)?,
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

    /// The keys that this config, as read from its file, sets and that `changes` override with
    /// another value, with the file's value. A key at its default counts as not set by the
    /// file, except under `settings.`, whose overrides exist only where the file has them.
    pub fn overridden_by(&self, changes: &BTreeMap<String, Value>) -> Vec<(String, Value)> {
        let own = serde_json::to_value(self).unwrap_or_default();
        let default = serde_json::to_value(Self::default()).unwrap_or_default();
        let at = |root: &Value, key: &str| {
            key.split('.')
                .try_fold(root, |value, part| value.get(part))
                .cloned()
        };
        let mut overridden = Vec::new();
        for (key, stored) in changes {
            let file = match key.strip_prefix("settings.") {
                Some(session_key) if session_key != "profile" => {
                    self.settings.overrides.get(session_key).cloned()
                }
                _ => at(&own, key).filter(|value| Some(value) != at(&default, key).as_ref()),
            };
            if let Some(file) = file.filter(|file| file != stored) {
                overridden.push((key.clone(), file));
            }
        }
        overridden
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
        check_rates(
            "limits",
            RateLimits {
                download: self.limits.download,
                upload: self.limits.upload,
            },
        )?;
        let alternative = &self.limits.alternative;
        check_rates(
            "limits.alternative",
            RateLimits {
                download: alternative.download,
                upload: alternative.upload,
            },
        )?;
        check_sharing("sharing", &self.sharing)?;
        if self
            .notifications
            .command
            .first()
            .is_some_and(String::is_empty)
        {
            return Err(invalid(
                "notifications.command",
                "the first item must name a program",
            ));
        }
        let mut names = std::collections::HashSet::new();
        for view in &self.ui.views {
            if view.name.trim().is_empty() {
                return Err(invalid("ui.views", "every view needs a name"));
            }
            if !names.insert(view.name.as_str()) {
                return Err(invalid(
                    "ui.views",
                    format!("two views are named {:?}", view.name),
                ));
            }
        }
        for (key, urls) in [("trackers", &self.trackers), ("publish", &self.publish)] {
            if let Some(url) = urls.iter().find(|url| !is_http_url(url)) {
                return Err(invalid(
                    key,
                    format!("{url:?} is not an http:// or https:// URL"),
                ));
            }
        }
        Ok(())
    }

    /// The session settings every transfer starts from.
    pub fn session(&self) -> Result<Config, ConfigError> {
        for key in self.settings.overrides.keys() {
            check_not_lingering(key)?;
        }
        let mut config = self.settings.resolve()?;
        set_lingering(&mut config, &self.sharing);
        config.validate()?;
        Ok(config)
    }

    /// The settings of a transfer with `overrides` of its own, by dotted key. Keys under
    /// `limits.` (`download` and `upload`, none by default) and `sharing.` (as in the config,
    /// whose values are the defaults) are the transfer's own; the rest are session settings,
    /// applied after the profile and the config's overrides.
    pub fn transfer(
        &self,
        kind: Kind,
        overrides: &BTreeMap<String, Value>,
    ) -> Result<TransferConfig, ConfigError> {
        let mut session = self.session()?;
        // A share has no limits unless it sets its own, so its link keeps working (D-224).
        let mut own = TransferOverrides {
            limits: RateLimits::default(),
            sharing: match kind {
                Kind::Download => self.sharing,
                Kind::Share => SharingLimits {
                    action: self.sharing.action,
                    ..SharingLimits::NONE
                },
            },
        };
        for (key, value) in overrides {
            check_not_lingering(key)?;
            match TRANSFER_PREFIXES
                .iter()
                .any(|prefix| key.starts_with(prefix))
            {
                true => own = set_in(&own, key, value.clone())?,
                false => session.set(key, value.clone())?,
            }
        }
        session.validate()?;
        check_rates("limits", own.limits)?;
        check_sharing("sharing", &own.sharing)?;
        set_lingering(&mut session, &own.sharing);
        Ok(TransferConfig {
            session,
            limits: own.limits,
            sharing: own.sharing,
        })
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

/// `config` with the setting at the dotted `key` replaced by `value`.
fn set_in<T: Serialize + serde::de::DeserializeOwned>(
    config: &T,
    key: &str,
    value: Value,
) -> Result<T, ConfigError> {
    let mut tree = serde_json::to_value(config).expect("every config serializes");
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
    serde_json::from_value(tree).map_err(|e| invalid(key, e.to_string()))
}

/// Checks speed limits, naming them under `prefix`.
fn check_rates(prefix: &str, limits: RateLimits) -> Result<(), ConfigError> {
    for (name, limit) in [("download", limits.download), ("upload", limits.upload)] {
        if limit == Some(0) {
            return Err(invalid(
                &format!("{prefix}.{name}"),
                "must be at least 1 byte per second, or null for no limit",
            ));
        }
    }
    Ok(())
}

/// Makes the session stop at the idle timeout. The engine enforces the other limits itself.
fn set_lingering(session: &mut Config, sharing: &SharingLimits) {
    session.lingering = LingerSettings {
        ratio: None,
        idle_timeout: sharing.idle_timeout,
        direct_send: false,
    };
}

/// Rejects a session setting under `lingering.`, which the `sharing.` settings replace.
fn check_not_lingering(key: &str) -> Result<(), ConfigError> {
    let Some(name) = key.strip_prefix("lingering.") else {
        return Ok(());
    };
    let message = match name {
        "ratio" | "idle_timeout" => format!("use sharing.{name}"),
        _ => "the engine sets it".to_owned(),
    };
    Err(invalid(key, message))
}

fn check_sharing(prefix: &str, sharing: &SharingLimits) -> Result<(), ConfigError> {
    if sharing
        .ratio
        .is_some_and(|ratio| !(ratio.is_finite() && ratio > 0.0))
    {
        return Err(invalid(
            &format!("{prefix}.ratio"),
            "must be a number above 0, or null for no limit",
        ));
    }
    for (name, limit) in [
        ("time", sharing.time),
        ("idle_timeout", sharing.idle_timeout),
    ] {
        if limit == Some(Duration::ZERO) {
            return Err(invalid(
                &format!("{prefix}.{name}"),
                "must be longer than 0, or null for no limit",
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    ["http://", "https://"].iter().any(|scheme| {
        lower
            .strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty())
    })
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
publish: [https://listings.example.org]
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
    fn relays_are_identities_and_none_ship_by_default() {
        let default = EngineConfig::default().relays;
        assert!(default.known.is_empty());
        assert!(default.from_trackers);

        let key = "ab".repeat(32);
        let text =
            format!("relays: {{ known: [\"{key}@relay.example.org\"], from_trackers: false }}");
        let config = EngineConfig::from_yaml(&text).unwrap();
        assert_eq!(config.relays.known[0].port, 4775);
        assert!(!config.relays.from_trackers);
        let yaml = config.to_yaml();
        assert!(
            yaml.contains(&format!("{key}@relay.example.org:4775")),
            "{yaml}"
        );
        assert_eq!(EngineConfig::from_yaml(&yaml).unwrap(), config);

        let mut config = EngineConfig::default();
        let error = config
            .set("relays.known", json!(["relay.example.org"]))
            .unwrap_err();
        assert!(error.to_string().starts_with("relays.known: "), "{error}");
    }

    #[test]
    fn port_mapping_is_on_by_default_and_applies_without_a_restart() {
        let default = EngineConfig::default();
        assert!(default.port_mapping.config().enabled);
        let config = EngineConfig::from_yaml("port_mapping: { enabled: false }").unwrap();
        assert!(!config.port_mapping.config().enabled);
        let mut changed = default.clone();
        changed.set("port_mapping.enabled", json!(false)).unwrap();
        assert_eq!(changed, config);
        assert!(changed.restart_required(&default).is_empty());
    }

    #[test]
    fn stored_changes_that_override_the_file_are_found() {
        let config = EngineConfig::from_yaml(
            "queue: { max_active_downloads: 2 }\nsettings: { overrides: { upload.reciprocal_slots: 7 } }",
        )
        .unwrap();
        let changes = BTreeMap::from([
            ("queue.max_active_downloads".to_owned(), json!(4)),
            // The file doesn't set this one.
            ("queue.max_active_uploads".to_owned(), json!(1)),
            ("settings.upload.reciprocal_slots".to_owned(), json!(9)),
        ]);
        assert_eq!(
            config.overridden_by(&changes),
            vec![
                ("queue.max_active_downloads".to_owned(), json!(2)),
                ("settings.upload.reciprocal_slots".to_owned(), json!(7)),
            ]
        );
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
        let session = config.transfer(Kind::Download, &transfer).unwrap().session;
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
    fn limits_sharing_and_notifications_are_read() {
        use crate::schedule::{TimeOfDay, Weekday};

        let config = EngineConfig::from_yaml(
            r#"
limits:
  download: 1000000
  alternative:
    upload: 50000
    schedule: { days: [mon, fri], from: "09:00", to: "17:00" }
sharing: { ratio: 2.5, time: 12h, action: remove }
notifications: { command: [sh, -c, "echo done"] }
"#,
        )
        .unwrap();
        assert_eq!(config.sharing.time, Some(Duration::from_secs(12 * 3600)));
        assert_eq!(config.sharing.action, SharingAction::Remove);
        let text = config.to_yaml();
        assert_eq!(EngineConfig::from_yaml(&text).unwrap(), config, "{text}");

        let at = |day, hour| LocalTime {
            day,
            time: TimeOfDay::new(hour, 0).unwrap(),
        };
        let normal = RateLimits {
            download: Some(1_000_000),
            upload: None,
        };
        let alternative = RateLimits {
            download: None,
            upload: Some(50_000),
        };
        assert_eq!(config.limits.in_effect(at(Weekday::Mon, 10)), alternative);
        assert_eq!(config.limits.in_effect(at(Weekday::Mon, 18)), normal);
        assert_eq!(config.limits.in_effect(at(Weekday::Tue, 10)), normal);
        let mut toggled = config.clone();
        toggled
            .set("limits.alternative.enabled", json!(true))
            .unwrap();
        assert_eq!(toggled.limits.in_effect(at(Weekday::Tue, 10)), alternative);
    }

    #[test]
    fn a_transfer_overrides_its_limits_and_sharing() {
        let config = EngineConfig::from_yaml("sharing: { ratio: 2, time: 1h }").unwrap();
        let defaults = config.transfer(Kind::Download, &BTreeMap::new()).unwrap();
        assert_eq!(defaults.limits, RateLimits::default());
        assert_eq!(defaults.sharing, config.sharing);
        assert_eq!(defaults.session, config.session().unwrap());
        assert_eq!(defaults.session.lingering.ratio, None);
        assert_eq!(
            defaults.session.lingering.idle_timeout,
            config.sharing.idle_timeout
        );

        let overrides = BTreeMap::from([
            ("limits.upload".to_owned(), json!(4096)),
            ("sharing.ratio".to_owned(), json!(null)),
            ("sharing.action".to_owned(), json!("remove")),
            ("upload.reciprocal_slots".to_owned(), json!(9)),
        ]);
        let transfer = config.transfer(Kind::Download, &overrides).unwrap();
        assert_eq!(transfer.limits.upload, Some(4096));
        assert_eq!(transfer.sharing.ratio, None);
        assert_eq!(transfer.sharing.time, Some(Duration::from_secs(3600)));
        assert_eq!(transfer.sharing.action, SharingAction::Remove);
        assert_eq!(transfer.session.upload.reciprocal_slots, 9);

        let error = |key: &str, value| {
            let overrides = BTreeMap::from([(key.to_owned(), value)]);
            config
                .transfer(Kind::Download, &overrides)
                .unwrap_err()
                .to_string()
        };
        assert!(error("limits.upload", json!(0)).contains("limits.upload"));
        assert!(error("limits.nope", json!(1)).contains("limits.nope"));
        assert!(error("sharing.ratio", json!(-1)).contains("sharing.ratio"));
        assert!(error("sharing.time", json!("soon")).contains("sharing.time"));
        assert!(error("sharing.idle_timeout", json!("0s")).contains("sharing.idle_timeout"));
        assert!(error("lingering.ratio", json!(2)).contains("sharing.ratio"));
        assert!(error("lingering.idle_timeout", json!("1m")).contains("sharing.idle_timeout"));
    }

    #[test]
    fn a_share_has_no_limits_of_its_own_unless_it_sets_them() {
        let config = EngineConfig::from_yaml("sharing: { action: remove }").unwrap();
        let share = config.transfer(Kind::Share, &BTreeMap::new()).unwrap();
        assert_eq!(
            share.sharing,
            SharingLimits {
                action: SharingAction::Remove,
                ..SharingLimits::NONE
            }
        );
        assert_eq!(share.session.lingering.idle_timeout, None);
        let own = BTreeMap::from([("sharing.ratio".to_owned(), json!(3))]);
        let share = config.transfer(Kind::Share, &own).unwrap();
        assert_eq!(share.sharing.ratio, Some(3.0));
        assert_eq!(share.sharing.time, None);
    }

    #[test]
    fn session_lingering_settings_are_refused_in_the_config() {
        let error = |yaml: &str| EngineConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error("settings: { overrides: { lingering.ratio: 2 } }").contains("sharing.ratio"));
        let mut config = EngineConfig::default();
        let refused = config.set("settings.lingering.idle_timeout", json!("1m"));
        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("sharing.idle_timeout")
        );
    }

    #[test]
    fn the_clients_look_and_views_are_kept_and_apply_at_once() {
        let default = EngineConfig::default();
        assert_eq!(default.ui.theme, "green");
        let mut changed = default.clone();
        changed.set("ui.theme", json!("amber")).unwrap();
        changed.set("ui.columns", json!(["size", "tags"])).unwrap();
        let views = json!([{ "name": "Music", "query": "#music", "sort": "size" }]);
        changed.set("ui.views", views).unwrap();
        assert_eq!(changed.ui.views[0].status, None);
        assert!(changed.restart_required(&default).is_empty());
        let text = changed.to_yaml();
        assert_eq!(EngineConfig::from_yaml(&text).unwrap(), changed, "{text}");

        let twice = json!([{ "name": "a" }, { "name": "a" }]);
        assert!(changed.set("ui.views", twice).is_err());
        assert!(changed.set("ui.views", json!([{ "name": " " }])).is_err());
    }

    #[test]
    fn sharing_limits_are_reached_by_ratio_or_time() {
        let hour = Duration::from_secs(3600);
        let limits = SharingLimits {
            ratio: Some(2.0),
            time: Some(hour),
            ..SharingLimits::NONE
        };
        assert!(!limits.reached(Some(1.9), hour / 2));
        assert!(limits.reached(Some(2.0), Duration::ZERO));
        assert!(limits.reached(None, hour));
        assert!(!SharingLimits::NONE.reached(Some(100.0), hour * 1000));
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
        assert!(error("publish: [tracker.example.org]").contains("publish"));
        assert!(error("limits: { upload: 0 }").contains("limits.upload"));
        assert!(
            error("limits: { alternative: { download: 0 } }")
                .contains("limits.alternative.download")
        );
        assert!(
            error("limits: { alternative: { schedule: { from: '25:00', to: '01:00' } } }")
                .contains("25:00")
        );
        assert!(error("sharing: { ratio: 0 }").contains("sharing.ratio"));
        assert!(error("sharing: { time: 0s }").contains("sharing.time"));
        assert!(error("sharing: { action: pause }").contains("pause"));
        assert!(error("notifications: { command: [''] }").contains("notifications.command"));
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
