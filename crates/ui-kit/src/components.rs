//! The building blocks every web UI shares: navigation, buttons, badges, panels, tables,
//! drawers, dialogs, toasts and key hints, and the form controls of [`crate::form`]. Each
//! renders the stylesheet's classes.

use std::time::Duration;

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;

use crate::chart::{Color, Sparkline};
pub use crate::form::*;
use crate::theme::bool_attr;

/// An accent, for text and marks that say how something is going.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    /// Plain text.
    #[default]
    Neutral,
    /// Quiet: stopped, queued, done.
    Dim,
    /// Downloading.
    Down,
    /// Uploading or sharing.
    Up,
    /// Needs attention now: checking, moving.
    Hot,
    /// Something is wrong but may fix itself: stalled.
    Warn,
    /// Something failed.
    Bad,
}

impl Tone {
    /// Its class, which sets `--tone`.
    pub fn class(self) -> &'static str {
        match self {
            Tone::Neutral => "tone--neutral",
            Tone::Dim => "tone--dim",
            Tone::Down => "tone--down",
            Tone::Up => "tone--up",
            Tone::Hot => "tone--hot",
            Tone::Warn => "tone--warn",
            Tone::Bad => "tone--bad",
        }
    }
}

/// TEMPEST in vector strokes.
#[component]
pub fn Wordmark() -> impl IntoView {
    // Each glyph is on a 10 by 14 grid, 14 apart.
    const GLYPHS: [&str; 7] = [
        "M0 0H10M5 0V14",
        "M10 0H0V14H10M0 7H7",
        "M0 14V0L5 8L10 0V14",
        "M0 14V0H10V7H0",
        "M10 0H0V14H10M0 7H7",
        "M10 0H0V7H10V14H0",
        "M0 0H10M5 0V14",
    ];
    view! {
        <svg class="wordmark" viewBox="-1 -1 96 16" role="img" aria-label="Tempest">
            // Numeric attributes keep this subtree off Leptos's static-template path, which
            // wraps static SVG in a stray <g> and breaks the gradient and path layout.
            <defs>
                <linearGradient id="wordmark-gradient" x1=0 x2=1 y1=0 y2=0>
                    <stop offset=0 stop-color="#3ef3ff"></stop>
                    <stop offset=0.55 stop-color="#ff52d9"></stop>
                    <stop offset=1 stop-color="#f4ff4f"></stop>
                </linearGradient>
            </defs>
            {GLYPHS
                .iter()
                .enumerate()
                .map(|(i, d)| view! { <path transform=format!("translate({} 0)", i * 14) d=*d /> })
                .collect_view()}
        </svg>
    }
}

/// The app's areas as tabs in the app bar, or a bottom bar on phones. Each shows its number,
/// the key that opens it.
#[component]
pub fn Nav<T>(
    /// Each area and its name.
    items: Vec<(T, &'static str)>,
    /// The area shown.
    current: RwSignal<T>,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <nav class="nav" aria-label="Areas">
            {items
                .into_iter()
                .enumerate()
                .map(|(i, (item, label))| {
                    view! {
                        <button
                            class="nav__item"
                            type="button"
                            aria-current=move || (current.get() == item).then_some("page")
                            on:click=move |_| current.set(item)
                        >
                            <span class="nav__key">{i + 1}</span>
                            {label}
                        </button>
                    }
                })
                .collect_view()}
        </nav>
    }
}

/// A live number with a label, and optionally a sparkline of its recent values.
#[component]
pub fn Meter(
    /// What it measures.
    label: &'static str,
    /// The value, formatted.
    #[prop(into)]
    value: Signal<String>,
    /// Its accent.
    #[prop(default = Tone::Neutral)]
    tone: Tone,
    /// Recent values, oldest first, for a sparkline.
    #[prop(optional, into)]
    history: Option<Signal<Vec<f64>>>,
) -> impl IntoView {
    let color = match tone {
        Tone::Up => Color::Up,
        Tone::Hot => Color::Hot,
        Tone::Warn => Color::Warn,
        Tone::Bad => Color::Bad,
        Tone::Down => Color::Down,
        Tone::Neutral | Tone::Dim => Color::Fg,
    };
    view! {
        <div class=format!("meter {}", tone.class())>
            <span class="meter__label">{label}</span>
            <span class="meter__value">{value}</span>
            {history.map(|values| view! { <Sparkline values color /> })}
        </div>
    }
}

/// How a button looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// A framed button.
    #[default]
    Plain,
    /// The main action on a screen.
    Primary,
    /// An action that destroys something.
    Danger,
    /// A frameless button for minor actions.
    Ghost,
}

