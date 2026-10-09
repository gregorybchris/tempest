//! Form controls: text fields, a themed select with its own popup, checkboxes, radios,
//! switches, sliders, number and file inputs.
//!
//! Most stay native elements restyled by the stylesheet, which keeps the browser's
//! accessibility and keyboard behavior. The select is the exception: a native one opens the
//! operating system's popup, which no stylesheet reaches, so [`Select`] draws its own listbox
//! and implements the keyboard of the WAI-ARIA select-only combobox. Its logic is in [`listbox`].

use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::theme::bool_attr;

pub mod listbox;
mod select;

pub use select::{Select, SelectField};

/// A page-unique ID for an element, starting with `prefix`.
fn unique_id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Whether `element` takes typed keys, so an app's single-key shortcuts must leave them alone:
/// text fields, native selects and [`Select`]s.
pub fn takes_keys(element: &web_sys::Element) -> bool {
    match element.tag_name().as_str() {
        "SELECT" | "TEXTAREA" => true,
        "INPUT" => element.get_attribute("type").is_none_or(|t| {
            !matches!(
                t.as_str(),
                "checkbox" | "radio" | "submit" | "button" | "range" | "file"
            )
        }),
        _ => element.get_attribute("role").as_deref() == Some("combobox"),
    }
}

/// A labeled text input.
#[component]
pub fn TextField(
    /// Its label.
    label: &'static str,
    /// Its text.
    value: RwSignal<String>,
    /// Shown while it is empty.
    #[prop(optional)]
    placeholder: &'static str,
    /// A line of help under it.
    #[prop(optional)]
    help: Option<&'static str>,
    /// What is wrong with the text, if anything.
    #[prop(optional, into)]
    error: Signal<Option<String>>,
    /// Whether the text is a secret: hidden, and left to a password manager.
    #[prop(optional)]
    secret: bool,
) -> impl IntoView {
    // The help and the error describe the input rather than name it, so they sit outside
    // its label.
    let id = unique_id("text");
    let (help_id, error_id) = (format!("{id}-help"), format!("{id}-error"));
    let described = {
        let (help_id, error_id) = (help_id.clone(), error_id.clone());
        move || {
            let ids: Vec<&str> = [
                help.is_some().then_some(help_id.as_str()),
                error.with(Option::is_some).then_some(error_id.as_str()),
            ]
            .into_iter()
            .flatten()
            .collect();
            (!ids.is_empty()).then(|| ids.join(" "))
        }
    };
    view! {
        <div class="field">
            <label class="field__label" for=id.clone()>{label}</label>
            <input
                id=id
                class="input"
                type=if secret { "password" } else { "text" }
                autocomplete=secret.then_some("current-password")
                placeholder=placeholder
                spellcheck="false"
                aria-invalid=move || bool_attr(error.with(Option::is_some))
                aria-describedby=described
                prop:value=move || value.get()
                on:input=move |e| value.set(event_target_value(&e))
            />
            {help.map(|help| view! { <span class="field__help" id=help_id>{help}</span> })}
            {move || {
                let error_id = error_id.clone();
                error.get().map(|error| view! { <span class="field__error" id=error_id>{error}</span> })
            }}
        </div>
    }
}

/// A search box: a text input marked with `/`, the key that focuses it.
#[component]
pub fn SearchBox(
    /// The query.
    value: RwSignal<String>,
    /// Shown while it is empty.
    #[prop(default = "search…")]
    placeholder: &'static str,
    /// Its element, so a shortcut can focus it.
    #[prop(optional)]
    node_ref: NodeRef<html::Input>,
) -> impl IntoView {
    view! {
        <div class="search">
            <input
                node_ref=node_ref
                class="input"
                type="search"
                placeholder=placeholder
                aria-label=placeholder
                spellcheck="false"
                prop:value=move || value.get()
                on:input=move |e| value.set(event_target_value(&e))
            />
        </div>
    }
}

/// A labeled number input with − and + buttons that step it within its range.
#[component]
pub fn NumberField(
    /// Its label.
    label: &'static str,
    /// Its value.
    value: RwSignal<f64>,
    /// The smallest value.
    min: f64,
    /// The largest value.
    max: f64,
    /// How far the buttons and arrow keys step.
    #[prop(default = 1.0)]
    step: f64,
    /// A line of help under it.
    #[prop(optional)]
    help: Option<&'static str>,
) -> impl IntoView {
    let id = unique_id("number");
    let nudge = move |by: f64| value.update(|v| *v = listbox::stepped(*v, by, min, max));
    view! {
        <div class="field">
            <label class="field__label" for=id.clone()>{label}</label>
            <div class="number">
                <button class="number__step" type="button" tabindex="-1" aria-label="Less"
                    disabled=move || { value.get() <= min } on:click=move |_| nudge(-step)>"−"</button>
                <input
                    id=id
                    class="input number__input"
                    type="number"
                    min=min
                    max=max
                    step=step
                    prop:value=move || value.get().to_string()
                    // While typing, a number in range is taken as it is, and anything else, such
                    // as "1." or a first digit of "12" under a minimum of 5, waits; leaving the
                    // field brings it into range.
                    on:input=move |e| {
                        if let Ok(v) = event_target_value(&e).parse::<f64>()
                            && (min..=max).contains(&v)
                            && v != value.get_untracked()
                        {
                            value.set(v);
                        }
                    }
                    on:change=move |e| {
                        let typed = event_target_value(&e).parse::<f64>();
                        let v = typed.map_or(value.get_untracked(), |v| v.clamp(min, max));
                        value.set(v);
                        if let Some(input) = e
                            .target()
                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                        {
                            input.set_value(&v.to_string());
                        }
                    }
                />
                <button class="number__step" type="button" tabindex="-1" aria-label="More"
                    disabled=move || { value.get() >= max } on:click=move |_| nudge(step)>"+"</button>
            </div>
            {help.map(|help| view! { <span class="field__help">{help}</span> })}
        </div>
    }
}

