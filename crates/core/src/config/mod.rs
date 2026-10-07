//! Session settings and strategy choices: profiles plus per-setting overrides (D-005).
//!
//! A [`Config`] holds every setting, grouped by topic. A [`Profile`] fills in all of them, and
//! [`Config::set`] then overrides single settings by dotted key, such as
//! `strategies.chunk_selection` or `upload.reciprocal_slots`. Every type here is serde-ready, so
//! a scenario file can name a profile and a list of overrides ([`Settings`]).

mod duration;
mod profile;

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempest_protocol::message::MAX_REQUEST_CHUNKS;

pub use self::duration::{format as format_duration, parse as parse_duration};
pub use self::profile::{Profile, UnknownProfile};
use crate::select::{self, ChunkSelector};
use crate::upload::{self, UploadStrategy};

/// Everything a [`Session`](crate::Session) can be configured with.
///
/// The defaults are the `balanced` profile: starting points, to be tuned with simulation
/// (`specs/strategies.md`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// The client name and version sent in Hello.
    pub client: String,
    /// Which strategy to use for each decision.
    pub strategies: Strategies,
    /// Chunk selection parameters.
    pub selection: SelectionSettings,
    /// Request sizing and pipelining.
    pub requests: RequestSettings,
    /// Endgame parameters.
    pub endgame: EndgameSettings,
    /// Upload limits and upload strategy parameters.
    pub upload: UploadSettings,
    /// Connection management and peer exchange.
    pub connections: ConnectionSettings,
    /// When to stop sharing after completing.
    pub lingering: LingerSettings,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client: concat!("tempest/", env!("CARGO_PKG_VERSION")).to_owned(),
            strategies: Strategies::default(),
            selection: SelectionSettings::default(),
            requests: RequestSettings::default(),
            endgame: EndgameSettings::default(),
            upload: UploadSettings::default(),
            connections: ConnectionSettings::default(),
            lingering: LingerSettings::default(),
        }
    }
}

/// The strategy chosen for each decision (`specs/strategies.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Strategies {
    /// Which chunks to request (D-059).
    pub chunk_selection: ChunkSelection,
    /// Whom to serve while downloading (D-051, D-062).
    pub upload: Upload,
    /// Whom to serve once complete (D-063).
    pub when_complete: WhenComplete,
    /// Whether a peer that starts with every chunk superseeds: it offers each chunk to only one
    /// peer until every chunk has been sent once (D-063). For a bundle's only, brand-new copy.
    pub superseed: bool,
    /// How to finish the last chunks (D-061).
    pub endgame: Endgame,
}

/// A chunk selection strategy (D-059).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChunkSelection {
    /// The chunks the fewest connected peers have, after the first few at random.
    #[default]
    RarestFirst,
    /// Any needed chunk at random.
    Random,
    /// Chunks in order.
    Sequential,
    /// Chunks near the playback position in order, rarest-first elsewhere.
    Streaming,
}

impl ChunkSelection {
    /// Builds the strategy.
    pub fn build(self, settings: &SelectionSettings) -> Box<dyn ChunkSelector> {
        let rarest = select::RarestFirst {
            random_first: settings.random_first,
        };
        match self {
            Self::RarestFirst => Box::new(rarest),
            Self::Random => Box::new(select::Random),
            Self::Sequential => Box::new(select::Sequential),
            Self::Streaming => Box::new(select::Streaming {
                window: settings.streaming_window,
                elsewhere: rarest,
            }),
        }
    }
}

/// An upload strategy for a peer that is still downloading (D-062).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Upload {
    /// Serve the peers that upload to us fastest, plus a rotating optimistic slot.
    #[default]
    Bittorrent,
    /// Share upload in proportion to what each peer uploaded to us.
    Propshare,
    /// Send each next response to the peer we owe most.
    Fairtorrent,
    /// Serve every peer that wants data.
    Cooperative,
}

