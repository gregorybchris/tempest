//! What views are laid out with: panels, tabs, stats and empty states.

use leptos::ev;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::theme::bool_attr;

/// A box-drawn frame with its title set into the top border.
#[component]
pub fn Panel(
    /// Its title.
    #[prop(into)]
    title: Signal<String>,
    /// Controls at the right of the title, such as toggles.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    /// Its contents.
    children: Children,
) -> impl IntoView {
    view! {
        <section class="panel">
            <div class="panel__head">
                <h2 class="panel__title">{title}</h2>
                {actions.map(|actions| view! { <div class="panel__actions">{actions.run()}</div> })}
            </div>
            {children()}
        </section>
    }
}

/// Tabs over a set of views. The arrow keys, `home` and `end` move between them, and focus
/// follows the tab shown.
#[component]
pub fn Tabs<T>(
    /// What the tabs choose between, for screen readers.
    label: &'static str,
    /// Each tab and its name.
    tabs: Vec<(T, &'static str)>,
    /// The tab shown.
    current: RwSignal<T>,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    let order: Vec<T> = tabs.iter().map(|(t, _)| *t).collect();
    let on_key = move |e: ev::KeyboardEvent| {
        let len = order.len() as isize;
        let i = order
            .iter()
            .position(|&t| t == current.get_untracked())
            .unwrap_or(0) as isize;
        let next = match e.key().as_str() {
            "ArrowRight" => (i + 1).rem_euclid(len),
            "ArrowLeft" => (i - 1).rem_euclid(len),
            "Home" => 0,
            "End" => len - 1,
            _ => return,
        } as usize;
        current.set(order[next]);
        e.prevent_default();
        let tab = e
            .current_target()
            .and_then(|list| list.dyn_into::<web_sys::Element>().ok())
            .and_then(|list| list.children().item(next as u32))
            .and_then(|tab| tab.dyn_into::<web_sys::HtmlElement>().ok());
        if let Some(tab) = tab {
            let _ = tab.focus();
        }
    };
    view! {
        <div class="tabs" role="tablist" aria-label=label on:keydown=on_key>
            {tabs
                .into_iter()
                .map(|(tab, name)| {
                    let selected = move || current.get() == tab;
                    view! {
                        <button
                            class="tabs__tab"
                            type="button"
                            role="tab"
                            aria-selected=move || bool_attr(selected())
                            tabindex=move || if selected() { "0" } else { "-1" }
                            on:click=move |_| current.set(tab)
                        >
                            {name}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

/// A labeled figure in a [`Stats`] list.
#[component]
pub fn Stat(
    /// What it is.
    label: &'static str,
    /// Its value, formatted.
    #[prop(into)]
    value: Signal<String>,
    /// A tooltip for the value, such as an exact time.
    #[prop(optional, into)]
    title: Option<Signal<String>>,
) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd title=move || title.map(|t| t.get())>{value}</dd>
        </div>
    }
}

/// A row of [`Stat`]s.
#[component]
pub fn Stats(children: Children) -> impl IntoView {
    view! { <dl class="stats">{children()}</dl> }
}

/// What a view shows when it has nothing to show: what that means and what to do.
#[component]
pub fn Empty(
    /// A short heading.
    title: &'static str,
    /// The explanation, in plain language.
    #[prop(into)]
    text: Signal<String>,
    /// Actions, such as a button that adds the first item.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    view! {
        <div class="empty">
            <div class="empty__title">{title}</div>
            <p class="empty__text">{text}</p>
            {children.map(|children| children())}
        </div>
    }
}
