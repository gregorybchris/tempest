//! What the forms send, checked before sending: a transfer's own limits, a share, a move.
//! Plain functions, so they are tested here and the dialogs stay thin.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use tempest_api::requests::AddRequest;
use tempest_api::settings::{ValueKind, absolute, parse_tags};
use tempest_api::units;

/// The settings a transfer's limits form edits.
pub const DOWNLOAD: &str = "limits.download";
/// The transfer's own upload limit.
pub const UPLOAD: &str = "limits.upload";
/// The transfer's own sharing ratio.
pub const RATIO: &str = "sharing.ratio";
/// The transfer's own sharing time.
pub const TIME: &str = "sharing.time";
/// The transfer's own sharing idle timeout.
pub const IDLE_TIMEOUT: &str = "sharing.idle_timeout";
/// What happens at the transfer's own sharing limit.
pub const ACTION: &str = "sharing.action";

/// The choices for what happens at a sharing limit: the engine's, then the transfer's own.
pub const ACTIONS: [(&str, &str); 3] = [
    ("", "use Settings"),
    ("stop", "stop sharing"),
    ("remove", "remove from list, keep files"),
];

/// A transfer's limits as its form shows them. Empty means the Settings value; `none`, no
/// limit of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitsForm {
    /// Its download limit.
    pub download: String,
    /// Its upload limit.
    pub upload: String,
    /// Its sharing ratio.
    pub ratio: String,
    /// Its sharing time.
    pub time: String,
    /// Its sharing idle timeout.
    pub idle_timeout: String,
    /// What happens at its sharing limit: `stop`, `remove`, or empty for the engine's.
    pub action: String,
}

/// A setting as its field starts.
fn shown(settings: &BTreeMap<String, Value>, key: &str) -> String {
    match settings.get(key) {
        None => String::new(),
        Some(Value::Null) => "none".to_owned(),
        Some(Value::Number(n)) if key.starts_with("limits.") => units::limit_input(n.as_u64()),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

impl LimitsForm {
    /// The form for a transfer with `settings` of its own.
    pub fn of(settings: &BTreeMap<String, Value>) -> Self {
        Self {
            download: shown(settings, DOWNLOAD),
            upload: shown(settings, UPLOAD),
            ratio: shown(settings, RATIO),
            time: shown(settings, TIME),
            idle_timeout: shown(settings, IDLE_TIMEOUT),
            action: match settings.get(ACTION).and_then(Value::as_str) {
                Some(action @ ("stop" | "remove")) => action.to_owned(),
                _ => String::new(),
            },
        }
    }

    /// The transfer's settings with the form's in place of those it had, keeping the others,
    /// or why the form is wrong.
    pub fn apply(
        &self,
        settings: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, String> {
        let mut settings = settings.clone();
        for key in [DOWNLOAD, UPLOAD, RATIO, TIME, IDLE_TIMEOUT, ACTION] {
            settings.remove(key);
        }
        let fields = [
            (DOWNLOAD, "Download", ValueKind::Limit, &self.download),
            (UPLOAD, "Upload", ValueKind::Limit, &self.upload),
            (RATIO, "Ratio", ValueKind::Ratio, &self.ratio),
            (TIME, "Time", ValueKind::Duration, &self.time),
            (
                IDLE_TIMEOUT,
                "Idle timeout",
                ValueKind::Duration,
                &self.idle_timeout,
            ),
        ];
        for (key, label, kind, text) in fields {
            if text.trim().is_empty() {
                continue;
            }
            let value = kind.parse(text).map_err(|e| format!("{label}: {e}"))?;
            // A speed limit of `none` is no limit of its own, as with the daemon's.
            if !(kind == ValueKind::Limit && value.is_null()) {
                settings.insert(key.to_owned(), value);
            }
        }
        if !self.action.is_empty() {
            settings.insert(ACTION.to_owned(), json!(self.action));
        }
        Ok(settings)
    }
}

/// The share form's fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShareForm {
    /// What to share: an absolute path on the engine's machine.
    pub path: String,
    /// The bundle's name; empty for the file or folder's.
    pub name: String,
    /// A description.
    pub comment: String,
    /// Tags, separated by commas.
    pub tags: String,
}

impl ShareForm {
    /// The request that shares it, or why it can't.
    pub fn request(&self) -> Result<AddRequest, String> {
        let path = self.path.trim();
        if path.is_empty() {
            return Err("Enter the path of a file or folder to share.".to_owned());
        }
        if !absolute(path) {
            return Err("Enter an absolute path on the daemon's machine.".to_owned());
        }
        let text = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
        Ok(AddRequest {
            path: Some(PathBuf::from(path)),
            name: text(&self.name),
            comment: self.comment.trim().to_owned(),
            tags: parse_tags(&self.tags),
            ..AddRequest::default()
        })
    }
}

/// Why `folder` can't take a transfer's files, if it can't.
pub fn folder_problem(folder: &str) -> Option<&'static str> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Some("Enter the folder to move the files into.");
    }
    (!absolute(folder)).then_some("Enter an absolute path on the daemon's machine.")
}