impl Upload {
    /// Builds the strategy.
    pub fn build(self, settings: &UploadSettings) -> Box<dyn UploadStrategy> {
        match self {
            Self::Bittorrent => Box::new(upload::BitTorrent::new(settings)),
            Self::Propshare => Box::new(upload::PropShare::new(settings)),
            Self::Fairtorrent => Box::new(upload::FairTorrent),
            Self::Cooperative => Box::new(upload::Cooperative),
        }
    }
}

/// Whom a complete peer serves (D-063).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhenComplete {
    /// Serve every interested peer, answering their requests in turn.
    #[default]
    RoundRobin,
    /// Keep the upload strategy used while downloading.
    Unchanged,
}

/// An endgame strategy (D-061).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Endgame {
    /// Request each remaining chunk from up to `endgame.extra_copies` more peers.
    #[default]
    Limited,
    /// Request each remaining chunk from every peer that has it.
    Classic,
    /// No endgame: each chunk is requested from one peer at a time. An experimental contrast.
    Off,
}

impl Endgame {
    /// The most open requests a remaining chunk may have at once during endgame.
    pub fn max_copies(self, settings: &EndgameSettings) -> u32 {
        match self {
            Self::Limited => settings.extra_copies.saturating_add(1),
            Self::Classic => u32::MAX,
            Self::Off => 1,
        }
    }
}

/// Chunk selection parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SelectionSettings {
    /// How many chunks rarest-first picks at random before switching to rarest-first.
    pub random_first: u32,
    /// How many chunks from the playback position the streaming strategy requests in order.
    pub streaming_window: u32,
}

impl Default for SelectionSettings {
    fn default() -> Self {
        Self {
            random_first: 4,
            streaming_window: 64,
        }
    }
}

/// Request sizing and pipelining (D-060).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RequestSettings {
    /// The number of chunks asked for in one Request, when possible: a power of two from 1 to
    /// 1,024. Requests are smaller at file ends, during endgame and when the provider's limit is
    /// lower.
    pub chunks: u32,
    /// Whether the number of requests kept open with a provider follows its measured rate and
    /// round-trip time. If not, it stays at `initial_in_flight`.
    pub adaptive: bool,
    /// Requests kept open with a provider before its rate and round-trip time are measured.
    pub initial_in_flight: u32,
    /// The fewest requests kept open with a provider that is serving this peer.
    pub min_in_flight: u32,
    /// Bytes kept requested from a provider, as a multiple of its rate × round-trip time. Above
    /// 1, a provider that could send faster gets more requests, so the amount grows until the
    /// link, not the pipeline, limits the rate.
    pub in_flight_factor: f64,
    /// How far below a provider's `max_open_requests` this peer stays, since a provider's count
    /// can lag behind (`protocol/messages.md`, Request).
    pub headroom: u32,
    /// How long to wait before asking a provider again after it rejected a request.
    #[serde(with = "duration")]
    pub retry_delay: Duration,
    /// A provider that sends no Data for this long while requests are open is snubbing this
    /// peer: its requests are cancelled and sent elsewhere, and it gets one at a time until it
    /// sends Data again.
    #[serde(with = "duration")]
    pub snub_timeout: Duration,
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            chunks: 16,
            adaptive: true,
            initial_in_flight: 4,
            min_in_flight: 2,
            in_flight_factor: 2.0,
            headroom: 2,
            retry_delay: Duration::from_secs(1),
            snub_timeout: Duration::from_secs(60),
        }
    }
}

/// Endgame parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EndgameSettings {
    /// *k*: how many more peers the limited endgame asks for each remaining chunk.
    pub extra_copies: u32,
}

impl Default for EndgameSettings {
    fn default() -> Self {
        Self { extra_copies: 2 }
    }
}

