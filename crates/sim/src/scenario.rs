//! Scenario files: the world to simulate and the experiment to run in it (D-072).
//!
//! One YAML file describes the bundle, the network, the classes of peers and how they arrive and
//! leave, the settings every peer starts from, and the experiment: a grid of settings to vary
//! and how many seeds to run each variant with. [`Scenario::from_yaml`] reads and checks one.
//! The field names are documented on the types below; `crates/sim/scenarios/` has examples.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempest_core::config::{Config, ConfigError, Settings};

use crate::bundle::BundleSpec;
use crate::units::{Fraction, Rate, duration};

/// The most peers a scenario may have.
pub const MAX_PEERS: u64 = 1_000_000;

/// The most variants an experiment may have.
pub const MAX_VARIANTS: u64 = 10_000;

/// The most bytes all peers together may hold in real payload mode.
pub const MAX_REAL_BYTES: u64 = 1 << 30;

/// A scenario and the experiment to run in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// A short name.
    pub name: String,
    /// What the scenario is for.
    #[serde(default)]
    pub description: String,
    /// The longest a run may last, in simulated time. A run ends sooner once every peer that
    /// started without the bundle has completed or left. Default 1 hour.
    #[serde(default = "default_duration", with = "duration")]
    pub duration: Duration,
    /// How many seeds to run each variant with: seeds 1 to `seeds`. Default 1.
    #[serde(default = "one")]
    pub seeds: u32,
    /// Whether peers exchange real bytes or only their sizes. Default `virtual`.
    #[serde(default)]
    pub payload: Payload,
    /// The bundle every peer shares.
    pub bundle: BundleSpec,
    /// Defaults for every peer's link.
    #[serde(default)]
    pub network: NetworkSpec,
    /// The simulated tracker.
    #[serde(default)]
    pub tracker: TrackerSpec,
    /// The profile and overrides every peer starts from.
    #[serde(default)]
    pub settings: Settings,
    /// The classes of peers.
    pub peers: Vec<PeerClass>,
    /// The variants to compare.
    #[serde(default)]
    pub experiment: Experiment,
}

/// Whether simulated peers exchange real bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Payload {
    /// Messages carry sizes only. The simulator decides whether Data passes verification: it
    /// fails only when sent by a bad-data peer. Scales to large swarms.
    #[default]
    Virtual,
    /// Peers hold the bundle's bytes, send real Data with proofs and verify everything they
    /// receive. For small scenarios.
    Real,
}

/// Defaults for every peer's link to the rest of the network.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkSpec {
    /// One-way latency from a peer to the core of the network. Two peers are this far from
    /// each other twice over: once at each end. Default 20 ms.
    #[serde(with = "duration")]
    pub latency: Duration,
    /// The chance that a peer's link loses a message. Default 0.
    pub loss: Fraction,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(20),
            loss: Fraction::default(),
        }
    }
}

/// The simulated tracker every peer announces to (`protocol/trackers.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TrackerSpec {
    /// How many peers, chosen at random, each announce returns. Default 50.
    pub peers: u32,
    /// How often a peer announces again. Default 5 minutes.
    #[serde(with = "duration")]
    pub interval: Duration,
}

impl Default for TrackerSpec {
    fn default() -> Self {
        Self {
            peers: 50,
            interval: Duration::from_secs(300),
        }
    }
}

/// A group of peers alike in everything but their randomness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerClass {
    /// The class's name, unique in the scenario.
    pub class: String,
    /// How many peers. Default 1.
    #[serde(default = "one")]
    pub count: u32,
    /// Upload and download capacity.
    #[serde(default)]
    pub bandwidth: Bandwidth,
    /// One-way latency to the core of the network, instead of the network's default.
    #[serde(
        default,
        with = "duration::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub latency: Option<Duration>,
    /// Chance of losing a message, instead of the network's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss: Option<Fraction>,
    /// Whether the peers start with the whole bundle.
    #[serde(default)]
    pub starts: Start,
    /// How the peers behave.
    #[serde(default)]
    pub behavior: Behavior,
    /// When the peers arrive. Default: all at the start.
    #[serde(default)]
    pub arrivals: Arrivals,
    /// When the peers leave early, if they do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub churn: Option<Churn>,
    /// Settings for this class only, by dotted key. They apply after the scenario's settings
    /// and the variant's.
    #[serde(default)]
    pub overrides: BTreeMap<String, Value>,
}

/// A peer's link capacity.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bandwidth {
    /// Upload capacity. Default unlimited.
    pub up: Rate,
    /// Download capacity. Default unlimited.
    pub down: Rate,
}