/// A button.
#[component]
pub fn Button(
    /// What happens on a click.
    #[prop(into)]
    on_click: Callback<()>,
    /// How it looks.
    #[prop(default = ButtonKind::Plain)]
    kind: ButtonKind,
    /// A smaller size, for rows and toolbars.
    #[prop(optional)]
    small: bool,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// A tooltip, such as its keyboard shortcut.
    #[prop(optional, into)]
    title: MaybeProp<String>,
    /// Its label.
    children: Children,
) -> impl IntoView {
    let mut class = "btn".to_owned();
    class.push_str(match kind {
        ButtonKind::Plain => "",
        ButtonKind::Primary => " btn--primary",
        ButtonKind::Danger => " btn--danger",
        ButtonKind::Ghost => " btn--ghost",
    });
    if small {
        class.push_str(" btn--small");
    }
    view! {
        <button class=class type="button" title=move || title.get() disabled=disabled on:click=move |_| on_click.run(())>
            {children()}
        </button>
    }
}

/// A `[x] label` switch.
#[component]
pub fn Toggle(
    /// What it switches.
    label: &'static str,
    /// Whether it is on.
    value: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <button
            class="toggle"
            type="button"
            aria-pressed=move || bool_attr(value.get())
            on:click=move |_| value.update(|v| *v = !*v)
        >
            {label}
        </button>
    }
}

/// A row of exclusive options.
#[component]
pub fn Segmented<T>(
    /// What the options choose, for screen readers.
    label: &'static str,
    /// Each option and its name.
    options: Vec<(T, &'static str)>,
    /// The option chosen.
    value: RwSignal<T>,
) -> impl IntoView
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    view! {
        <div class="segmented" role="group" aria-label=label>
            {options
                .into_iter()
                .map(|(option, name)| {
                    view! {
                        <button
                            class="segmented__option"
                            type="button"
                            aria-pressed=move || bool_attr(value.get() == option)
                            on:click=move |_| value.set(option)
                        >
                            {name}
                        </button>
                    }
                })
                .collect_view()}
        </div>
    }
}

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

/// Whether a click on a table row whose first link goes where the row does should follow
/// the row: a plain click, not one on a link, which goes by itself, nor one with a modifier,
/// which opens the link elsewhere.
pub fn row_click(e: &ev::MouseEvent) -> bool {
    let modified = e.ctrl_key() || e.meta_key() || e.shift_key() || e.alt_key();
    let on_link = e
        .target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .is_some_and(|t| t.closest("a").ok().flatten().is_some());
    e.button() == 0 && !modified && !on_link
}

/// A labeled figure in a [`Stats`] list.
#[component]
pub fn Stat(
    /// What it is.
    label: &'static str,
    /// Its value, formatted.
    #[prop(into)]
    value: Signal<String>,
) -> impl IntoView {
    view! {
        <div>
            <dt>{label}</dt>
            <dd>{value}</dd>
        </div>
    }
}

/// A row of [`Stat`]s.
#[component]
pub fn Stats(children: Children) -> impl IntoView {
    view! { <dl class="stats">{children()}</dl> }
}

/// Which column a table is sorted by, and which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<K> {
    /// The column.
    pub key: K,
    /// Largest or last first.
    pub descending: bool,
}

impl<K: PartialEq> Sort<K> {
    /// The sort after a click on column `key`: the same column flips, and another starts in its
    /// natural direction, largest first for numbers and A to Z for text.
    pub fn clicked(self, key: K, numeric: bool) -> Sort<K> {
        if self.key == key {
            Sort {
                key,
                descending: !self.descending,
            }
        } else {
            Sort {
                key,
                descending: numeric,
            }
        }
    }
}

