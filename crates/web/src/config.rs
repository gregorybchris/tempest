//! The engine's settings and categories, which have no events: fetched when the engine
//! answers, when Catalog or Settings opens, every few seconds while one shows, and after each
//! change (D-196). Every change goes through here and says how it went.

use std::collections::BTreeMap;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{Map, Value};
use tempest_api::settings::SettingsInfo;
use tempest_ui_kit::components::{Toasts, Tone};

use crate::api;
use crate::request::RequestError;
use crate::state::{Area, Ui};
use crate::store::Store;
use crate::stream::Link;

/// How often the settings are fetched while Catalog or Settings shows.
const REFRESH: Duration = Duration::from_secs(5);

/// The engine's settings, as last known. It is `Copy`.
#[derive(Clone, Copy)]
pub struct Config {
    /// The settings, once fetched.
    pub info: RwSignal<Option<SettingsInfo>>,
    /// Each category's save folder, by name, once fetched.
    pub categories: RwSignal<Option<BTreeMap<String, String>>>,
    /// Changes sent and not yet answered, and how many have been sent: a fetch that overlaps
    /// a change could show the old values, so its answer is dropped.
    busy: StoredValue<(u32, u64)>,
    toasts: Toasts,
}

impl Config {
    /// Settings that report changes to `toasts`, fetched as the module says.
    pub fn new(store: Store, ui: Ui, toasts: Toasts) -> Self {
        let config = Self {
            info: RwSignal::new(None),
            categories: RwSignal::new(None),
            busy: StoredValue::new((0, 0)),
            toasts,
        };
        // Whenever the engine comes (back) into reach, and whenever an area that shows the
        // settings opens.
        Effect::new(move |_| {
            let live = store.link.get() == Link::Live;
            let _ = ui.area.get();
            if live {
                config.refresh();
            }
        });
        if let Ok(timer) = set_interval_with_handle(
            move || {
                let showing = matches!(ui.area.get_untracked(), Area::Catalog | Area::Settings);
                if showing && store.link.get_untracked() == Link::Live {
                    config.refresh();
                }
            },
            REFRESH,
        ) {
            on_cleanup(move || timer.clear());
        }
        config
    }

    /// Fetches the settings and categories, unless a change awaits its answer or is sent
    /// before they arrive.
    pub fn refresh(self) {
        let Some((0, sent)) = self.busy.try_get_value() else {
            return;
        };
        spawn_local(async move {
            let (settings, categories) = (api::settings().await, api::categories().await);
            if self.busy.try_get_value() != Some((0, sent)) {
                return;
            }
            if let Ok(settings) = settings {
                self.info.set(Some(settings));
            }
            if let Ok(categories) = categories {
                let folders = categories
                    .into_iter()
                    .map(|(name, c)| (name, c.save_folder.display().to_string()))
                    .collect();
                self.categories.set(Some(folders));
            }
        });
    }

    /// Changes settings, by dotted key, then says `done`, or what went wrong.
    pub fn change(self, changes: impl IntoIterator<Item = (&'static str, Value)>, done: String) {
        let changes: Map<String, Value> = changes
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        let changes = Value::Object(changes);
        self.settle(async move { api::change_settings(&changes).await }, done);
    }

    /// Changes settings without a word unless it fails, for choices that show at once, such
    /// as the theme.
    pub fn change_quietly(self, key: &'static str, value: Value) {
        self.change([(key, value)], String::new());
    }

    /// Gives setting `key` the config file's value again.
    pub fn reset(self, key: &'static str, label: String) {
        let done = format!("{label} is the config file's again");
        self.settle(async move { api::reset_setting(key).await }, done);
    }

    async fn track<T>(
        self,
        request: impl Future<Output = Result<T, RequestError>>,
    ) -> Result<T, RequestError> {
        self.busy.update_value(|(waiting, sent)| {
            *waiting += 1;
            *sent += 1;
        });
        let answer = request.await;
        let _ = self.busy.try_update_value(|(waiting, _)| *waiting -= 1);
        answer
    }

    fn settle(
        self,
        request: impl Future<Output = Result<SettingsInfo, RequestError>> + 'static,
        done: String,
    ) {
        spawn_local(async move {
            match self.track(request).await {
                Ok(info) => {
                    let _ = self.info.try_set(Some(info));
                    if !done.is_empty() {
                        self.toasts.push(Tone::Neutral, done);
                    }
                }
                Err(error) => self.toasts.push(Tone::Bad, error.to_string()),
            }
        });
    }

    /// Adds category `name`, or changes its folder.
    pub fn set_category(self, name: String, folder: String) {
        spawn_local(async move {
            match self.track(api::set_category_folder(&name, folder)).await {
                Ok(category) => {
                    let folder = category.save_folder.display().to_string();
                    self.toasts
                        .push(Tone::Neutral, format!("{name}'s downloads go to {folder}"));
                    let _ = self.categories.try_update(|c| {
                        c.get_or_insert_default().insert(name, folder);
                    });
                }
                Err(error) => self.toasts.push(Tone::Bad, error.to_string()),
            }
        });
    }

    /// Removes category `name`.
    pub fn remove_category(self, name: String) {
        spawn_local(async move {
            match self.track(api::remove_category(&name)).await {
                Ok(()) => {
                    self.toasts.push(Tone::Neutral, format!("Removed {name}"));
                    let _ = self.categories.try_update(|c| {
                        if let Some(c) = c {
                            c.remove(&name);
                        }
                    });
                }
                Err(error) => self.toasts.push(Tone::Bad, error.to_string()),
            }
        });
    }

    /// The category names, in order, once known.
    pub fn category_names(self) -> Vec<String> {
        self.categories
            .with(|c| c.as_ref().map(|c| c.keys().cloned().collect()))
            .unwrap_or_default()
    }
}