/// What a peer has when it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    /// Only the bundle ID: it fetches the manifest, then the chunks.
    #[default]
    Empty,
    /// The whole bundle, like its origin.
    Complete,
}

/// How a peer behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Behavior {
    /// It follows the protocol and its settings.
    #[default]
    Normal,
    /// It downloads but never uploads: it answers every chunk request with a `busy` Reject.
    FreeRider,
    /// Every Data it sends fails verification.
    BadData,
}

/// When the peers of a class arrive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "pattern", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Arrivals {
    /// All at once.
    AtOnce {
        /// When. Default 0.
        #[serde(default, with = "duration")]
        at: Duration,
    },
    /// One every `interval`, the first at `start`.
    Steady {
        /// When the first arrives. Default 0.
        #[serde(default, with = "duration")]
        start: Duration,
        /// The time between arrivals.
        #[serde(with = "duration")]
        interval: Duration,
    },
    /// A flash crowd: arrivals start at `start` and fall off exponentially, with a mean delay
    /// of `decay`.
    FlashCrowd {
        /// When the crowd starts. Default 0.
        #[serde(default, with = "duration")]
        start: Duration,
        /// The mean time from the start to an arrival.
        #[serde(with = "duration")]
        decay: Duration,
    },
}

impl Default for Arrivals {
    fn default() -> Self {
        Self::AtOnce { at: Duration::ZERO }
    }
}

/// Peers leaving before they would otherwise stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Churn {
    /// The mean time a peer stays, drawn from an exponential distribution. A peer that stops
    /// sharing sooner (`lingering`) leaves sooner.
    #[serde(with = "duration")]
    pub session: Duration,
}

/// The variants to compare.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Experiment {
    /// Settings to vary, by dotted key, each with the values to try. Every combination is a
    /// variant.
    pub vary: BTreeMap<String, Vec<Value>>,
}

/// One combination of the experiment's values.
#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    /// Its position in [`Scenario::variants`].
    pub index: usize,
    /// The value of each varied setting.
    pub values: BTreeMap<String, Value>,
}

impl Variant {
    /// A readable name, such as `strategies.upload=propshare`, or `default` if nothing varies.
    pub fn name(&self) -> String {
        if self.values.is_empty() {
            return "default".to_owned();
        }
        let parts: Vec<String> = self
            .values
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}={text}"),
                other => format!("{key}={other}"),
            })
            .collect();
        parts.join(" ")
    }
}

/// Why a scenario file can't be used.
#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    /// The file couldn't be read.
    #[error("can't read {path}: {source}")]
    Read {
        /// The file.
        path: String,
        /// Why.
        source: std::io::Error,
    },
    /// The file isn't a well-formed scenario.
    #[error("{0}")]
    Parse(String),
    /// A field's value is unusable.
    #[error("{field}: {message}")]
    Invalid {
        /// Where, as a path such as `peers[1].count`.
        field: String,
        /// What is wrong.
        message: String,
    },
}

fn invalid(field: impl Into<String>, message: impl Into<String>) -> ScenarioError {
    ScenarioError::Invalid {
        field: field.into(),
        message: message.into(),
    }
}

fn one() -> u32 {
    1
}

fn default_duration() -> Duration {
    Duration::from_secs(3600)
}

impl Scenario {
    /// Reads and checks a scenario file.
    pub fn load(path: &Path) -> Result<Self, ScenarioError> {
        Self::load_text(path).map(|(scenario, _)| scenario)
    }

