//! Small views the pages share: loading and failure, breadcrumbs, swarm health, text to copy,
//! and fetching an answer into a signal.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::de::DeserializeOwned;
use tempest_protocol::tracker::BundleStats;
use tempest_ui_kit::components::*;

use crate::app::use_ctx;
use crate::present;
use crate::request::{self, RequestError};

/// What a page fetched: `None` while the first answer is on its way.
pub type Fetched<T> = RwSignal<Option<Result<T, RequestError>>>;

/// Fetches the tracker endpoint at `path()` into a signal, again whenever the signals `path`
/// reads change. Answers that arrive after a newer request was sent are dropped, so a fast
/// typist never sees an older search's results last.
pub fn fetch<T>(path: impl Fn() -> String + 'static) -> (Fetched<T>, ReadSignal<bool>)
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    let answer: Fetched<T> = RwSignal::new(None);
    let (loading, set_loading) = signal(false);
    let sent = StoredValue::new(0u64);
    Effect::new(move |_| {
        let path = path();
        let n = sent.get_value() + 1;
        sent.set_value(n);
        set_loading.set(true);
        spawn_local(async move {
            let got = request::get::<T>(&path).await;
            // The page may have moved on, or been torn down.
            if sent.try_get_value() == Some(n) {
                let _ = answer.try_set(Some(got));
                let _ = set_loading.try_set(false);
            }
        });
    });
    (answer, loading)
}

/// `show` of the fetched value, a quiet placeholder while the first answer is on its way, or
/// what went wrong.
pub fn shown<T, V>(
    fetched: Fetched<T>,
    show: impl Fn(T) -> V + Send + Sync + 'static,
) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
    V: IntoView + 'static,
{
    move || match fetched.get() {
        None => view! { <p class="loading dim">"loading…"</p> }.into_any(),
        Some(Err(e)) => view! { <Problem error=e /> }.into_any(),
        Some(Ok(value)) => show(value).into_any(),
    }
}

/// What went wrong with a request, in plain language.
#[component]
pub fn Problem(error: RequestError) -> impl IntoView {
    let ctx = use_ctx();
    let title = match &error {
        e if request::needs_sign_in(e) => "Sign in first",
        e if request::not_found(e) => "Not here",
        RequestError::Unreachable => "Can't reach the tracker",
        _ => "Something went wrong",
    };
    let text = match &error {
        e if request::not_found(e) => {
            "No listing is here: it may have been unpublished, or not be listed yet.".to_owned()
        }
        e => e.to_string(),
    };
    let sign_in = request::needs_sign_in(&error);
    view! {
        <Empty title text>
            {sign_in.then(|| view! {
                <Button kind=ButtonKind::Primary on_click=move || ctx.sign_in()>"Sign in"</Button>
            })}
        </Empty>
    }
}

/// Links back up to where a view sits.
#[component]
pub fn Crumbs(
    /// Each step's label and link, outermost first.
    steps: Vec<(String, String)>,
    /// The view itself.
    #[prop(into)]
    here: String,
) -> impl IntoView {
    view! {
        <nav class="crumbs" aria-label="Breadcrumbs">
            {steps
                .into_iter()
                .map(|(label, href)| view! { <a href=href>{label}</a><span class="crumbs__sep">"/"</span> })
                .collect_view()}
            <span aria-current="page">{here}</span>
        </nav>
    }
}

/// A swarm's health as four bars, filled by how many peers share it, with its words as the
/// tooltip.
#[component]
pub fn HealthBars(swarm: BundleStats) -> impl IntoView {
    let level = present::health_level(&swarm);
    let (words, tone) = present::health(&swarm);
    view! {
        <span class=format!("health {}", tone.class()) title=words role="img" aria-label=words>
            {(1..=4u8)
                .map(|bar| view! { <span class="health__bar" class:health__bar--on=bar <= level></span> })
                .collect_view()}
        </span>
    }
}

/// Text to copy, such as a command or a link, with its copy button.
#[component]
pub fn Copyable(
    /// The text.
    #[prop(into)]
    text: Signal<String>,
    /// What it is, for the toast: `command`, `link`.
    what: &'static str,
) -> impl IntoView {
    let ctx = use_ctx();
    view! {
        <div class="copyable">
            <code class="copyable__text">{text}</code>
            <button
                class="btn btn--small copyable__button"
                type="button"
                aria-label=format!("Copy the {what}")
                on:click=move |_| ctx.copy(text.get_untracked(), what)
            >
                "copy"
            </button>
        </div>
    }
}

/// A labeled multi-line text input.
#[component]
pub fn TextArea(
    /// Its label.
    label: &'static str,
    /// Its text.
    value: RwSignal<String>,
    /// Shown while it is empty.
    #[prop(optional)]
    placeholder: &'static str,
    /// The most characters it takes, shown as a count.
    max: usize,
    /// Rows tall.
    #[prop(default = 6)]
    rows: u32,
    /// What is wrong with the text, if anything.
    #[prop(optional, into)]
    error: Signal<Option<String>>,
    /// Its element, so a shortcut can focus it.
    #[prop(optional)]
    node_ref: NodeRef<leptos::html::Textarea>,
) -> impl IntoView {
    let count = move || value.with(|v| v.chars().count());
    let over = Signal::derive(move || count() > max);
    view! {
        <label class="field">
            <span class="field__label">{label}</span>
            <textarea
                node_ref=node_ref
                class="input textarea"
                rows=rows
                placeholder=placeholder
                aria-invalid=move || if error.with(Option::is_some) { "true" } else { "false" }
                prop:value=move || value.get()
                on:input=move |e| value.set(event_target_value(&e))
            ></textarea>
            <span class="field__help textarea__count" class:textarea__count--over=over>
                {move || format!("{} / {}", count(), max)}
            </span>
            {move || error.get().map(|error| view! { <span class="field__error">{error}</span> })}
        </label>
    }
}
