//! What apps whose views live in the address share around those views: the route kept in the
//! address's `#`, the theme kept in the browser, fields that edit the address, and the page's
//! single-key shortcuts.

use std::time::Duration;

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::form::takes_keys;
use crate::theme::{Look, Theme, apply_look};

/// A view and what it shows, written in the address's `#` so that every view has a link and the
/// browser's back button works.
pub trait Route: Clone + PartialEq + Send + Sync + 'static {
    /// The route a `#` fragment names, with or without the `#`. Anything unknown is the
    /// default view.
    fn parse(hash: &str) -> Self;
    /// Its `#` fragment.
    fn hash(&self) -> String;
}

/// The route shown, kept in step with the address's `#` both ways.
pub struct Address<R: 'static> {
    /// The route shown.
    pub route: RwSignal<R>,
    /// Whether the next change of route replaces the browser's history entry.
    replace_next: StoredValue<bool>,
}

impl<R> Clone for Address<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R> Copy for Address<R> {}

impl<R: Route> Address<R> {
    /// The route the address names, followed from now on.
    pub fn follow() -> Self {
        let address = Self {
            route: RwSignal::new(R::parse(&current_hash())),
            replace_next: StoredValue::new(false),
        };
        let route = address.route;
        let handle = window_event_listener(ev::hashchange, move |_| {
            let parsed = R::parse(&current_hash());
            if route.with_untracked(|r| *r != parsed) {
                route.set(parsed);
            }
        });
        on_cleanup(move || handle.remove());
        Effect::new(move |_| {
            let hash = route.with(R::hash);
            let replace = address.replace_next.get_value();
            address.replace_next.set_value(false);
            if current_hash() == hash {
                return;
            }
            let location = window().location();
            let _ = if replace {
                location.replace(&hash)
            } else {
                location.set_hash(&hash)
            };
        });
        address
    }

    /// Shows `route`, as a new entry in the browser's history.
    pub fn go(self, route: R) {
        self.route.set(route);
    }

    /// Shows `route` in place of the current entry, as fields do while they are typed in.
    pub fn replace(self, route: R) {
        if self.route.with_untracked(|r| *r != route) {
            self.replace_next.set_value(true);
            self.route.set(route);
        }
    }
}

fn current_hash() -> String {
    window().location().hash().unwrap_or_default()
}

/// This browser's storage for the app, if it allows any.
pub fn storage() -> Option<web_sys::Storage> {
    window().local_storage().ok().flatten()
}

/// The page's theme, applied to the page: `?theme=` in the address, or the one last chosen in
/// this browser, saved under `key`, or else the default (D-135). It is saved only once chosen,
/// so the default applies until then.
pub fn use_theme(key: &'static str) -> RwSignal<Theme> {
    let saved = || {
        storage()?
            .get_item(key)
            .ok()
            .flatten()
            .and_then(|n| Theme::named(&n))
    };
    let search = window().location().search().unwrap_or_default();
    let theme = RwSignal::new(asked_theme(&search).or_else(saved).unwrap_or_default());
    apply_look(Signal::derive(move || Look {
        theme: theme.get(),
        ..Look::default()
    }));
    Effect::new(move |previous: Option<Theme>| {
        let now = theme.get();
        if previous.is_some_and(|p| p != now)
            && let Some(storage) = storage()
        {
            let _ = storage.set_item(key, now.name());
        }
        now
    });
    theme
}

/// The theme an address's query string, such as `?theme=amber`, asks for.
fn asked_theme(search: &str) -> Option<Theme> {
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|p| p.strip_prefix("theme="))
        .and_then(Theme::named)
}

/// A local signal that follows `from`, and hands its own changes to `to`: for a field that
/// edits part of the address.
pub fn synced<T>(from: Memo<T>, to: impl Fn(T) + 'static) -> RwSignal<T>
where
    T: Clone + PartialEq + Send + Sync + 'static,
{
    let local = RwSignal::new(from.get_untracked());
    Effect::new(move |_| {
        let value = from.get();
        if local.with_untracked(|l| *l != value) {
            local.set(value);
        }
    });
    Effect::new(move |_| {
        let value = local.get();
        if from.with_untracked(|f| *f != value) {
            to(value);
        }
    });
    local
}

/// How long typing must pause before [`debounce`] acts on it.
const TYPING_PAUSE: Duration = Duration::from_millis(150);

/// Runs `then` a moment after the last call, so typing doesn't ask the server at every key. A
/// call replaces the one waiting.
pub fn debounce(then: impl FnOnce() + 'static) {
    thread_local! {
        static PENDING: std::cell::Cell<Option<TimeoutHandle>> = const { std::cell::Cell::new(None) };
    }
    PENDING.with(|pending| {
        if let Some(handle) = pending.take() {
            handle.clear();
        }
        pending.set(set_timeout_with_handle(then, TYPING_PAUSE).ok());
    });
}

/// A key the page handles, after the rules every app shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageKey {
    /// `esc` outside a field: go back a level.
    Back,
    /// Any other key, by its `key` name, for the app's bindings.
    Key(String),
}

/// Calls `then` with each key press the page should handle: not with a modifier, nor one a
/// handler that ran first has dealt with, nor one typed into a field, which `esc` leaves. Enter
/// on a link or button is the browser's. `then` prevents the key's default once it acts on it.
pub fn page_keys(then: impl Fn(&web_sys::KeyboardEvent, PageKey) + 'static) {
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.ctrl_key() || e.meta_key() || e.alt_key() || e.default_prevented() {
            return;
        }
        let target = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
        let typing = target.as_ref().is_some_and(takes_keys);
        let on_control = target
            .as_ref()
            .is_some_and(|t| matches!(t.tag_name().as_str(), "A" | "BUTTON"));
        let key = e.key();
        if key == "Escape"
            && typing
            && let Some(field) = target.and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = field.blur();
        }
        if let Some(press) = page_key(&key, typing, on_control) {
            then(&e, press);
        }
    });
    on_cleanup(move || handle.remove());
}

/// What `key` means to the page, pressed in a field (`typing`) or on a link or button.
fn page_key(key: &str, typing: bool, on_control: bool) -> Option<PageKey> {
    match key {
        _ if typing => None,
        "Escape" => Some(PageKey::Back),
        "Enter" if on_control => None,
        key => Some(PageKey::Key(key.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_can_ask_for_a_theme() {
        assert_eq!(asked_theme("?theme=amber"), Theme::named("amber"));
        assert_eq!(asked_theme("?q=x&theme=amber"), Theme::named("amber"));
        assert_eq!(asked_theme("?theme=plaid"), None);
        assert_eq!(asked_theme(""), None);
    }

    #[test]
    fn fields_keep_their_keys_and_controls_their_enter() {
        assert_eq!(page_key("j", true, false), None);
        assert_eq!(page_key("Escape", true, false), None);
        assert_eq!(page_key("Escape", false, true), Some(PageKey::Back));
        assert_eq!(page_key("Enter", false, true), None);
        assert_eq!(
            page_key("Enter", false, false),
            Some(PageKey::Key("Enter".to_owned()))
        );
        assert_eq!(
            page_key("/", false, true),
            Some(PageKey::Key("/".to_owned()))
        );
    }
}
