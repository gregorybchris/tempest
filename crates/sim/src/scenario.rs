//! Scenario files: the world to simulate and the experiment to run in it (D-072).
//!
//! One YAML file describes the bundle, the network, the classes of peers and how they arrive and
//! leave, the settings every peer starts from, and the experiment: a grid of settings to vary
//! and how many seeds to run each variant with. [`Scenario::from_yaml`] reads and checks one.
//! The field names are documented on the types below; `crates/sim/scenarios/` has examples.

use std::borrow::Cow;
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
    /// How often a run samples how many peers hold each chunk. Default 10 seconds.
    #[serde(default = "default_sample_interval", with = "duration")]
    pub sample_interval: Duration,
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
    /// The relays that peers behind routers register with.
    #[serde(default)]
    pub relay: RelaySpec,
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

/// The relays that peers behind routers register with (`protocol/relays.md`).
///
/// Every peer with a home relay has one with these properties. A relayed connection detours
/// through the relay: its messages take the relay's latency twice more each way, and the relay
/// forwards each direction of each relayed connection at up to `bandwidth`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RelaySpec {
    /// One-way latency from a relay to the core of the network. Default 20 ms.
    #[serde(with = "duration")]
    pub latency: Duration,
    /// The most a relay forwards each way on one relayed connection. Default 10 Mbps.
    pub bandwidth: Rate,
}

impl Default for RelaySpec {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(20),
            bandwidth: Rate::bytes_per_second(10e6 / 8.0),
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
    /// The router the peers sit behind, if they can't be dialed directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nat: Option<Nat>,
    /// How fast the peers write verified chunks to disk. Default unlimited.
    #[serde(default)]
    pub disk: Rate,
    /// How fast the peers hash received chunks to verify them. Default unlimited.
    #[serde(default)]
    pub hashing: Rate,
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

/// A router that drops incoming connections a peer didn't ask for (network address
/// translation).
///
/// Nobody can dial such a peer directly. A peer with a home relay is dialed through it, and the
/// two peers then try to hole-punch a direct connection (`protocol/relays.md`). Punching works
/// when the dialing peer is directly reachable, or when both routers are punchable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Nat {
    /// The chance that a peer's router keeps one public port for every destination, so that
    /// hole punching through it works. Default 85%, so that about 72% of pairs of such peers
    /// can punch.
    pub punchable: Fraction,
    /// Whether the peers keep a home relay. Without one they can only dial out. Default true.
    pub relay: bool,
}

