//! The engine's settings as clients read and change them (`GET` and `PATCH /v1/settings`).
//!
//! The engine's own config type depends on the network stack, so clients that build for the
//! browser read the settings they show into [`SettingsInfo`] instead. Its types skip what they
//! don't name, and the engine's tests check that they read what the engine sends.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempest_protocol::relay::RelayIdentity;

use crate::schedule::Schedule;
use crate::ui::UiSettings;
use crate::units;

pub mod text;

/// `GET /v1/settings`, as far as clients show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsInfo {
    /// The settings in effect.
    pub settings: Settings,
    /// The settings changed through the API rather than the config file, by dotted key.
    pub changed: BTreeMap<String, Value>,
    /// The groups changed since the engine started that apply only once it restarts.
    pub restart_required: Vec<String>,
    /// Whether the alternative speed limits apply now.
    pub alternative_limits_active: bool,
}

/// The settings clients show; the config file's reference documents each.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where downloads go unless a category or the add says otherwise; `None` for the user's
    /// Downloads folder.
    pub save_folder: Option<String>,
    /// How many transfers run at once.
    pub queue: Queue,
    /// Trackers every transfer announces to.
    pub trackers: Vec<String>,
    /// The relays to pick a home relay from.
    pub relays: Relays,
    /// Asking the router to forward the peer port.
    pub port_mapping: Switch,
    /// Finding peers on the local network.
    pub lan: Switch,
    /// Speed limits over every transfer together.
    pub limits: Limits,
    /// When a complete transfer stops sharing.
    pub sharing: Sharing,
    /// The clients' shared look and views (D-195).
    pub ui: UiSettings,
}

/// `queue`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    /// The most downloads running at once, or `None` for no limit.
    pub max_active_downloads: Option<u32>,
    /// The most complete transfers sharing at once, or `None` for no limit.
    pub max_active_uploads: Option<u32>,
}

/// `relays`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Relays {
    /// Relays as `<key>@host:port`.
    pub known: Vec<String>,
    /// Also pick from the relays trackers recommend.
    pub from_trackers: bool,
}

/// `port_mapping` and `lan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Switch {
    /// Whether it is on.
    pub enabled: bool,
}

/// `limits`, in bytes per second, `None` for no limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Data received.
    pub download: Option<u64>,
    /// Data sent.
    pub upload: Option<u64>,
    /// The limits that replace these while on or scheduled.
    pub alternative: Alternative,
}

/// `limits.alternative`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alternative {
    /// Data received.
    pub download: Option<u64>,
    /// Data sent.
    pub upload: Option<u64>,
    /// Switched on, whatever the schedule says.
    pub enabled: bool,
    /// When they apply on their own.
    pub schedule: Option<Schedule>,
}

/// `sharing`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharing {
    /// Bytes sent per byte received.
    pub ratio: Option<f64>,
    /// Time spent sharing while complete, as the engine writes durations: `12h`.
    pub time: Option<String>,
    /// How long no peer may want anything before a finished download stops, as `10m`.
    pub idle_timeout: Option<String>,
    /// What happens at the limit: `stop` or `remove`.
    pub action: String,
}

