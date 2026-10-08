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
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos_use::{
    OnClickOutsideOptions, UseEventListenerOptions, on_click_outside_with_options,
    use_event_listener, use_event_listener_with_options,
};
use wasm_bindgen::JsCast;

use crate::theme::bool_attr;

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
    view! {
        <label class="field">
            <span class="field__label">{label}</span>
            <input
                class="input"
                type=if secret { "password" } else { "text" }
                autocomplete=secret.then_some("current-password")
                placeholder=placeholder
                spellcheck="false"
                aria-invalid=move || bool_attr(error.with(Option::is_some))
                prop:value=move || value.get()
                on:input=move |e| value.set(event_target_value(&e))
            />
            {help.map(|help| view! { <span class="field__help">{help}</span> })}
            {move || error.get().map(|error| view! { <span class="field__error">{error}</span> })}
        </label>
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
                    on:input=move |e| {
                        if let Ok(v) = event_target_value(&e).parse::<f64>() {
                            value.set(v.clamp(min, max));
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

/// A labeled [`Select`] of named values.
#[component]
pub fn SelectField(
    /// Its label.
    label: &'static str,
    /// Each value and its name.
    #[prop(into)]
    options: Signal<Vec<(String, String)>>,
    /// The value chosen.
    value: RwSignal<String>,
) -> impl IntoView {
    let label_id = unique_id("select-label");
    let trigger = NodeRef::<html::Button>::new();
    let focus = move |_| {
        if let Some(trigger) = trigger.get_untracked() {
            let _ = trigger.focus();
        }
    };
    view! {
        <div class="field">
            <span class="field__label" id=label_id.clone() on:click=focus>{label}</span>
            <Select options value on_change=move |v| value.set(v) labelledby=label_id node_ref=trigger />
        </div>
    }
}

/// A drop-down of named values, themed throughout: a button that opens a listbox in a popup.
///
/// It follows the WAI-ARIA select-only combobox. Focus stays on the button, which points at the
/// highlighted option with `aria-activedescendant`. The arrow keys, `Home`, `End`, `PageUp` and
/// `PageDown` move the highlight, typing jumps to a matching name, `enter` or `space` chooses,
/// `esc` closes, and so does a click outside or moving focus away. The popup opens below the
/// button, or above it when there is more room there.
#[component]
pub fn Select(
    /// Each value and its name.
    #[prop(into)]
    options: Signal<Vec<(String, String)>>,
    /// The value chosen.
    #[prop(into)]
    value: Signal<String>,
    /// Called with a newly chosen value.
    #[prop(into)]
    on_change: Callback<String>,
    /// Its name for screen readers, when no visible label names it.
    #[prop(optional)]
    label: Option<&'static str>,
    /// The ID of the element that names it.
    #[prop(optional, into)]
    labelledby: Option<String>,
    /// Shown when no option has the value.
    #[prop(optional)]
    placeholder: &'static str,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// Whether its value is wrong.
    #[prop(optional, into)]
    invalid: Signal<bool>,
    /// Its button, so a shortcut can focus it.
    #[prop(optional)]
    node_ref: NodeRef<html::Button>,
) -> impl IntoView {
    let id = unique_id("select");
    let list_id = format!("{id}-list");
    let base = StoredValue::new(id.clone());
    let option_id = move |i: usize| base.with_value(|id| format!("{id}-option-{i}"));
    let list = NodeRef::<html::Ul>::new();
    let open = RwSignal::new(false);
    let active = RwSignal::new(None::<usize>);
    let placement = RwSignal::new(None::<listbox::Placement>);
    let natural = StoredValue::new((0.0, 0.0));
    let typed = StoredValue::new(listbox::TypeAhead::default());

    let chosen = Memo::new(move |_| {
        let value = value.get();
        options.with(|o| o.iter().position(|(v, _)| *v == value))
    });
    let shown = move || {
        chosen
            .get()
            .and_then(|i| options.with(|o| o.get(i).map(|(_, name)| name.clone())))
            .unwrap_or_else(|| match value.get() {
                v if v.is_empty() => placeholder.to_owned(),
                v => v,
            })
    };
    let len = move || options.with(Vec::len);

    let show = move |at: Option<usize>| {
        if disabled.get_untracked() || len() == 0 {
            return;
        }
        active.set(at.or(chosen.get_untracked()).or(Some(0)));
        open.set(true);
    };
    let close = move || {
        open.set(false);
        placement.set(None);
    };
    let choose = move |i: usize| {
        let picked = options.with_untracked(|o| o.get(i).map(|(v, _)| v.clone()));
        close();
        if let Some(trigger) = node_ref.get_untracked() {
            let _ = trigger.focus();
        }
        if let Some(picked) = picked
            && picked != value.get_untracked()
        {
            on_change.run(picked);
        }
    };

    // Places the popup against the button, measuring its natural size first.
    let place = move |measure: bool| {
        let (Some(trigger), Some(list)) = (node_ref.get_untracked(), list.get_untracked()) else {
            return;
        };
        if measure {
            let rect = list.get_bounding_client_rect();
            natural.set_value((rect.width(), list.scroll_height() as f64));
        }
        let rect = trigger.get_bounding_client_rect();
        let viewport = (
            window()
                .inner_width()
                .ok()
                .and_then(|w| w.as_f64())
                .unwrap_or(0.0),
            window()
                .inner_height()
                .ok()
                .and_then(|h| h.as_f64())
                .unwrap_or(0.0),
        );
        placement.set(Some(listbox::place(
            listbox::Rect {
                left: rect.left(),
                top: rect.top(),
                width: rect.width(),
                height: rect.height(),
            },
            natural.get_value(),
            viewport,
        )));
    };
    Effect::new(move |_| {
        if open.get() && list.get().is_some() {
            place(true);
        }
    });
    // Keeps the highlighted option in view.
    Effect::new(move |_| {
        let (Some(i), Some(list)) = (active.get(), list.get()) else {
            return;
        };
        placement.track();
        let Some(item) = list
            .children()
            .item(i as u32)
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let (top, bottom) = (item.offset_top(), item.offset_top() + item.offset_height());
        if top < list.scroll_top() {
            list.set_scroll_top(top);
        } else if bottom > list.scroll_top() + list.client_height() {
            list.set_scroll_top(bottom - list.client_height());
        }
    });
    let follow = move |e: ev::Event| {
        if !open.get_untracked() {
            return;
        }
        let inside = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
            .zip(list.get_untracked())
            .is_some_and(|(target, list)| list.contains(Some(&target)));
        if !inside {
            place(false);
        }
    };
    let _ = use_event_listener(window(), ev::resize, move |e| follow(e.into()));
    let _ = use_event_listener_with_options(
        window(),
        ev::scroll,
        follow,
        UseEventListenerOptions::default()
            .capture(true)
            .passive(true),
    );
    let _ = on_click_outside_with_options(
        node_ref,
        move |_| {
            if open.get_untracked() {
                close();
            }
        },
        OnClickOutsideOptions::default().ignore(list),
    );

    let on_key = move |e: ev::KeyboardEvent| {
        if e.ctrl_key() || e.meta_key() || disabled.get_untracked() {
            return;
        }
        let key = e.key();
        let is_open = open.get_untracked();
        let printable = key.chars().count() == 1;
        let typing_on = typed.with_value(|t| t.active(e.time_stamp()));
        let move_to = |to: listbox::Move| listbox::moved(active.get_untracked(), len(), to);
        let find = |c: char| {
            let query = typed.try_update_value(|t| t.push(c, e.time_stamp()).to_owned());
            let from = active.get_untracked().or(chosen.get_untracked());
            query.and_then(|q| options.with_untracked(|o| listbox::matching(o, &q, from)))
        };
        match (is_open, key.as_str()) {
            (false, "ArrowDown" | "ArrowUp" | "Enter") => show(None),
            (false, " ") if !typing_on => show(None),
            (false, "Home") => show(Some(0)),
            (false, "End") => show(Some(len().saturating_sub(1))),
            (true, "ArrowUp") if e.alt_key() => {
                if let Some(i) = active.get_untracked() {
                    choose(i);
                }
            }
            (true, "ArrowDown") => active.set(move_to(listbox::Move::Next)),
            (true, "ArrowUp") => active.set(move_to(listbox::Move::Previous)),
            (true, "Home") => active.set(move_to(listbox::Move::First)),
            (true, "End") => active.set(move_to(listbox::Move::Last)),
            (true, "PageDown") => active.set(move_to(listbox::Move::PageDown)),
            (true, "PageUp") => active.set(move_to(listbox::Move::PageUp)),
            (true, "Enter") => {
                if let Some(i) = active.get_untracked() {
                    choose(i);
                }
            }
            (true, " ") if !typing_on => {
                if let Some(i) = active.get_untracked() {
                    choose(i);
                }
            }
            (true, "Escape") => close(),
            (true, "Tab") => {
                // Tab chooses the highlighted option and moves on, so it isn't prevented.
                if let Some(i) = active.get_untracked() {
                    let picked = options.with_untracked(|o| o.get(i).map(|(v, _)| v.clone()));
                    close();
                    if let Some(picked) = picked.filter(|p| *p != value.get_untracked()) {
                        on_change.run(picked);
                    }
                }
                return;
            }
            (_, _) if printable && !e.alt_key() => {
                let Some(c) = key.chars().next() else { return };
                match find(c) {
                    Some(i) if is_open => active.set(Some(i)),
                    Some(i) => show(Some(i)),
                    None if is_open => {}
                    None => show(None),
                }
            }
            _ => return,
        }
        e.prevent_default();
        // The app's own key handling and dialogs listen on the window: keys a select uses,
        // `esc` most of all, stop here.
        e.stop_propagation();
    };

    let list_style = move || match placement.get() {
        Some(p) => format!(
            "left:{:.1}px;top:{:.1}px;min-width:{:.1}px;max-height:{:.1}px",
            p.left, p.top, p.min_width, p.max_height
        ),
        // Unplaced, it is measured where it can't be seen.
        None => "left:0;top:0;visibility:hidden".to_owned(),
    };
    let list_attrs = StoredValue::new((list_id.clone(), labelledby.clone()));
    view! {
        <button
            node_ref=node_ref
            id=id.clone()
            class="select"
            type="button"
            role="combobox"
            aria-haspopup="listbox"
            aria-expanded=move || bool_attr(open.get())
            aria-controls=list_id
            aria-activedescendant=move || open.get().then(|| active.get().map(option_id)).flatten()
            aria-label=label
            aria-labelledby=labelledby
            aria-invalid=move || bool_attr(invalid.get())
            disabled=disabled
            on:click=move |_| {
                if open.get_untracked() {
                    close();
                } else {
                    // Safari doesn't focus buttons on a click; the keys need the focus here.
                    if let Some(trigger) = node_ref.get_untracked() {
                        let _ = trigger.focus();
                    }
                    show(None);
                }
            }
            on:keydown=on_key
            on:blur=move |_| close()
        >
            <span class="select__value" class:select__value--empty=move || chosen.get().is_none()>{shown}</span>
        </button>
        <Show when=move || open.get()>
            <Portal>
                <ul
                    node_ref=list
                    id=list_attrs.with_value(|a| a.0.clone())
                    class="listbox"
                    class:listbox--above=move || placement.get().is_some_and(|p| p.above)
                    role="listbox"
                    tabindex="-1"
                    aria-label=label
                    aria-labelledby=list_attrs.with_value(|a| a.1.clone())
                    style=list_style
                    // Clicks in the popup keep the focus on the button.
                    on:mousedown=|e| e.prevent_default()
                >
                    {move || {
                        options
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, (_, name))| {
                                view! {
                                    <li
                                        id=option_id(i)
                                        class="listbox__option"
                                        role="option"
                                        aria-selected=move || bool_attr(chosen.get() == Some(i))
                                        data-active=move || (active.get() == Some(i)).then_some("")
                                        on:mousemove=move |_| if active.get_untracked() != Some(i) { active.set(Some(i)) }
                                        on:click=move |_| choose(i)
                                    >
                                        {name}
                                    </li>
                                }
                            })
                            .collect_view()
                    }}
                </ul>
            </Portal>
        </Show>
    }
}

/// The logic behind [`Select`]'s listbox, apart from the DOM: moving the highlight, matching
/// typed text and placing the popup.
pub mod listbox {
    /// How far a page key moves the highlight.
    pub const PAGE: usize = 10;
    /// How long typed letters keep adding to one query, in milliseconds.
    pub const TYPE_AHEAD_MS: f64 = 700.0;
    /// The popup's distance from its button, in pixels.
    pub const GAP: f64 = 4.0;
    /// The least room the popup leaves at the viewport's edges, in pixels.
    pub const MARGIN: f64 = 8.0;
    /// The tallest the popup grows before it scrolls, in pixels.
    pub const MAX_HEIGHT: f64 = 320.0;

    /// A move of the highlight.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Move {
        /// Down one.
        Next,
        /// Up one.
        Previous,
        /// To the first option.
        First,
        /// To the last option.
        Last,
        /// Down a page.
        PageDown,
        /// Up a page.
        PageUp,
    }

    /// The highlighted option after `to`, from `active`, among `len` options. Moves stop at
    /// the ends rather than wrapping, as native selects do.
    pub fn moved(active: Option<usize>, len: usize, to: Move) -> Option<usize> {
        let last = len.checked_sub(1)?;
        Some(match (to, active) {
            (Move::First, _) => 0,
            (Move::Last, _) => last,
            (Move::Next, None) | (Move::PageDown, None) => 0,
            (Move::Previous, None) | (Move::PageUp, None) => last,
            (Move::Next, Some(i)) => (i + 1).min(last),
            (Move::Previous, Some(i)) => i.saturating_sub(1).min(last),
            (Move::PageDown, Some(i)) => (i + PAGE).min(last),
            (Move::PageUp, Some(i)) => i.saturating_sub(PAGE).min(last),
        })
    }

    /// The letters typed in quick succession, which together pick an option.
    #[derive(Clone, Debug, Default)]
    pub struct TypeAhead {
        query: String,
        at: f64,
    }

    impl TypeAhead {
        /// Adds `c`, typed at `now` (in milliseconds), and returns the query. A pause longer
        /// than [`TYPE_AHEAD_MS`] starts a new one.
        pub fn push(&mut self, c: char, now: f64) -> &str {
            if !self.active(now) {
                self.query.clear();
            }
            self.query.extend(c.to_lowercase());
            self.at = now;
            &self.query
        }

        /// Whether a query is under way at `now`, so a space adds to it rather than choosing.
        pub fn active(&self, now: f64) -> bool {
            !self.query.is_empty() && now - self.at <= TYPE_AHEAD_MS
        }
    }

    /// The option whose name starts with `query`, ignoring case, searching from `from` and
    /// wrapping around. A query of one letter, or of one letter repeated, moves on to the next
    /// match after `from`, so pressing a letter again cycles through the names it starts.
    pub fn matching(
        options: &[(String, String)],
        query: &str,
        from: Option<usize>,
    ) -> Option<usize> {
        let len = options.len();
        let mut letters = query.chars();
        let first = letters.next()?;
        let repeated = letters.all(|c| c == first);
        let (query, start) = match repeated {
            true => (first.to_string(), from.map_or(0, |i| i + 1)),
            false => (query.to_owned(), from.unwrap_or(0)),
        };
        (0..len)
            .map(|k| (start + k) % len)
            .find(|&i| options[i].1.trim_start().to_lowercase().starts_with(&query))
    }

    /// A rectangle in viewport pixels.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Rect {
        /// Its left edge.
        pub left: f64,
        /// Its top edge.
        pub top: f64,
        /// Its width.
        pub width: f64,
        /// Its height.
        pub height: f64,
    }

    /// Where the popup goes, in viewport pixels.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Placement {
        /// Its left edge.
        pub left: f64,
        /// Its top edge.
        pub top: f64,
        /// At least the button's width.
        pub min_width: f64,
        /// The most it may grow before scrolling.
        pub max_height: f64,
        /// Whether it opens above the button.
        pub above: bool,
    }

    /// Places a popup of natural `size` (width and height) against `button`, inside a
    /// `viewport` (width and height): below it, unless it doesn't fit there and there is more
    /// room above. It is shifted left to stay on screen.
    pub fn place(button: Rect, size: (f64, f64), viewport: (f64, f64)) -> Placement {
        let (width, height) = (size.0.max(button.width), size.1.min(MAX_HEIGHT));
        let below = viewport.1 - (button.top + button.height) - GAP - MARGIN;
        let above_room = button.top - GAP - MARGIN;
        let above = height > below && above_room > below;
        let max_height = if above { above_room } else { below }.clamp(0.0, MAX_HEIGHT);
        let shown = height.min(max_height);
        let top = match above {
            true => button.top - GAP - shown,
            false => button.top + button.height + GAP,
        };
        let left = button
            .left
            .min(viewport.0 - MARGIN - width)
            .max(MARGIN.min(button.left));
        Placement {
            left,
            top,
            min_width: button.width,
            max_height,
            above,
        }
    }

    /// `value` moved by `by` and kept within `min` and `max`, for a number field's buttons.
    pub fn stepped(value: f64, by: f64, min: f64, max: f64) -> f64 {
        (value + by).clamp(min, max)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn named(names: &[&str]) -> Vec<(String, String)> {
            names
                .iter()
                .map(|n| (n.to_lowercase(), (*n).to_owned()))
                .collect()
        }

        #[test]
        fn moves_stop_at_the_ends() {
            assert_eq!(moved(Some(0), 3, Move::Next), Some(1));
            assert_eq!(moved(Some(2), 3, Move::Next), Some(2));
            assert_eq!(moved(Some(0), 3, Move::Previous), Some(0));
            assert_eq!(moved(None, 3, Move::Next), Some(0));
            assert_eq!(moved(None, 3, Move::Previous), Some(2));
            assert_eq!(moved(Some(1), 3, Move::First), Some(0));
            assert_eq!(moved(Some(1), 3, Move::Last), Some(2));
            assert_eq!(moved(Some(3), 30, Move::PageDown), Some(13));
            assert_eq!(moved(Some(25), 30, Move::PageDown), Some(29));
            assert_eq!(moved(Some(5), 30, Move::PageUp), Some(0));
            assert_eq!(moved(Some(1), 0, Move::Next), None);
            // An index left over from a longer list comes back inside.
            assert_eq!(moved(Some(9), 3, Move::Previous), Some(2));
        }

        #[test]
        fn typing_finds_names_and_repeats_cycle() {
            let options = named(&["Audio", "Archive", "Books", "audiobooks"]);
            assert_eq!(matching(&options, "b", None), Some(2));
            assert_eq!(matching(&options, "au", None), Some(0));
            assert_eq!(matching(&options, "au", Some(1)), Some(3));
            // One letter, typed again, goes on to the next name it starts.
            assert_eq!(matching(&options, "a", Some(0)), Some(1));
            assert_eq!(matching(&options, "aa", Some(1)), Some(3));
            assert_eq!(matching(&options, "a", Some(3)), Some(0));
            assert_eq!(matching(&options, "z", Some(0)), None);
            assert_eq!(matching(&options, "", Some(0)), None);
            assert_eq!(matching(&[], "a", None), None);
        }

        #[test]
        fn type_ahead_starts_over_after_a_pause() {
            let mut typed = TypeAhead::default();
            assert!(!typed.active(0.0));
            assert_eq!(typed.push('A', 0.0), "a");
            assert_eq!(typed.push('u', 300.0), "au");
            assert!(typed.active(900.0));
            assert!(!typed.active(1100.0));
            assert_eq!(typed.push('b', 1100.0), "b");
        }

        #[test]
        fn popups_open_below_unless_above_has_more_room() {
            let viewport = (1000.0, 800.0);
            let button = |top| Rect {
                left: 100.0,
                top,
                width: 160.0,
                height: 30.0,
            };
            let below = place(button(100.0), (120.0, 200.0), viewport);
            assert!(!below.above);
            assert_eq!(below.top, 134.0);
            assert_eq!(below.min_width, 160.0);
            assert_eq!(below.max_height, MAX_HEIGHT);

            // Near the bottom it flips, ending just above the button.
            let above = place(button(700.0), (120.0, 200.0), viewport);
            assert!(above.above);
            assert_eq!(above.top, 700.0 - GAP - 200.0);

            // Too tall for either side, it takes the roomier one and scrolls.
            let tall = place(button(300.0), (120.0, 1000.0), (1000.0, 500.0));
            assert!(tall.above);
            assert_eq!(tall.max_height, 300.0 - GAP - MARGIN);
            assert_eq!(tall.top, MARGIN);

            // It stays inside the right edge.
            let wide = place(
                Rect {
                    left: 900.0,
                    ..button(100.0)
                },
                (300.0, 100.0),
                viewport,
            );
            assert_eq!(wide.left, 1000.0 - MARGIN - 300.0);
        }

        #[test]
        fn steps_stay_in_range() {
            assert_eq!(stepped(4.0, 1.0, 0.0, 5.0), 5.0);
            assert_eq!(stepped(5.0, 1.0, 0.0, 5.0), 5.0);
            assert_eq!(stepped(0.5, -1.0, 0.0, 5.0), 0.0);
        }
    }
}