impl Default for Nat {
    fn default() -> Self {
        Self {
            punchable: Fraction::new(0.85).expect("a fraction"),
            relay: true,
        }
    }
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
    /// A BitTyrant-style strategic client: while downloading it keeps more connections and
    /// buys download with as little upload as it can (`strategic` module).
    Strategic,
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
    /// A Poisson process: arrivals start at `start`, with exponential gaps of mean `interval`.
    Poisson {
        /// When the first gap starts. Default 0.
        #[serde(default, with = "duration")]
        start: Duration,
        /// The mean time between arrivals.
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Churn {
    /// The mean time a peer stays. A peer that stops sharing sooner (`lingering`) leaves
    /// sooner.
    #[serde(with = "duration")]
    pub session: Duration,
    /// The shape of the Weibull distribution session lengths are drawn from. 1, the default,
    /// is the exponential distribution; below 1, most sessions are short and a few are very
    /// long, as measured in deployed swarms.
    #[serde(default = "unit_shape")]
    pub shape: f64,
}

fn unit_shape() -> f64 {
    1.0
}

/// The variants to compare.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Experiment {
    /// What to vary, by dotted key, each with the values to try. Every combination is a
    /// variant. A key is a setting for every peer, such as `strategies.upload`, or a field of
    /// one peer class, `peers.<class>.<field>`: `peers.origin.count`, `peers.home.bandwidth.up`,
    /// or a setting for that class alone, such as `peers.origin.settings.strategies.superseed`.
    pub vary: BTreeMap<String, Vec<Value>>,
}

/// The prefix of a varied key that addresses a peer class.
const CLASS_KEY: &str = "peers.";

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

fn default_sample_interval() -> Duration {
    Duration::from_secs(10)
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
        if self.sample_interval.is_zero() {
            return Err(invalid("sample_interval", "must be more than 0"));
        }
        if self.tracker.interval.is_zero() {
            return Err(invalid("tracker.interval", "must be more than 0"));
        }
        if self.peers.is_empty() {
            return Err(invalid("peers", "needs at least one class"));
        }
        let bundle = self.bundle.build().map_err(|m| invalid("bundle", m))?;
        let peers = self.most_peers();
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
            if key.starts_with(CLASS_KEY) && self.class_key(key).is_none() {
                return Err(invalid(
                    format!("experiment.vary.{key}"),
                    "names no peer class: use peers.<class>.<field>",
                ));
            }
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
        // A link that loses every message never delivers one.
        let lossless = |loss: Fraction| loss.get() < 1.0;
        if !lossless(self.network.loss) {
            return Err(invalid("network.loss", "must be below 100%"));
        }
        for (i, class) in self.peers.iter().enumerate() {
            let field = |name: &str| format!("peers[{i}].{name}");
            if !class.loss.is_none_or(lossless) {
                return Err(invalid(field("loss"), "must be below 100%"));
            }
            if self.peers[..i].iter().any(|c| c.class == class.class) {
                return Err(invalid(
                    field("class"),
                    format!("{:?} is used twice", class.class),
                ));
            }
            if let Arrivals::Steady { interval, .. } | Arrivals::Poisson { interval, .. } =
                class.arrivals
                && interval.is_zero()
            {
                return Err(invalid(field("arrivals.interval"), "must be more than 0"));
            }
            if let Some(churn) = class.churn {
                if churn.session.is_zero() {
                    return Err(invalid(field("churn.session"), "must be more than 0"));
                }
                if !(churn.shape.is_finite() && churn.shape >= 0.05) {
                    return Err(invalid(field("churn.shape"), "must be at least 0.05"));
                }
            }
        }
        // Every class's settings must resolve in every variant. Check each source on its own
        // first, so the error names the field at fault.
        let base = self
            .settings
            .resolve()
            .map_err(|e| config_error("settings", e))?;
        for (key, values) in &self.experiment.vary {
            let field = format!("experiment.vary.{key}");
            for value in values {
                let Some((class, path)) = self.class_key(key) else {
                    base.clone()
                        .set(key, value.clone())
                        .map_err(|e| config_error(&field, e))?;
                    continue;
                };
                if let Some(setting) = path.strip_prefix("settings.") {
                    base.clone()
                        .set(setting, value.clone())
                        .map_err(|e| config_error(&field, e))?;
                }
                // The class must still make sense with the new value.
                let mut changed = self.clone();
                changed.experiment = Experiment::default();
                changed.peers[class] = set_class_field(&self.peers[class], path, value)
                    .map_err(|m| invalid(&field, m))?;
                changed
                    .validate()
                    .map_err(|e| invalid(&field, e.to_string()))?;
            }
        }
        // Values for the same class must also fit together, such as a whole `arrivals` and one
        // of its fields.
        if self.varies_classes() {
            for variant in self.variants() {
                self.try_resolve(&variant)?;
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

    /// Whether the experiment varies any class's fields.
    fn varies_classes(&self) -> bool {
        self.experiment
            .vary
            .keys()
            .any(|key| key.starts_with(CLASS_KEY))
    }

    /// The class a varied key addresses, and the field of it, for keys of the form
    /// `peers.<class>.<field>`. The longest matching class name wins.
    pub fn class_key<'k>(&self, key: &'k str) -> Option<(usize, &'k str)> {
        let rest = key.strip_prefix(CLASS_KEY)?;
        self.peers
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let field = rest.strip_prefix(c.class.as_str())?.strip_prefix('.')?;
                (!field.is_empty()).then_some((i, c.class.len(), field))
            })
            .max_by_key(|&(_, len, _)| len)
            .map(|(i, _, field)| (i, field))
    }