/// A labeled slider over a range, showing its value.
#[component]
pub fn Slider(
    /// Its label.
    label: &'static str,
    /// Its value.
    value: RwSignal<f64>,
    /// The smallest value.
    min: f64,
    /// The largest value.
    max: f64,
    /// The step between values.
    #[prop(default = 1.0)]
    step: f64,
    /// Formats the value shown beside the label.
    #[prop(default = |v| format!("{v}"))]
    format: fn(f64) -> String,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
) -> impl IntoView {
    let fill = move || {
        let share = if max > min {
            (value.get() - min) / (max - min)
        } else {
            0.0
        };
        format!("--fill:{:.2}%", share.clamp(0.0, 1.0) * 100.0)
    };
    view! {
        <label class="field">
            <span class="field__label slider__head">
                <span>{label}</span>
                <span class="slider__value">{move || format(value.get())}</span>
            </span>
            <input
                class="slider"
                type="range"
                min=min
                max=max
                step=step
                disabled=disabled
                style=fill
                prop:value=move || value.get().to_string()
                on:input=move |e| {
                    if let Ok(v) = event_target_value(&e).parse() {
                        value.set(v);
                    }
                }
            />
        </label>
    }
}

/// A labeled file input, its button drawn like the kit's.
#[component]
pub fn FileField(
    /// Its label.
    label: &'static str,
    /// The file types it takes, as in the `accept` attribute.
    #[prop(optional)]
    accept: &'static str,
    /// Whether it takes several files.
    #[prop(optional)]
    multiple: bool,
    /// Called with the chosen files' names.
    #[prop(into)]
    on_change: Callback<Vec<String>>,
) -> impl IntoView {
    let names = move |e: ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&e);
        let files = input.files();
        (0..files.as_ref().map_or(0, web_sys::FileList::length))
            .filter_map(|i| files.as_ref()?.get(i).map(|f| f.name()))
            .collect::<Vec<_>>()
    };
    view! {
        <label class="field">
            <span class="field__label">{label}</span>
            <input class="file" type="file" accept=accept multiple=multiple on:change=move |e| on_change.run(names(e)) />
        </label>
    }
}

/// A checkbox and its label.
#[component]
pub fn Checkbox(
    /// Whether it is checked.
    #[prop(into)]
    checked: Signal<bool>,
    /// Called with the new state when it is clicked.
    #[prop(into)]
    on_change: Callback<bool>,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// Its label.
    children: Children,
) -> impl IntoView {
    view! {
        <label class="check">
            <input
                class="check__box"
                type="checkbox"
                disabled=disabled
                prop:checked=checked
                on:change=move |e| on_change.run(event_target_checked(&e))
            />
            <span class="check__label">{children()}</span>
        </label>
    }
}

/// One option of a radio group: the radios that share `name` are exclusive.
#[component]
pub fn Radio(
    /// The group's name.
    #[prop(into)]
    name: String,
    /// Whether it is the chosen option.
    #[prop(into)]
    checked: Signal<bool>,
    /// Called when it is chosen.
    #[prop(into)]
    on_select: Callback<()>,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// Its label.
    children: Children,
) -> impl IntoView {
    view! {
        <label class="check">
            <input
                class="check__box check__box--radio"
                type="radio"
                name=name
                disabled=disabled
                prop:checked=checked
                on:change=move |_| on_select.run(())
            />
            <span class="check__label">{children()}</span>
        </label>
    }
}

/// An on and off switch: a button with the `switch` role, drawn as a sliding block.
#[component]
pub fn Switch(
    /// Whether it is on.
    #[prop(into)]
    on: Signal<bool>,
    /// Called with the new state when it is clicked.
    #[prop(into)]
    on_change: Callback<bool>,
    /// Its name for screen readers, when it has no label of its own.
    #[prop(optional)]
    label: Option<&'static str>,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// Text beside it, such as "on" or what it switches.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <button
            class="switch"
            type="button"
            role="switch"
            aria-label=label
            aria-checked=move || bool_attr(on.get())
            disabled=disabled
            on:click=move |_| on_change.run(!on.get_untracked())
        >
            <span class="switch__track" aria-hidden="true"><span class="switch__thumb"></span></span>
            {children.map(|children| view! { <span class="switch__label">{children()}</span> })}
        </button>
    }
}