/// A table header that sorts by its column.
#[component]
pub fn SortHeader<K>(
    /// Its label.
    label: &'static str,
    /// Its column.
    key: K,
    /// The table's sort.
    sort: RwSignal<Sort<K>>,
    /// Whether the column holds numbers, which sort largest first and align right.
    #[prop(optional)]
    numeric: bool,
) -> impl IntoView
where
    K: Copy + PartialEq + Send + Sync + 'static,
{
    let direction = move || {
        sort.with(|s| {
            (s.key == key).then_some(if s.descending {
                "descending"
            } else {
                "ascending"
            })
        })
    };
    view! {
        <th class:num=numeric aria-sort=direction>
            <button class="sort" type="button" data-sort=direction on:click=move |_| sort.update(|s| *s = s.clicked(key, numeric))>
                {label}
            </button>
        </th>
    }
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

/// A panel that slides in from the right over a scrim, or fills a narrow window. `esc` or a
/// click on the scrim closes it.
#[component]
pub fn Drawer(
    /// Whether it is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Closes it.
    #[prop(into)]
    on_close: Callback<()>,
    /// Its title.
    #[prop(into)]
    title: Signal<String>,
    /// Controls beside the title.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    /// Its contents.
    children: ChildrenFn,
) -> impl IntoView {
    close_on_escape(open, on_close);
    let actions = StoredValue::new(actions);
    view! {
        <Show when=move || open.get()>
            <div class="scrim" on:click=move |_| on_close.run(())></div>
            <aside class="drawer" role="dialog" aria-modal="true" aria-label=move || title.get()
                tabindex="-1" node_ref={focus_scope::<html::Aside>()} on:keydown=keep_tab_inside>
                <header class="drawer__head">
                    <h2 class="drawer__title">{title}</h2>
                    {actions.with_value(|a| a.as_ref().map(ViewFn::run))}
                    <button class="close" type="button" aria-label="Close" on:click=move |_| on_close.run(())>
                        "esc ×"
                    </button>
                </header>
                <div class="drawer__body">{children()}</div>
            </aside>
        </Show>
    }
}

/// A small window over a scrim, for a question or a short form. `esc` or a click on the scrim
/// closes it.
#[component]
pub fn Dialog(
    /// Whether it is open.
    #[prop(into)]
    open: Signal<bool>,
    /// Closes it.
    #[prop(into)]
    on_close: Callback<()>,
    /// Its title.
    #[prop(into)]
    title: Signal<String>,
    /// Its buttons, at the bottom right.
    #[prop(optional, into)]
    actions: Option<ViewFn>,
    /// Its contents.
    children: ChildrenFn,
) -> impl IntoView {
    close_on_escape(open, on_close);
    let actions = StoredValue::new(actions);
    view! {
        <Show when=move || open.get()>
            <div class="scrim" on:click=move |_| on_close.run(())></div>
            <div class="dialog" role="dialog" aria-modal="true" aria-label=move || title.get()
                tabindex="-1" node_ref={focus_scope::<html::Div>()} on:keydown=keep_tab_inside>
                <header class="dialog__head">
                    <h2 class="dialog__title">{title}</h2>
                    <button class="close" type="button" aria-label="Close" on:click=move |_| on_close.run(())>
                        "×"
                    </button>
                </header>
                <div class="dialog__body">{children()}</div>
                {actions.with_value(|a| a.as_ref().map(|a| view! { <div class="dialog__actions">{a.run()}</div> }))}
            </div>
        </Show>
    }
}

/// A reference for a dialog's element that moves focus into it once it shows, unless something
/// in it has already taken focus, and gives focus back to what had it when the dialog goes.
/// Must be called as the dialog is created.
fn focus_scope<E>() -> NodeRef<E>
where
    E: html::ElementType,
    E::Output: JsCast + Clone + 'static,
{
    let node = NodeRef::<E>::new();
    let opener = SendWrapper::new(
        document()
            .active_element()
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()),
    );
    Effect::new(move |_| {
        let Some(element) = node.get() else {
            return;
        };
        let element: web_sys::HtmlElement = element.unchecked_into();
        let inside = document()
            .active_element()
            .is_some_and(|active| element.contains(Some(&active)));
        if !inside {
            let _ = element.focus();
        }
    });
    on_cleanup(move || {
        if let Some(opener) = opener.take().filter(|o| o.is_connected()) {
            let _ = opener.focus();
        }
    });
    node
}

/// Keeps `tab` and `shift-tab` cycling through a dialog's controls rather than leaving it for
/// the page behind.
fn keep_tab_inside(e: ev::KeyboardEvent) {
    if e.key() != "Tab" {
        return;
    }
    let Some(dialog) = e
        .current_target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    const FOCUSABLE: &str = "button:not([disabled]), [href], input:not([disabled]), \
                             select:not([disabled]), textarea:not([disabled]), \
                             [tabindex]:not([tabindex=\"-1\"])";
    let Ok(all) = dialog.query_selector_all(FOCUSABLE) else {
        return;
    };
    let item = |i: u32| {
        all.item(i)
            .and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
    };
    let (Some(first), Some(last)) = (item(0), item(all.length().saturating_sub(1))) else {
        e.prevent_default();
        return;
    };
    let active = document().active_element();
    let at = |element: &web_sys::HtmlElement| {
        active
            .as_ref()
            .is_some_and(|a| a == element.unchecked_ref::<web_sys::Element>())
    };
    let on_dialog = active
        .as_ref()
        .is_some_and(|a| a == dialog.unchecked_ref::<web_sys::Element>());
    if e.shift_key() && (at(&first) || on_dialog) {
        e.prevent_default();
        let _ = last.focus();
    } else if !e.shift_key() && at(&last) {
        e.prevent_default();
        let _ = first.focus();
    }
}