    /// Reads and checks a scenario file, returning its text too.
    pub fn load_text(path: &Path) -> Result<(Self, String), ScenarioError> {
        let text = std::fs::read_to_string(path).map_err(|source| ScenarioError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let scenario = Self::from_yaml(&text).map_err(|error| match error {
            ScenarioError::Parse(message) => {
                ScenarioError::Parse(format!("{}: {message}", path.display()))
            }
            other => other,
        })?;
        Ok((scenario, text))
    }

    /// Reads and checks a scenario from YAML text.
    pub fn from_yaml(text: &str) -> Result<Self, ScenarioError> {
        // YAML 1.2 booleans only, so that values such as `off` stay strings.
        let options = serde_saphyr::options! { strict_booleans: true };
        let scenario: Self = serde_saphyr::from_str_with_options(text, options)
            .map_err(|e| ScenarioError::Parse(e.to_string()))?;
        scenario.validate()?;
        Ok(scenario)
    }

    /// Checks everything that parsing alone doesn't: counts, limits, and that every setting
    /// resolves for every class in every variant.
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if self.seeds == 0 {
            return Err(invalid("seeds", "must be at least 1"));
        }
        if self.peers.is_empty() {
            return Err(invalid("peers", "needs at least one class"));
        }
        let bundle = self.bundle.build().map_err(|m| invalid("bundle", m))?;
        let peers = self.peer_count();
        if peers > MAX_PEERS {
            return Err(invalid(
                "peers",
                format!("{peers} peers is more than the limit of {MAX_PEERS}"),
            ));
        }
        if self.payload == Payload::Real
            && bundle.total_size().saturating_mul(peers) > MAX_REAL_BYTES
        {
            return Err(invalid(
                "payload",
                format!(
                    "real payloads need the bundle's bytes for every peer: \
                     {peers} × {} bytes is more than {MAX_REAL_BYTES}; use `virtual`",
                    bundle.total_size()
                ),
            ));
        }
        let mut variants: u64 = 1;
        for (key, values) in &self.experiment.vary {
            if values.is_empty() {
                return Err(invalid(
                    format!("experiment.vary.{key}"),
                    "needs at least one value",
                ));
            }
            variants = variants.saturating_mul(values.len() as u64);
        }
        if variants > MAX_VARIANTS {
            return Err(invalid(
                "experiment.vary",
                format!("{variants} variants is more than the limit of {MAX_VARIANTS}"),
            ));
        }
        for (i, class) in self.peers.iter().enumerate() {
            let field = |name: &str| format!("peers[{i}].{name}");
            if self.peers[..i].iter().any(|c| c.class == class.class) {
                return Err(invalid(
                    field("class"),
                    format!("{:?} is used twice", class.class),
                ));
            }
            if let Arrivals::Steady { interval, .. } = class.arrivals
                && interval.is_zero()
            {
                return Err(invalid(field("arrivals.interval"), "must be more than 0"));
            }
            if let Some(churn) = class.churn
                && churn.session.is_zero()
            {
                return Err(invalid(field("churn.session"), "must be more than 0"));
            }
        }
        // Every class's settings must resolve in every variant. Check each source on its own
        // first, so the error names the field at fault.
        let base = self
            .settings
            .resolve()
            .map_err(|e| config_error("settings", e))?;
        for (key, values) in &self.experiment.vary {
            for value in values {
                base.clone()
                    .set(key, value.clone())
                    .map_err(|e| config_error(&format!("experiment.vary.{key}"), e))?;
            }
        }
        for i in 0..self.peers.len() {
            for variant in self.variants() {
                self.config(i, &variant)
                    .map_err(|e| config_error(&format!("peers[{i}].overrides"), e))?;
            }
        }
        Ok(())
    }

    /// The total number of peers.
    pub fn peer_count(&self) -> u64 {
        self.peers.iter().map(|c| u64::from(c.count)).sum()
    }

    /// Every combination of the experiment's values, in a fixed order.
    pub fn variants(&self) -> Vec<Variant> {
        let mut combos = vec![BTreeMap::new()];
        for (key, values) in &self.experiment.vary {
            combos = combos
                .into_iter()
                .flat_map(|combo| {
                    values.iter().map(move |value| {
                        let mut combo = combo.clone();
                        combo.insert(key.clone(), value.clone());
                        combo
                    })
                })
                .collect();
        }
        combos
            .into_iter()
            .enumerate()
            .map(|(index, values)| Variant { index, values })
            .collect()
    }

    /// The scenario as `variant` runs it: its values become overrides, and nothing varies.
    pub fn resolve(&self, variant: &Variant) -> Self {
        let mut resolved = self.clone();
        resolved.settings.overrides.extend(variant.values.clone());
        resolved.experiment = Experiment::default();
        resolved
    }

    /// The config of class `class`'s peers in `variant`: the profile, then the scenario's
    /// overrides, then the variant's values, then the class's overrides.
    pub fn config(&self, class: usize, variant: &Variant) -> Result<Config, ConfigError> {
        let mut config = self.settings.resolve()?;
        for (key, value) in variant.values.iter().chain(&self.peers[class].overrides) {
            config.set(key, value.clone())?;
        }
        Ok(config)
    }
}

