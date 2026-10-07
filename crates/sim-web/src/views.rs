//! Small views every page uses: what shows while an answer loads or when it fails, the
//! breadcrumbs, and a variant's settings.

use leptos::prelude::*;
use tempest_sim_api::data::Setting;
use tempest_ui_kit::components::*;

use crate::request::{Loaded, RequestError};

/// `show` of the loaded value, a quiet placeholder while the first answer is on its way, or
/// what went wrong.
pub fn loaded<T, V>(
    resource: Loaded<T>,
    show: impl Fn(T) -> V + Send + Sync + 'static,
) -> impl IntoView
where
    T: Clone + Send + Sync + 'static,
    V: IntoView + 'static,
{
    move || match resource.get() {
        None => view! { <p class="loading dim">"loading…"</p> }.into_any(),
        Some(Err(e)) => view! { <Problem error=e /> }.into_any(),
        Some(Ok(value)) => show(value).into_any(),
    }
}

/// What went wrong with a request, in plain language.
#[component]
pub fn Problem(error: RequestError) -> impl IntoView {
    let title = match &error {
        RequestError::Refused(e) if e.error == "no_database" => "No results yet",
        RequestError::Refused(e) if e.error == "busy" => "The results are busy",
        RequestError::Refused(e) if e.error == "not_found" => "Not found",
        RequestError::Unreachable => "Can't reach the results",
        _ => "Something went wrong",
    };
    view! { <Empty title text=error.to_string() /> }
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

/// A variant's settings as `key value` chips, or `default`.
#[component]
pub fn Settings(settings: Vec<Setting>) -> impl IntoView {
    if settings.is_empty() {
        return view! { <span class="dim">"default"</span> }.into_any();
    }
    view! {
        <span class="settings">
            {settings
                .into_iter()
                .map(|s| {
                    view! {
                        <span class="setting">
                            <span class="setting__key">{s.key}</span>
                            <span class="setting__value">{s.value}</span>
                        </span>
                    }
                })
                .collect_view()}
        </span>
    }
    .into_any()
}

/// A stat with a value that never changes.
#[component]
pub fn Fact(label: &'static str, #[prop(into)] value: String) -> impl IntoView {
    view! { <Stat label value=Signal::stored(value) /> }
}
