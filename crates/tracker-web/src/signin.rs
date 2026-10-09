//! Signing in with a Tempest key (D-128): the page asks for a code, shows it with the command
//! that signs it, and polls until the signature arrives, when the tracker sets the session's
//! cookie. The key never leaves the user's client.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_tracker_api::account::{Challenge, Poll, PollResponse};
use tempest_tracker_api::codes;
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;

use crate::app::{Me, unix_now, use_ctx};
use crate::present;
use crate::request::{self, RequestError, method};
use crate::route::Route;

/// How often the page asks whether the code has been signed.
const POLL_EVERY: Duration = Duration::from_secs(2);

/// Where a sign-in stands.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    /// Asking for a code.
    Asking,
    /// Waiting for the code, which expires at the time given, to be signed.
    Waiting(Challenge, u64),
    /// The code expired or was refused; a new one is a click away.
    Expired,
    /// Something went wrong.
    Failed(RequestError),
}

/// The sign-in page.
#[component]
pub fn SignInPage() -> impl IntoView {
    let ctx = use_ctx();
    move || match ctx.me.get() {
        Me::SignedIn(account) => {
            let name = account
                .name
                .clone()
                .unwrap_or_else(|| present::short_key(&account.key.to_string()));
            view! {
                <section class="page signin">
                    <Empty title="Signed in" text=format!("You're signed in as {name}.")>
                        <a class="btn btn--primary" href="#/me">"Your account"</a>
                    </Empty>
                </section>
            }
            .into_any()
        }
        _ => view! { <SignIn /> }.into_any(),
    }
}

#[component]
fn SignIn() -> impl IntoView {
    let ctx = use_ctx();
    let toasts = use_toasts();
    let step = RwSignal::new(Step::Asking);
    let invite = RwSignal::new(String::new());
    let ask = move || {
        step.set(Step::Asking);
        spawn_local(async move {
            let answer =
                request::send::<Challenge>(method::POST, "session/challenges", None::<&()>).await;
            let _ = step.try_set(match answer {
                Ok(challenge) => {
                    let expires = unix_now() + challenge.expires_in_secs;
                    Step::Waiting(challenge, expires)
                }
                Err(error) => Step::Failed(error),
            });
        });
    };
    ask();

    // Polls while a code waits, until it is signed or expires.
    let busy = StoredValue::new(false);
    if let Ok(timer) = set_interval_with_handle(
        move || {
            let Some(Step::Waiting(challenge, expires)) = step.try_get_untracked() else {
                return;
            };
            if unix_now() >= expires {
                step.set(Step::Expired);
                return;
            }
            if busy.get_value() {
                return;
            }
            busy.set_value(true);
            spawn_local(async move {
                let body = Poll {
                    code: challenge.code.clone(),
                    poll: challenge.poll.clone(),
                };
                let answer = request::send::<PollResponse>(
                    method::POST,
                    "session/challenges/poll",
                    Some(&body),
                )
                .await;
                let _ = busy.try_set_value(false);
                match answer {
                    Ok(PollResponse {
                        session: Some(session),
                    }) => {
                        let name = session.account.name.clone().unwrap_or_else(|| {
                            present::short_key(&session.account.key.to_string())
                        });
                        let _ = ctx.me.try_set(Me::SignedIn(session.account));
                        toasts.push(Tone::Up, format!("Signed in as {name}"));
                        let back = ctx.after_sign_in.try_get_value().unwrap_or_default();
                        let back = if back == Route::SignIn {
                            Route::default()
                        } else {
                            back
                        };
                        ctx.replace(back);
                    }
                    Ok(PollResponse { session: None }) => {}
                    Err(e) if e.code() == Some(codes::SIGN_IN_FAILED) => {
                        let _ = step.try_set(Step::Expired);
                    }
                    // A dropped poll is tried again at the next tick.
                    Err(_) => {}
                }
            });
        },
        POLL_EVERY,
    ) {
        on_cleanup(move || timer.clear());
    }

    let invites = move || {
        ctx.info
            .with(|i| i.as_ref().is_some_and(|i| i.invites_required))
    };
    let body = move || {
        match step.get() {
        Step::Asking => view! { <p class="loading dim">"getting a code…"</p> }.into_any(),
        Step::Failed(error) => view! {
            <Empty title="Can't sign in right now" text=error.to_string()>
                <Button on_click=move || ask()>"Try again"</Button>
            </Empty>
        }
        .into_any(),
        Step::Expired => view! {
            <Empty title="This code expired" text="Codes last ten minutes and work once.".to_owned()>
                <Button kind=ButtonKind::Primary on_click=move || ask()>"Get a new code"</Button>
            </Empty>
        }
        .into_any(),
        Step::Waiting(challenge, expires) => {
            let code = challenge.code.clone();
            let command = Signal::derive({
                let code = code.clone();
                move || present::sign_in_command(&ctx.origin(), &code, &invite.get())
            });
            let left = move || format::duration(expires.saturating_sub(ctx.now.get().max(unix_now())));
            view! {
                <div class="signin__code" aria-label="Your sign-in code">{code}</div>
                <ol class="signin__steps">
                    <Show when=invites>
                        <li>
                            <p>"This tracker needs an invite the first time you sign in. Enter yours, or skip this if you've signed in here before."</p>
                            <label class="field signin__invite">
                                <span class="field__label">"Invite"</span>
                                <input
                                    class="input"
                                    type="text"
                                    spellcheck="false"
                                    placeholder="XXXXX-XXXXX"
                                    prop:value=move || invite.get()
                                    on:input=move |e| invite.set(event_target_value(&e))
                                />
                            </label>
                        </li>
                    </Show>
                    <li>
                        <p>"On the computer where you use Tempest, run:"</p>
                        <crate::views::Copyable text=command what="command" />
                    </li>
                    <li>
                        <p class="signin__waiting">
                            <span class="signin__pulse" aria-hidden="true"></span>
                            "Waiting for your key's signature. This page signs in by itself."
                        </p>
                        <p class="note dim">"The code expires in about "{left}"."</p>
                    </li>
                </ol>
            }
            .into_any()
        }
    }
    };
    view! {
        <section class="page signin">
            <header class="page__head">
                <h1 class="page__title glow">"Sign in"</h1>
                <p class="dim">
                    "There are no passwords. You sign in with your Tempest key, the one your client \
                     shares with: it signs this code, and the key never leaves your computer."
                </p>
            </header>
            <div class="signin__card">{body}</div>
        </section>
    }
}