/// Setting keys clients change, by dotted key as in the config file.
pub mod keys {
    /// The download limit.
    pub const DOWNLOAD: &str = "limits.download";
    /// The upload limit.
    pub const UPLOAD: &str = "limits.upload";
    /// The alternative download limit.
    pub const ALT_DOWNLOAD: &str = "limits.alternative.download";
    /// The alternative upload limit.
    pub const ALT_UPLOAD: &str = "limits.alternative.upload";
    /// Whether the alternative limits are switched on.
    pub const ALT_ENABLED: &str = "limits.alternative.enabled";
    /// The alternative limits' schedule.
    pub const ALT_SCHEDULE: &str = "limits.alternative.schedule";
    /// How many downloads run at once.
    pub const MAX_DOWNLOADS: &str = "queue.max_active_downloads";
    /// How many complete transfers share at once.
    pub const MAX_UPLOADS: &str = "queue.max_active_uploads";
    /// The sharing ratio limit.
    pub const RATIO: &str = "sharing.ratio";
    /// The sharing time limit.
    pub const TIME: &str = "sharing.time";
    /// The sharing idle timeout.
    pub const IDLE_TIMEOUT: &str = "sharing.idle_timeout";
    /// What happens at a sharing limit.
    pub const ACTION: &str = "sharing.action";
    /// The default download folder.
    pub const SAVE_FOLDER: &str = "save_folder";
    /// The trackers every transfer announces to.
    pub const TRACKERS: &str = "trackers";
    /// The known relays.
    pub const RELAYS: &str = "relays.known";
    /// Whether to use relays trackers suggest.
    pub const RELAYS_FROM_TRACKERS: &str = "relays.from_trackers";
    /// Whether to ask the router to forward the peer port.
    pub const PORT_MAPPING: &str = "port_mapping.enabled";
    /// Whether to find peers on the local network.
    pub const LAN: &str = "lan.enabled";
}

/// What kind of value a setting takes, as typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    /// A speed, such as `2M`, or none.
    Limit,
    /// A whole number of transfers, or none.
    Count,
    /// A ratio above 0, or none.
    Ratio,
    /// A duration such as `12h`, or none.
    Duration,
    /// An absolute path, or nothing for the default.
    Folder,
}

impl ValueKind {
    /// The setting's value for `text`, or why it isn't one.
    pub fn parse(self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        let none = text.is_empty() || text.eq_ignore_ascii_case("none");
        match self {
            Self::Limit => units::parse_limit(text).map(|limit| json!(limit)),
            Self::Count if none => Ok(Value::Null),
            Self::Count => match text.parse::<u32>() {
                Ok(count) if count > 0 => Ok(json!(count)),
                _ => Err("Enter a whole number from 1, or none for no limit.".to_owned()),
            },
            Self::Ratio if none => Ok(Value::Null),
            Self::Ratio => match text.parse::<f64>() {
                Ok(ratio) if ratio.is_finite() && ratio > 0.0 => Ok(json!(ratio)),
                _ => {
                    Err("Enter a ratio above 0, such as 2 or 1.5, or none for no limit.".to_owned())
                }
            },
            Self::Duration if none => Ok(Value::Null),
            Self::Duration => Ok(json!(text)),
            Self::Folder if text.is_empty() => Ok(Value::Null),
            Self::Folder if absolute(text) => Ok(json!(text)),
            Self::Folder => {
                Err("Enter an absolute path, or leave it empty for the default.".to_owned())
            }
        }
    }

    /// A line on what to type.
    pub fn help(self) -> &'static str {
        match self {
            Self::Limit => "A speed per second, such as 500K or 2.5M. none for no limit.",
            Self::Count => "How many transfers, such as 3. none for no limit.",
            Self::Ratio => "Times the downloaded size to upload, such as 2. none for no limit.",
            Self::Duration => "A time such as 30m, 12h or 7d. none for no limit.",
            Self::Folder => "An absolute path on the daemon's machine.",
        }
    }
}

/// Whether `path` is absolute on the daemon's machine, which may not be the client's: a Unix
/// path, or a Windows one with a drive or a UNC prefix.
pub fn absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    let windows_drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    path.starts_with('/')
        || path.starts_with("\\\\")
        || windows_drive
        || Path::new(path).is_absolute()
}

/// Why `url` can't be a tracker, if it can't.
pub fn tracker_problem(url: &str) -> Option<&'static str> {
    let url = url.trim();
    let scheme = url.find("://").map(|end| url[..end].to_ascii_lowercase());
    let host = url.split_once("://").map_or("", |(_, rest)| rest);
    match scheme.as_deref() {
        Some("http" | "https") if !host.is_empty() && !host.starts_with('/') => None,
        _ => Some("A tracker is an http:// or https:// URL."),
    }
}

/// Why `relay` can't be a relay identity, if it can't: `<key>@host[:port]`, the key 64 hex
/// characters.
pub fn relay_problem(relay: &str) -> Option<&'static str> {
    relay
        .trim()
        .parse::<RelayIdentity>()
        .is_err()
        .then_some("A relay's key and address: <64 hex characters>@host[:port].")
}

