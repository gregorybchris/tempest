//! A transfer's own speed and sharing limits (D-118), edited in a form and sent as its
//! settings, which keep any other overrides it has.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tempest_api::{Kind, TransferInfo};

use super::App;
use super::modal::{FormKind, Modal};
use super::request::{Action, Request, Target};
use super::rows::{ValueKind, limit_input};
use crate::form::{Field, Form};
use crate::format;

/// The settings the form edits, and its labels for them.
const DOWNLOAD: (&str, &str) = ("limits.download", "Download");
const UPLOAD: (&str, &str) = ("limits.upload", "Upload");
const RATIO: (&str, &str) = ("sharing.ratio", "Ratio");
const TIME: (&str, &str) = ("sharing.time", "Time");
const IDLE: (&str, &str) = ("sharing.idle_timeout", "Idle timeout");
const ACTION: (&str, &str) = ("sharing.action", "At the limit");

/// The choices for what happens at a sharing limit, the engine's first.
const ACTIONS: [&str; 3] = ["as the engine says", "stop it", "remove it"];

/// A setting's value as a form field starts with it: empty when the transfer has none of its
/// own, `none` for an explicit no-limit.
fn shown(settings: &BTreeMap<String, Value>, key: &str) -> String {
    match settings.get(key) {
        None => String::new(),
        Some(Value::Null) => "none".to_owned(),
        Some(Value::Number(n)) if key.starts_with("limits.") && n.as_u64().is_some() => {
            limit_input(n.as_u64())
        }
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

impl App {
    /// Opens the limits form for `transfer`.
    pub(super) fn open_limits(&mut self, transfer: &TransferInfo) {
        let own = &transfer.settings;
        let engine = self
            .settings
            .as_ref()
            .map(|view| serde_json::to_value(&view.settings.sharing).expect("settings serialize"));
        // A share has no limits unless it sets its own (D-224).
        let share = transfer.kind == Kind::Share;
        let engine_says = |key: &str| match engine.as_ref().map(|e| &e[key]) {
            _ if share => "no limit of its own".to_owned(),
            Some(Value::Null) => "the engine's: none".to_owned(),
            Some(Value::String(text)) => format!("the engine's: {text}"),
            Some(value) if !value.is_null() => format!("the engine's: {value}"),
            _ => "the engine's".to_owned(),
        };
        let action = match own.get(ACTION.0).and_then(Value::as_str) {
            Some("stop") => 1,
            Some("remove") => 2,
            _ => 0,
        };
        let form = Form::new(vec![
            Field::text(DOWNLOAD.1, &shown(own, DOWNLOAD.0), "no limit of its own"),
            Field::text(UPLOAD.1, &shown(own, UPLOAD.0), "no limit of its own"),
            Field::text(RATIO.1, &shown(own, RATIO.0), engine_says("ratio")),
            Field::text(TIME.1, &shown(own, TIME.0), engine_says("time")),
            Field::text(IDLE.1, &shown(own, IDLE.0), engine_says("idle_timeout")),
            Field::choice(ACTION.1, ACTIONS.map(str::to_owned).to_vec(), action),
        ]);
        let target = Target {
            id: transfer.id,
            name: transfer.name.clone(),
        };
        self.modal = Some(Modal::Form(Box::new((FormKind::Limits(target), form))));
    }

    /// The request the limits form makes for `target`: its settings with the form's in place
    /// of those it had.
    pub(super) fn limits_request(&self, target: &Target, form: &Form) -> Result<Request, String> {
        let mut settings = self
            .model
            .transfer(&target.id)
            .map(|t| t.settings.clone())
            .unwrap_or_default();
        for (key, _) in [DOWNLOAD, UPLOAD, RATIO, TIME, IDLE, ACTION] {
            settings.remove(key);
        }
        for (key, label) in [DOWNLOAD, UPLOAD] {
            if let Some(limit) = format::parse_limit(&form.text(label))? {
                settings.insert(key.to_owned(), json!(limit));
            }
        }
        // Empty leaves the engine's setting; `none` overrides it with no limit.
        for ((key, label), kind) in [
            (RATIO, ValueKind::Ratio),
            (TIME, ValueKind::Duration),
            (IDLE, ValueKind::Duration),
        ] {
            let text = form.text(label);
            if !text.is_empty() {
                let value = kind
                    .parse(&text)
                    .map_err(|error| format!("{label}: {error}"))?;
                settings.insert(key.to_owned(), value);
            }
        }
        match form.chosen(ACTION.1) {
            1 => settings.insert(ACTION.0.to_owned(), json!("stop")),
            2 => settings.insert(ACTION.0.to_owned(), json!("remove")),
            _ => None,
        };
        Ok(Request::Transfer(
            target.clone(),
            Action::Settings(settings),
        ))
    }
}
