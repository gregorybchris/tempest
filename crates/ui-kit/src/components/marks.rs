//! Small marks inside rows and panels: badges, chips and progress bars.

use leptos::prelude::*;

use super::Tone;

/// A status with a colored mark.
#[component]
pub fn Badge(
    /// Its accent.
    #[prop(into)]
    tone: Signal<Tone>,
    /// Its text.
    #[prop(into)]
    label: Signal<String>,
) -> impl IntoView {
    view! { <span class=move || format!("badge {}", tone.get().class())>{label}</span> }
}

/// A small label, such as a tag, optionally removable.
#[component]
pub fn Chip(
    /// Its text.
    #[prop(into)]
    label: String,
    /// Removes it; without this it has no remove button.
    #[prop(optional, into)]
    on_remove: Option<Callback<()>>,
) -> impl IntoView {
    let remove_label = format!("Remove {label}");
    view! {
        <span class="chip">
            {label}
            {on_remove.map(|remove| {
                view! {
                    <button class="chip__remove" type="button" aria-label=remove_label on:click=move |_| remove.run(())>
                        "×"
                    </button>
                }
            })}
        </span>
    }
}

/// A progress bar with its percentage.
#[component]
pub fn ProgressBar(
    /// How far along, from 0 to 1.
    #[prop(into)]
    fraction: Signal<f64>,
    /// Its accent.
    #[prop(into, default = Signal::stored(Tone::Down))]
    tone: Signal<Tone>,
) -> impl IntoView {
    let width = move || format!("width:{:.2}%", fraction.get().clamp(0.0, 1.0) * 100.0);
    view! {
        <span class=move || format!("progress {}", tone.get().class())>
            <span
                class="progress__track"
                role="progressbar"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow=move || format!("{:.0}", fraction.get() * 100.0)
            >
                <span class="progress__fill" style=width></span>
            </span>
            <span class="progress__label">{move || crate::format::percent(fraction.get())}</span>
        </span>
    }
}