    /// The most peers any variant has.
    pub fn most_peers(&self) -> u64 {
        self.slots().iter().map(|&n| u64::from(n)).sum()
    }

    /// How many peers each class has room for: the most it has in any variant. A run draws
    /// every slot's randomness whether or not the slot holds a peer, so that varying one
    /// class's count leaves the other classes' peers as they were (D-073).
    pub fn slots(&self) -> Vec<u32> {
        let mut slots: Vec<u32> = self.peers.iter().map(|c| c.count).collect();
        for (key, values) in &self.experiment.vary {
            if let Some((class, "count")) = self.class_key(key) {
                for value in values {
                    let count = value.as_u64().and_then(|n| u32::try_from(n).ok());
                    slots[class] = slots[class].max(count.unwrap_or(0));
                }
            }
        }
        slots
    }

    /// The peer classes as variant `variant` (an index into [`Scenario::variants`]) has them.
    pub fn classes(&self, variant: usize) -> Cow<'_, [PeerClass]> {
        if !self.varies_classes() {
            return Cow::Borrowed(&self.peers);
        }
        match self.variants().get(variant) {
            Some(variant) => Cow::Owned(self.resolve(variant).peers),
            None => Cow::Borrowed(&self.peers),
        }
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

    /// The scenario as `variant` runs it, with nothing left to vary: its settings become the
    /// scenario's overrides and its class fields are set on their classes, a class's settings
    /// among its overrides.
    ///
    /// The variant must be one of this scenario's, which [`Scenario::validate`] has checked.
    pub fn resolve(&self, variant: &Variant) -> Self {
        self.try_resolve(variant)
            .expect("a validated scenario's class fields")
    }

    /// [`Scenario::resolve`], or why a class field's value doesn't fit with the variant's
    /// others.
    fn try_resolve(&self, variant: &Variant) -> Result<Self, ScenarioError> {
        let mut resolved = self.clone();
        resolved.experiment = Experiment::default();
        for (key, value) in &variant.values {
            match self.class_key(key) {
                Some((class, path)) => {
                    resolved.peers[class] = set_class_field(&resolved.peers[class], path, value)
                        .map_err(|m| invalid(format!("experiment.vary.{key}"), m))?;
                }
                None => {
                    resolved
                        .settings
                        .overrides
                        .insert(key.clone(), value.clone());
                }
            }
        }
        Ok(resolved)
    }

    /// The config of class `class`'s peers in `variant`: the profile, then the scenario's
    /// overrides, then the variant's settings for every class, then the class's overrides with
    /// the variant's settings for this class among them.
    pub fn config(&self, class: usize, variant: &Variant) -> Result<Config, ConfigError> {
        let mut config = self.settings.resolve()?;
        let mut overrides = self.peers[class].overrides.clone();
        for (key, value) in &variant.values {
            match self.class_key(key) {
                Some((c, path)) => {
                    if let Some(setting) = path.strip_prefix("settings.")
                        && c == class
                    {
                        overrides.insert(setting.to_owned(), value.clone());
                    }
                }
                None => config.set(key, value.clone())?,
            }
        }
        for (key, value) in overrides {
            config.set(&key, value)?;
        }
        Ok(config)
    }
}

