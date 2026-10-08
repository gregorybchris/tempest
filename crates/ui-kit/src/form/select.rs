//! The kit's drop-down: a button that opens a listbox it draws itself (D-219).

use leptos::ev;
use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use leptos_use::{
    OnClickOutsideOptions, UseEventListenerOptions, on_click_outside_with_options,
    use_event_listener, use_event_listener_with_options,
};
use wasm_bindgen::JsCast;

use super::{listbox, unique_id};
use crate::theme::bool_attr;

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