/// Why `folder` can't take a new download, if it can't. Empty leaves it to the daemon.
pub fn save_folder_problem(folder: &str) -> Option<&'static str> {
    let folder = folder.trim();
    (!folder.is_empty() && !absolute(folder))
        .then_some("Enter an absolute path on the daemon's machine.")
}

/// Why `name` can't name a view or category, if it can't.
pub fn name_problem(name: &str) -> Option<&'static str> {
    name.trim().is_empty().then_some("Enter a name.")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn a_transfers_limits_keep_its_other_settings() {
        let own = BTreeMap::from([
            ("limits.download".to_owned(), json!(2_000_000)),
            ("sharing.ratio".to_owned(), Value::Null),
            ("profile".to_owned(), json!("archive")),
        ]);
        let form = LimitsForm::of(&own);
        assert_eq!(form.download, "2M");
        assert_eq!(form.ratio, "none");
        assert_eq!(form.upload, "");

        let changed = LimitsForm {
            upload: "500K".to_owned(),
            time: "12h".to_owned(),
            idle_timeout: "5m".to_owned(),
            action: "remove".to_owned(),
            ..form
        }
        .apply(&own)
        .unwrap();
        assert_eq!(
            changed,
            BTreeMap::from([
                ("limits.download".to_owned(), json!(2_000_000)),
                ("limits.upload".to_owned(), json!(500_000)),
                ("sharing.ratio".to_owned(), Value::Null),
                ("sharing.time".to_owned(), json!("12h")),
                ("sharing.idle_timeout".to_owned(), json!("5m")),
                ("sharing.action".to_owned(), json!("remove")),
                ("profile".to_owned(), json!("archive")),
            ])
        );
        // Clearing the fields gives the engine's settings back.
        let cleared = LimitsForm::default().apply(&changed).unwrap();
        assert_eq!(
            cleared,
            BTreeMap::from([("profile".to_owned(), json!("archive"))])
        );
        let bad = LimitsForm {
            ratio: "-2".to_owned(),
            ..LimitsForm::default()
        };
        assert!(bad.apply(&own).unwrap_err().starts_with("Ratio: "));
    }

    #[test]
    fn shares_need_an_absolute_path() {
        let mut form = ShareForm::default();
        assert!(form.request().is_err());
        form.path = "films/one".to_owned();
        assert!(form.request().is_err());
        form.path = " /srv/films/one ".to_owned();
        form.tags = "film, #2026".to_owned();
        let request = form.request().unwrap();
        assert_eq!(request.path, Some(PathBuf::from("/srv/films/one")));
        assert_eq!(request.name, None);
        assert_eq!(request.tags.len(), 2);
        assert_eq!(folder_problem("/srv"), None);
        assert!(folder_problem("srv").is_some());
        assert_eq!(save_folder_problem("  "), None);
        assert_eq!(save_folder_problem("/srv"), None);
        assert!(save_folder_problem("srv").is_some());
        assert!(name_problem("  ").is_some());
    }

    proptest! {
        /// Whatever a transfer had, its limits form shows it and sends it back unchanged.
        #[test]
        fn the_limits_form_round_trips(
            download in proptest::option::of(1u64..1_000_000_000_000),
            ratio in proptest::option::of(1u32..100),
            action in proptest::sample::select(vec!["", "stop", "remove"]),
        ) {
            let mut own = BTreeMap::new();
            if let Some(download) = download {
                own.insert(DOWNLOAD.to_owned(), json!(download));
            }
            if let Some(ratio) = ratio {
                own.insert(RATIO.to_owned(), json!(f64::from(ratio) / 4.0));
            }
            if !action.is_empty() {
                own.insert(ACTION.to_owned(), json!(action));
            }
            prop_assert_eq!(LimitsForm::of(&own).apply(&own).unwrap(), own);
        }
    }
}