fn config_error(field: &str, error: ConfigError) -> ScenarioError {
    invalid(field, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "
name: two-peers
bundle:
  files: [{ size: 1MiB }]
peers:
  - class: origin
    starts: complete
  - class: leecher
";

    #[test]
    fn a_minimal_scenario_takes_the_defaults() {
        let scenario = Scenario::from_yaml(MINIMAL).unwrap();
        assert_eq!(scenario.seeds, 1);
        assert_eq!(scenario.duration, Duration::from_secs(3600));
        assert_eq!(scenario.payload, Payload::Virtual);
        assert_eq!(scenario.peer_count(), 2);
        assert_eq!(scenario.peers[0].starts, Start::Complete);
        assert_eq!(scenario.peers[1].bandwidth.up, Rate::UNLIMITED);
        assert_eq!(scenario.variants().len(), 1);
        assert_eq!(scenario.variants()[0].name(), "default");
    }

    #[test]
    fn a_full_scenario_reads() {
        let scenario = Scenario::from_yaml(
            "
name: flash-crowd
description: A flash crowd on one seed.
duration: 2h
seeds: 20
payload: virtual
bundle:
  files:
    - { size: 64MiB }
    - { size: 1MiB, count: 3 }
network: { latency: 30ms, loss: 0.5% }
tracker: { peers: 30, interval: 10m }
settings:
  profile: balanced
  overrides: { connections.max: 40 }
peers:
  - class: home
    count: 800
    bandwidth: { down: 200Mbps, up: 20Mbps }
    churn: { session: 40m }
    arrivals: { pattern: flash-crowd, decay: 15m }
  - class: free-rider
    count: 50
    behavior: free-rider
    arrivals: { pattern: steady, start: 1m, interval: 2s }
  - class: origin
    bandwidth: { down: 1Gbps, up: 50Mbps }
    latency: 5ms
    starts: complete
    overrides: { strategies.superseed: true }
experiment:
  vary:
    strategies.chunk_selection: [rarest-first, random]
    strategies.upload: [bittorrent, propshare, fairtorrent]
",
        )
        .unwrap();
        assert_eq!(scenario.network.loss.get(), 0.005);
        assert_eq!(
            scenario.peers[0].arrivals,
            Arrivals::FlashCrowd {
                start: Duration::ZERO,
                decay: Duration::from_secs(900)
            }
        );
        let variants = scenario.variants();
        assert_eq!(variants.len(), 6);
        assert_eq!(
            variants[1].name(),
            "strategies.chunk_selection=rarest-first strategies.upload=propshare"
        );
        let origin = scenario.config(2, &variants[5]).unwrap();
        assert!(origin.strategies.superseed);
        assert_eq!(origin.connections.max, 40);
        assert_eq!(
            origin.get("strategies.upload"),
            Some(Value::String("fairtorrent".into()))
        );
    }

    fn error(yaml: &str) -> String {
        Scenario::from_yaml(yaml).unwrap_err().to_string()
    }

    #[test]
    fn bad_files_say_what_is_wrong() {
        let e = error(&MINIMAL.replace("name:", "nmae:"));
        assert!(e.contains("unknown field `nmae`"), "{e}");
        assert!(e.contains("line 2"), "{e}");
        let e = error(&MINIMAL.replace("1MiB", "1 lots"));
        assert!(e.contains("unknown unit"), "{e}");
        let e = error(&format!(
            "{MINIMAL}experiment: {{ vary: {{ strategies.upload: [tit-for-tat] }} }}"
        ));
        assert!(e.starts_with("experiment.vary.strategies.upload:"), "{e}");
        let e = error(&format!(
            "{MINIMAL}experiment: {{ vary: {{ requests.nope: [1] }} }}"
        ));
        assert!(e.contains("no setting \"requests.nope\""), "{e}");
        let e = error(&format!("{MINIMAL}    overrides: {{ requests.chunks: 3 }}"));
        assert!(e.starts_with("peers[1].overrides:"), "{e}");
        let e = error(&MINIMAL.replace("class: leecher", "class: origin"));
        assert!(e.starts_with("peers[1].class:"), "{e}");
        let e = error(&format!("{MINIMAL}seeds: 0"));
        assert!(e.starts_with("seeds:"), "{e}");
        let e = error(&format!("{MINIMAL}payload: real\n").replace("1MiB", "1GiB"));
        assert!(e.starts_with("payload:"), "{e}");
        let e = error(&MINIMAL.replace(
            "class: leecher",
            "class: leecher\n    arrivals: { pattern: steady }",
        ));
        assert!(e.contains("missing field `interval`"), "{e}");
    }

    #[test]
    fn yaml_words_stay_strings() {
        let scenario = Scenario::from_yaml(&format!(
            "{MINIMAL}experiment: {{ vary: {{ strategies.endgame: [off, limited, classic] }} }}"
        ))
        .unwrap();
        assert_eq!(scenario.variants()[0].values["strategies.endgame"], "off");
    }
}