/// Calls `on_close` when `esc` is pressed while `open`, unless a handler that ran first, such as
/// an app's own key handling, has already dealt with it by preventing its default.
fn close_on_escape(open: Signal<bool>, on_close: Callback<()>) {
    let handle = window_event_listener(ev::keydown, move |e| {
        if e.key() == "Escape" && open.get_untracked() && !e.default_prevented() {
            e.prevent_default();
            on_close.run(());
        }
    });
    on_cleanup(move || handle.remove());
}

/// A short message that dismisses itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    id: u64,
    /// Its accent: [`Tone::Bad`] for errors.
    pub tone: Tone,
    /// What happened.
    pub text: String,
}

/// The page's toasts. [`Toaster`] provides it as context.
#[derive(Clone, Copy)]
pub struct Toasts {
    shown: RwSignal<Vec<Toast>>,
    next: StoredValue<u64>,
}

impl Toasts {
    /// How long a toast stays; errors stay twice as long.
    const SHOWN_FOR: Duration = Duration::from_secs(5);

    /// Shows `text`.
    pub fn push(self, tone: Tone, text: impl Into<String>) {
        let id = self.next.get_value();
        self.next.set_value(id + 1);
        self.shown.update(|shown| {
            shown.push(Toast {
                id,
                tone,
                text: text.into(),
            });
            // A burst of messages keeps only the latest few.
            if shown.len() > 4 {
                shown.remove(0);
            }
        });
        let stay = if tone == Tone::Bad {
            Self::SHOWN_FOR * 2
        } else {
            Self::SHOWN_FOR
        };
        set_timeout(move || self.dismiss(id), stay);
    }

    fn dismiss(self, id: u64) {
        // The page may have been torn down in the meantime.
        let _ = self.shown.try_update(|shown| shown.retain(|t| t.id != id));
    }
}

/// The page's [`Toasts`], from context.
///
/// # Panics
///
/// Outside a [`Toaster`].
pub fn use_toasts() -> Toasts {
    expect_context()
}

/// Provides [`Toasts`] to its children and shows them at the bottom right.
#[component]
pub fn Toaster(children: Children) -> impl IntoView {
    let toasts = Toasts {
        shown: RwSignal::new(Vec::new()),
        next: StoredValue::new(0),
    };
    provide_context(toasts);
    view! {
        {children()}
        <div class="toasts" role="status" aria-live="polite">
            <For each=move || toasts.shown.get() key=|t| t.id let:toast>
                <div class=format!("toast {}", toast.tone.class())>
                    <span class="toast__text">{toast.text}</span>
                    <button class="close" type="button" aria-label="Dismiss" on:click=move |_| toasts.dismiss(toast.id)>
                        "×"
                    </button>
                </div>
            </For>
        </div>
    }
}

/// A key hint: keys and what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    /// The keys, each shown boxed.
    pub keys: Vec<&'static str>,
    /// What they do.
    pub label: &'static str,
}

/// The bar of key hints along the bottom, listing the keys that work in the focused view.
#[component]
pub fn HintBar(
    /// The hints.
    #[prop(into)]
    hints: Signal<Vec<Hint>>,
) -> impl IntoView {
    view! {
        <footer class="hints">
            {move || {
                hints
                    .get()
                    .into_iter()
                    .map(|hint| {
                        view! {
                            <span>
                                {hint.keys.into_iter().map(|k| view! { <kbd>{k}</kbd> }).collect_view()}
                                {hint.label}
                            </span>
                        }
                    })
                    .collect_view()
            }}
        </footer>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorting_flips_the_same_column_and_starts_others_naturally() {
        let sort = Sort {
            key: "name",
            descending: false,
        };
        assert_eq!(
            sort.clicked("name", false),
            Sort {
                key: "name",
                descending: true
            }
        );
        assert_eq!(
            sort.clicked("size", true),
            Sort {
                key: "size",
                descending: true
            }
        );
        assert_eq!(
            sort.clicked("status", false),
            Sort {
                key: "status",
                descending: false
            }
        );
    }
}