/// Tags as typed: separated by commas, with or without `#`.
pub fn parse_tags(text: &str) -> BTreeSet<String> {
    text.split(',')
        .map(|tag| tag.trim().trim_start_matches('#').trim())
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_typed_as_clients_show_them() {
        assert_eq!(ValueKind::Limit.parse("2.5M"), Ok(json!(2_500_000)));
        assert_eq!(ValueKind::Limit.parse("none"), Ok(Value::Null));
        assert!(ValueKind::Limit.parse("fast").is_err());
        assert_eq!(ValueKind::Count.parse("3"), Ok(json!(3)));
        assert_eq!(ValueKind::Count.parse(""), Ok(Value::Null));
        assert!(ValueKind::Count.parse("0").is_err());
        assert_eq!(ValueKind::Ratio.parse("1.5"), Ok(json!(1.5)));
        assert!(ValueKind::Ratio.parse("-1").is_err());
        assert_eq!(ValueKind::Duration.parse(" 12h "), Ok(json!("12h")));
        assert_eq!(
            ValueKind::Folder.parse("/srv/films"),
            Ok(json!("/srv/films"))
        );
        assert_eq!(ValueKind::Folder.parse(""), Ok(Value::Null));
        assert!(ValueKind::Folder.parse("films").is_err());
    }

    #[test]
    fn engine_paths_may_be_windows_paths() {
        assert!(absolute("/home/a"));
        assert!(absolute("C:\\Users\\a"));
        assert!(absolute("d:/films"));
        assert!(absolute("\\\\server\\share"));
        assert!(!absolute("films"));
        assert!(!absolute("C:films"));
    }

    #[test]
    fn trackers_and_relays_are_checked_as_typed() {
        assert_eq!(tracker_problem("https://t.example.org/announce"), None);
        assert_eq!(tracker_problem("HTTP://10.0.0.2:4777"), None);
        assert!(tracker_problem("udp://t.example.org").is_some());
        assert!(tracker_problem("https://").is_some());
        assert!(tracker_problem("t.example.org").is_some());
        let key = "ab".repeat(32);
        assert_eq!(
            relay_problem(&format!("{key}@relay.example.org:4775")),
            None
        );
        assert_eq!(relay_problem(&format!(" {key}@relay.example.org ")), None);
        assert!(relay_problem(&format!("{key}@relay.example.org:0")).is_some());
        assert!(relay_problem("abc@relay.example.org:4775").is_some());
        assert!(relay_problem("relay.example.org:4775").is_some());
    }

    #[test]
    fn tags_are_split_on_commas_and_trimmed() {
        assert_eq!(
            parse_tags(" music, #live,,  field recordings ,music"),
            BTreeSet::from(["field recordings".into(), "live".into(), "music".into()])
        );
        assert!(parse_tags(" , ").is_empty());
    }

    #[test]
    fn unknown_settings_are_skipped() {
        let info: SettingsInfo = serde_json::from_value(json!({
            "settings": {
                "peers": { "port": 4777 },
                "queue": { "max_active_downloads": 2, "max_active_uploads": null },
                "limits": { "alternative": { "schedule": { "from": "09:00", "to": "17:00" } } },
                "sharing": { "ratio": 2.0, "time": "12h", "idle_timeout": "5m", "action": "remove" },
                "ui": { "theme": "amber" }
            },
            "alternative_limits_active": true,
            "later": 1
        }))
        .unwrap();
        assert_eq!(info.settings.queue.max_active_downloads, Some(2));
        assert_eq!(info.settings.sharing.time.as_deref(), Some("12h"));
        assert_eq!(info.settings.sharing.idle_timeout.as_deref(), Some("5m"));
        assert_eq!(info.settings.ui.theme, "amber");
        let schedule = info.settings.limits.alternative.schedule.unwrap();
        assert_eq!(schedule.days.len(), 7);
        assert!(info.alternative_limits_active);
    }
}
