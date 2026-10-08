//! Signing in to a remote engine (D-222). A remote engine wants its access token, which the
//! page trades for a session cookie it can't read; the cookie then carries every request and
//! the event stream. A local engine wants nothing, and the page never shows this.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::PREFIX;
use tempest_api::requests::{SessionInfo, SignInRequest};
use tempest_ui_kit::components::*;
use tempest_ui_kit::form::TextField;
use tempest_ui_kit::theme::{Look, Theme};

use crate::request::{self, RequestError, method};

/// How often a signed-in page checks that its session lives.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// Whether the page may use the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// Asking the engine.
    Checking,
    /// The engine wants a sign-in.
    SignedOut,
    /// The page may use the engine.
    SignedIn {
        /// Whether the engine asked for a sign-in, so the page can offer to sign out.
        remote: bool,
    },
}

impl Auth {
    /// What the engine's answer means for the page.
    pub fn from_info(info: SessionInfo) -> Self {
        match (info.required, info.signed_in) {
            (true, false) => Self::SignedOut,
            (required, _) => Self::SignedIn { remote: required },
        }
    }
}

fn session_path() -> String {
    format!("{PREFIX}/session")
}

/// Asks the engine whether the page may use it.
async fn status() -> Result<SessionInfo, RequestError> {
    request::get(&session_path()).await
}

/// Trades the access token for a session.
async fn sign_in(token: String) -> Result<SessionInfo, RequestError> {
    let body = SignInRequest { token };
    request::send(method::POST, &session_path(), Some(&body)).await
}

/// Ends the session.
pub async fn sign_out() -> Result<(), RequestError> {
    request::send(method::DELETE, &session_path(), None::<&()>).await
}

/// Learns whether the page is signed in, now and then every minute, so an expired session or
/// a changed token sends the page back to the sign-in.
pub fn watch(auth: RwSignal<Auth>) {
    check(auth);
    if let Ok(timer) = set_interval_with_handle(move || check(auth), CHECK_EVERY) {
        on_cleanup(move || timer.clear());
    }
}

/// Asks the engine once, and updates `auth` if it answers. An engine that can't be reached
/// changes nothing, except that a page still checking shows itself, and says so there.
pub fn check(auth: RwSignal<Auth>) {
    spawn_local(async move {
        match status().await {
            Ok(info) => {
                let _ = auth.try_set(Auth::from_info(info));
            }
            Err(RequestError::Unreachable) if auth.get_untracked() == Auth::Checking => {
                let _ = auth.try_set(Auth::SignedIn { remote: false });
            }
            Err(_) => {}
        }
    });
}

const LOOK_KEY: &str = "tempest.look";

/// Remembers the look in this browser, so the sign-in page, which can't ask the engine for
/// it, can wear it.
pub fn remember_look(look: &Look) {
    if let Some(storage) = window().local_storage().ok().flatten() {
        let value = format!("{},{},{}", look.theme.name(), look.scanlines, look.clean);
        let _ = storage.set_item(LOOK_KEY, &value);
    }
}

/// The look this browser last showed, or the default.
pub fn remembered_look() -> Look {
    let stored = window()
        .local_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(LOOK_KEY).ok().flatten())
        .unwrap_or_default();
    let mut parts = stored.split(',');
    Look {
        theme: parts.next().and_then(Theme::named).unwrap_or_default(),
        scanlines: parts.next() == Some("true"),
        clean: parts.next() == Some("true"),
    }
}

/// The sign-in page.
#[component]
pub fn SignIn(auth: RwSignal<Auth>) -> impl IntoView {
    let token = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move || {
        if busy.get_untracked() {
            return;
        }
        let entered = token.get_untracked().trim().to_owned();
        if entered.is_empty() {
            error.set(Some("Enter the access token.".to_owned()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn_local(async move {
            let answer = sign_in(entered).await;
            let _ = busy.try_set(false);
            match answer {
                Ok(info) => {
                    let _ = token.try_set(String::new());
                    let _ = auth.try_set(Auth::from_info(info));
                }
                Err(e) => {
                    let _ = error.try_set(Some(e.to_string()));
                }
            }
        });
    };
    view! {
        <main class="signin">
            <form
                class="signin__card"
                on:submit=move |e| {
                    e.prevent_default();
                    submit();
                }
            >
                <Wordmark />
                <h1 class="signin__title">"Sign in"</h1>
                <p class="signin__text">
                    "This engine takes remote connections. Enter its access token to use it from this browser."
                </p>
                <TextField
                    label="Access token"
                    value=token
                    secret=true
                    error=error
                    help="The engine keeps the token in its data directory."
                />
                <Button kind=ButtonKind::Primary disabled=busy on_click=move || submit()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </Button>
            </form>
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_required_sign_in_that_is_missing_shows_the_sign_in() {
        let info = |required, signed_in| SessionInfo {
            required,
            signed_in,
        };
        assert_eq!(Auth::from_info(info(true, false)), Auth::SignedOut);
        assert_eq!(
            Auth::from_info(info(true, true)),
            Auth::SignedIn { remote: true }
        );
        assert_eq!(
            Auth::from_info(info(false, true)),
            Auth::SignedIn { remote: false }
        );
    }
}