/// Upload limits and upload strategy parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UploadSettings {
    /// The largest Request `count` this peer accepts, sent in Hello: a power of two from 1 to
    /// 1,024.
    pub max_request_chunks: u32,
    /// The most Requests this peer accepts open at once from one requester, sent in Hello.
    pub max_open_requests: u32,
    /// The most Data responses sent at once, across all connections. Further accepted requests
    /// wait in a queue that the upload strategy orders.
    pub parallel_responses: u32,
    /// The window over which transfer rates are measured.
    #[serde(with = "duration")]
    pub rate_window: Duration,
    /// BitTorrent: how many peers are served for reciprocity.
    pub reciprocal_slots: u32,
    /// BitTorrent and PropShare: how often to review whom to serve.
    #[serde(with = "duration")]
    pub review_interval: Duration,
    /// BitTorrent and PropShare: how many peers are served optimistically, chosen at random.
    pub optimistic_slots: u32,
    /// BitTorrent and PropShare: how often the optimistic slots rotate.
    #[serde(with = "duration")]
    pub optimistic_interval: Duration,
}

impl Default for UploadSettings {
    fn default() -> Self {
        Self {
            max_request_chunks: 64,
            max_open_requests: 32,
            parallel_responses: 8,
            rate_window: Duration::from_secs(20),
            reciprocal_slots: 4,
            review_interval: Duration::from_secs(10),
            optimistic_slots: 1,
            optimistic_interval: Duration::from_secs(30),
        }
    }
}

/// Connection management and peer exchange (D-064).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConnectionSettings {
    /// The most connections for the bundle.
    pub max: u32,
    /// The most connections this peer dials at once: being dialed, or dialed and not yet open.
    /// The rest of the limit stays free for peers that dial this one, so that peers arriving
    /// together don't fill their limits with dials to peers that will refuse them.
    pub max_dialing: u32,
    /// How long to wait before dialing a peer again after a connection to it failed or ended.
    #[serde(with = "duration")]
    pub redial_delay: Duration,
    /// A connection that has carried no Data either way for this long is idle, and is dropped
    /// first when the limit is reached.
    #[serde(with = "duration")]
    pub idle_after: Duration,
    /// How often a session at its connection limit checks whether to drop an idle connection
    /// to try a peer it hasn't met.
    #[serde(with = "duration")]
    pub review_interval: Duration,
    /// The most peers remembered for dialing.
    pub max_known_peers: u32,
    /// Whether to tell connected peers about other peers with Peers messages.
    pub peer_exchange: bool,
    /// How often Peers is sent on a connection. The protocol asks for at most once a minute.
    #[serde(with = "duration")]
    pub peer_exchange_interval: Duration,
    /// The most chunk ranges kept from a peer's Have and HaveUpdates while the manifest is
    /// unknown. A peer that sends more is disconnected.
    pub max_pending_ranges: u32,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            max: 50,
            max_dialing: 8,
            redial_delay: Duration::from_secs(60),
            idle_after: Duration::from_secs(60),
            review_interval: Duration::from_secs(30),
            max_known_peers: 1000,
            peer_exchange: true,
            peer_exchange_interval: Duration::from_secs(60),
            max_pending_ranges: 1 << 20,
        }
    }
}

/// When to stop sharing after completing (D-065, D-066).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LingerSettings {
    /// Keep sharing indefinitely, as the user chose.
    pub keep_sharing: bool,
    /// After downloading, stop once uploaded ≥ `ratio` × downloaded.
    pub ratio: f64,
    /// After downloading, stop once no peer has wanted anything for this long.
    #[serde(with = "duration")]
    pub idle_timeout: Duration,
    /// This session is the sender of a direct send: stop once a recipient has every chunk and
    /// no connected peer still lacks any.
    pub direct_send: bool,
}

impl Default for LingerSettings {
    fn default() -> Self {
        Self {
            keep_sharing: false,
            ratio: 1.0,
            idle_timeout: Duration::from_secs(600),
            direct_send: false,
        }
    }
}

