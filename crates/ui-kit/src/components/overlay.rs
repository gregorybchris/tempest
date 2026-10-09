//! Windows over the page: the drawer and the dialog, which hold focus while they show.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsCast;

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
    /// A class beside `dialog`, for a dialog laid out its own way.
    #[prop(optional)]
    class: &'static str,
    /// Its contents.
    children: ChildrenFn,
) -> impl IntoView {
    close_on_escape(open, on_close);
    let actions = StoredValue::new(actions);
    let class = format!("dialog {class}").trim_end().to_owned();
    view! {
        <Show when=move || open.get()>
            <div class="scrim" on:click=move |_| on_close.run(())></div>
            <div class=class.clone() role="dialog" aria-modal="true" aria-label=move || title.get()
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