/// `class` with the field at dotted `path` set to `value`. `settings.<key>` sets the class's
/// override of setting `<key>`.
fn set_class_field(class: &PeerClass, path: &str, value: &Value) -> Result<PeerClass, String> {
    let mut class = class.clone();
    if let Some(setting) = path.strip_prefix("settings.") {
        class.overrides.insert(setting.to_owned(), value.clone());
        return Ok(class);
    }
    let first = path.split('.').next().unwrap_or_default();
    if matches!(first, "class" | "overrides" | "settings") {
        return Err(format!(
            "`{first}` can't be varied; vary a class's settings as `settings.<key>`"
        ));
    }
    let mut json = serde_json::to_value(&class).map_err(|e| e.to_string())?;
    let mut node = &mut json;
    for part in path.split('.') {
        // An absent optional field, such as `nat`, takes its defaults.
        if node.is_null() {
            *node = Value::Object(serde_json::Map::new());
        }
        let Value::Object(map) = node else {
            return Err(format!("`{path}` is not a field of a peer class"));
        };
        node = map.entry(part).or_insert(Value::Null);
    }
    *node = value.clone();
    serde_json::from_value(json).map_err(|e| e.to_string())
}

fn config_error(field: &str, error: ConfigError) -> ScenarioError {
    invalid(field, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempest_core::config::Superseed;

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
        assert_eq!(scenario.sample_interval, Duration::from_secs(10));
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
    overrides: { strategies.superseed: on }
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
        assert_eq!(origin.strategies.superseed, Superseed::On);
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
        let e = error(&format!("{MINIMAL}sample_interval: 0s"));
        assert!(e.starts_with("sample_interval:"), "{e}");
        let e = error(&format!("{MINIMAL}tracker: {{ interval: 0s }}"));
        assert!(e.starts_with("tracker.interval:"), "{e}");
        let e = error(&format!("{MINIMAL}payload: real\n").replace("1MiB", "1GiB"));
        assert!(e.starts_with("payload:"), "{e}");
        let e = error(&MINIMAL.replace(
            "class: leecher",
            "class: leecher\n    arrivals: { pattern: steady }",
        ));
        assert!(e.contains("missing field `interval`"), "{e}");
        let e = error(&format!("{MINIMAL}    churn: {{ session: 1m, shape: 0 }}"));
        assert!(e.starts_with("peers[1].churn.shape:"), "{e}");
        let e = error(&format!(
            "{MINIMAL}    arrivals: {{ pattern: poisson, interval: 0s }}"
        ));
        assert!(e.starts_with("peers[1].arrivals.interval:"), "{e}");
        let e = error(&format!("{MINIMAL}    loss: 100%"));
        assert!(e.starts_with("peers[1].loss:"), "{e}");
        let e = error(&format!("{MINIMAL}network: {{ loss: 1 }}"));
        assert!(e.starts_with("network.loss:"), "{e}");
        let e = error(&format!("{MINIMAL}    nat: {{ punchable: 2 }}"));
        assert!(e.contains("not a fraction"), "{e}");
    }

    #[test]
    fn routers_relays_disks_and_sessions_read() {
        let scenario = Scenario::from_yaml(&format!(
            "{MINIMAL}    nat: {{ punchable: 40% }}
    disk: 50MB/s
    hashing: 400MB/s
    arrivals: {{ pattern: poisson, interval: 3s }}
    churn: {{ session: 20m, shape: 0.5 }}
  - class: hidden
    nat: {{ relay: false }}
relay: {{ latency: 35ms, bandwidth: 20Mbps }}
"
        ))
        .unwrap();
        let leecher = &scenario.peers[1];
        assert_eq!(leecher.nat.unwrap().punchable.get(), 0.4);
        assert!(leecher.nat.unwrap().relay);
        assert_eq!(leecher.disk.get(), 50e6);
        assert_eq!(leecher.hashing.get(), 400e6);
        assert_eq!(
            leecher.arrivals,
            Arrivals::Poisson {
                start: Duration::ZERO,
                interval: Duration::from_secs(3)
            }
        );
        assert_eq!(leecher.churn.unwrap().shape, 0.5);
        let hidden = scenario.peers[2].nat.unwrap();
        assert!(!hidden.relay);
        assert_eq!(hidden.punchable.get(), 0.85);
        assert_eq!(scenario.peers[0].nat, None);
        assert_eq!(scenario.relay.latency, Duration::from_millis(35));
        assert_eq!(scenario.relay.bandwidth.get(), 2.5e6);

        let defaults = Scenario::from_yaml(MINIMAL).unwrap();
        assert_eq!(defaults.relay, RelaySpec::default());
        assert_eq!(defaults.peers[1].disk, Rate::UNLIMITED);
    }

    #[test]
    fn yaml_words_stay_strings() {
        let scenario = Scenario::from_yaml(&format!(
            "{MINIMAL}experiment: {{ vary: {{ strategies.endgame: [off, limited, classic] }} }}"
        ))
        .unwrap();
        assert_eq!(scenario.variants()[0].values["strategies.endgame"], "off");
    }

    #[test]
    fn experiments_vary_class_fields() {
        let scenario = Scenario::from_yaml(&format!(
            "{MINIMAL}    overrides: {{ strategies.upload: propshare }}
experiment:
  vary:
    peers.origin.settings.strategies.superseed: [off, on]
    peers.leecher.count: [1, 5]
    peers.leecher.bandwidth.up: [10Mbps]
    peers.leecher.settings.strategies.upload: [cooperative]
    peers.leecher.nat.punchable: [50%]
    strategies.endgame: [classic]
"
        ))
        .unwrap();
        assert_eq!(scenario.slots(), vec![1, 5]);
        assert_eq!(scenario.most_peers(), 6);
        let variants = scenario.variants();
        assert_eq!(variants.len(), 4);
        let last = &variants[3];
        let origin = scenario.config(0, last).unwrap();
        assert_eq!(origin.strategies.superseed, Superseed::On);
        assert_eq!(
            origin.get("strategies.endgame"),
            Some(Value::String("classic".into()))
        );
        // The variant's setting for the class wins over the class's own.
        let leecher = scenario.config(1, last).unwrap();
        assert_eq!(leecher.strategies.superseed, Superseed::Crowd);
        assert_eq!(
            leecher.get("strategies.upload"),
            Some(Value::String("cooperative".into()))
        );

        let resolved = scenario.resolve(last);
        assert_eq!(resolved.peers[1].count, 5);
        assert_eq!(resolved.peers[1].bandwidth.up.get(), 10e6 / 8.0);
        assert_eq!(resolved.peers[1].nat.unwrap().punchable.get(), 0.5);
        assert!(resolved.experiment.vary.is_empty());
        let plain = &resolved.variants()[0];
        for class in 0..2 {
            assert_eq!(
                scenario.config(class, last).unwrap(),
                resolved.config(class, plain).unwrap()
            );
        }
        assert_eq!(scenario.classes(0)[1].count, 1);
        assert_eq!(scenario.classes(3)[1].count, 5);
    }

    #[test]
    fn bad_class_fields_say_what_is_wrong() {
        let vary = |entry: &str| error(&format!("{MINIMAL}experiment: {{ vary: {{ {entry} }} }}"));
        let e = vary("peers.nobody.count: [2]");
        assert!(e.starts_with("experiment.vary.peers.nobody.count:"), "{e}");
        let e = vary("peers.leecher.colour: [red]");
        assert!(e.contains("unknown field `colour`"), "{e}");
        let e = vary("peers.leecher.class: [other]");
        assert!(e.contains("can't be varied"), "{e}");
        let e = vary("peers.leecher.settings.requests.nope: [1]");
        assert!(e.contains("no setting \"requests.nope\""), "{e}");
        let e = vary("peers.leecher.loss: [100%]");
        assert!(e.contains("must be below 100%"), "{e}");
        let e = vary("peers.leecher.count: [2000000]");
        assert!(e.contains("more than the limit"), "{e}");
        // Each value fits on its own, but not together.
        let e = vary(
            "peers.leecher.arrivals: [{ pattern: steady, interval: 1s }], \
             peers.leecher.arrivals.at: [1s]",
        );
        assert!(
            e.starts_with("experiment.vary.peers.leecher.arrivals.at:"),
            "{e}"
        );
        assert!(e.contains("unknown field `at`"), "{e}");
    }
}