/// A profile and the settings that override it, as a scenario file or settings screen gives
/// them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// The profile that sets every setting.
    pub profile: Profile,
    /// Single settings by dotted key, applied in key order.
    pub overrides: BTreeMap<String, Value>,
}

impl Settings {
    /// The profile's config with the overrides applied.
    pub fn resolve(&self) -> Result<Config, ConfigError> {
        let mut config = self.profile.config();
        for (key, value) in &self.overrides {
            config.set(key, value.clone())?;
        }
        Ok(config)
    }
}

/// A setting that is unknown or out of range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A chunk count was not a power of two from 1 to 1,024.
    #[error("{field} is {value}, not a power of two from 1 to {MAX_REQUEST_CHUNKS}")]
    NotRequestSize {
        /// The setting.
        field: &'static str,
        /// Its value.
        value: u32,
    },
    /// A count or duration that must be positive was 0.
    #[error("{0} must be more than 0")]
    Zero(&'static str),
    /// A number was not finite and positive.
    #[error("{0} must be a finite number above 0")]
    NotPositive(&'static str),
    /// No setting has this dotted key.
    #[error("there is no setting {0:?}")]
    UnknownSetting(String),
    /// The value doesn't fit the setting.
    #[error("{key}: {message}")]
    InvalidValue {
        /// The setting.
        key: String,
        /// What is wrong.
        message: String,
    },
}

impl Config {
    /// Checks that every setting is in range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("upload.max_request_chunks", self.upload.max_request_chunks),
            ("requests.chunks", self.requests.chunks),
        ] {
            if !value.is_power_of_two() || value > MAX_REQUEST_CHUNKS {
                return Err(ConfigError::NotRequestSize { field, value });
            }
        }
        for (field, value) in [
            ("upload.max_open_requests", self.upload.max_open_requests),
            ("upload.parallel_responses", self.upload.parallel_responses),
            (
                "requests.initial_in_flight",
                self.requests.initial_in_flight,
            ),
            ("requests.min_in_flight", self.requests.min_in_flight),
            ("connections.max", self.connections.max),
            ("connections.max_dialing", self.connections.max_dialing),
        ] {
            if value == 0 {
                return Err(ConfigError::Zero(field));
            }
        }
        for (field, value) in [
            ("upload.rate_window", self.upload.rate_window),
            ("upload.review_interval", self.upload.review_interval),
            (
                "upload.optimistic_interval",
                self.upload.optimistic_interval,
            ),
            ("requests.snub_timeout", self.requests.snub_timeout),
            (
                "connections.review_interval",
                self.connections.review_interval,
            ),
            (
                "connections.peer_exchange_interval",
                self.connections.peer_exchange_interval,
            ),
        ] {
            if value.is_zero() {
                return Err(ConfigError::Zero(field));
            }
        }
        for (field, value) in [
            ("requests.in_flight_factor", self.requests.in_flight_factor),
            ("lingering.ratio", self.lingering.ratio),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(ConfigError::NotPositive(field));
            }
        }
        Ok(())
    }

    /// Overrides one setting, named by its dotted key, such as `strategies.upload` or
    /// `requests.chunks`. Values take the same form as in a scenario file: strategy names in
    /// kebab case, durations as strings such as `"10s"`.
    ///
    /// The config is unchanged if the key is unknown, names a group of settings, or the value
    /// doesn't fit or is out of range.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), ConfigError> {
        let mut tree = serde_json::to_value(&*self).expect("configs serialize");
        let slot = key
            .split('.')
            .try_fold(&mut tree, |node, part| node.get_mut(part))
            .filter(|slot| !slot.is_object())
            .ok_or_else(|| ConfigError::UnknownSetting(key.to_owned()))?;
        *slot = value;
        let config: Config =
            serde_json::from_value(tree).map_err(|error| ConfigError::InvalidValue {
                key: key.to_owned(),
                message: error.to_string(),
            })?;
        config.validate()?;
        *self = config;
        Ok(())
    }

    /// One setting's value, named by its dotted key, or `None` if there is no such setting.
    pub fn get(&self, key: &str) -> Option<Value> {
        let tree = serde_json::to_value(self).expect("configs serialize");
        key.split('.')
            .try_fold(&tree, |node, part| node.get(part))
            .filter(|value| !value.is_object())
            .cloned()
    }

    /// Every setting's dotted key, in order.
    pub fn keys() -> Vec<String> {
        fn walk(prefix: &str, value: &Value, keys: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    for (name, child) in map {
                        let key = if prefix.is_empty() {
                            name.clone()
                        } else {
                            format!("{prefix}.{name}")
                        };
                        walk(&key, child, keys);
                    }
                }
                _ => keys.push(prefix.to_owned()),
            }
        }
        let tree = serde_json::to_value(Config::default()).expect("configs serialize");
        let mut keys = Vec::new();
        walk("", &tree, &mut keys);
        keys
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn default_is_valid() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn rejects_out_of_range_settings() {
        let mut config = Config::default();
        config.requests.chunks = 12;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::NotRequestSize { value: 12, .. })
        ));
        let mut config = Config::default();
        config.upload.parallel_responses = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::Zero("upload.parallel_responses"))
        );
        let mut config = Config::default();
        config.lingering.ratio = f64::NAN;
        assert_eq!(
            config.validate(),
            Err(ConfigError::NotPositive("lingering.ratio"))
        );
    }

    #[test]
    fn settings_are_overridden_by_dotted_key() {
        let mut config = Config::default();
        config
            .set("strategies.chunk_selection", json!("random"))
            .unwrap();
        config.set("strategies.upload", json!("propshare")).unwrap();
        config.set("upload.review_interval", json!("2.5s")).unwrap();
        config.set("connections.redial_delay", json!(5)).unwrap();
        assert_eq!(config.strategies.chunk_selection, ChunkSelection::Random);
        assert_eq!(config.strategies.upload, Upload::Propshare);
        assert_eq!(config.upload.review_interval, Duration::from_millis(2500));
        assert_eq!(config.connections.redial_delay, Duration::from_secs(5));
        assert_eq!(config.get("upload.review_interval"), Some(json!("2500ms")));
    }

    #[test]
    fn bad_overrides_leave_the_config_unchanged() {
        let mut config = Config::default();
        let before = config.clone();
        assert_eq!(
            config.set("strategies.nope", json!(1)),
            Err(ConfigError::UnknownSetting("strategies.nope".to_owned()))
        );
        assert_eq!(
            config.set("strategies", json!({})),
            Err(ConfigError::UnknownSetting("strategies".to_owned()))
        );
        assert!(matches!(
            config.set("strategies.upload", json!("tit-for-tat")),
            Err(ConfigError::InvalidValue { .. })
        ));
        assert!(matches!(
            config.set("requests.chunks", json!(3)),
            Err(ConfigError::NotRequestSize { value: 3, .. })
        ));
        assert_eq!(config, before);
    }

    #[test]
    fn every_key_reads_and_writes_back() {
        let keys = Config::keys();
        assert!(keys.contains(&"strategies.chunk_selection".to_owned()));
        assert!(keys.contains(&"lingering.ratio".to_owned()));
        let mut config = Config::default();
        for key in keys {
            let value = config.get(&key).unwrap();
            config.set(&key, value).unwrap();
        }
        assert_eq!(config, Config::default());
    }

    #[test]
    fn settings_resolve_a_profile_with_overrides() {
        let settings: Settings = serde_json::from_value(json!({
            "profile": "cooperative",
            "overrides": { "endgame.extra_copies": 5 },
        }))
        .unwrap();
        let config = settings.resolve().unwrap();
        assert_eq!(config.strategies.upload, Upload::Cooperative);
        assert_eq!(config.endgame.extra_copies, 5);
    }
}
