//! The UI kit's select in a real browser: its keyboard, its popup and where focus goes.
//!
//! Run it like `live_view`, with `wasm-bindgen-test-runner` and a chromedriver.

use leptos::prelude::*;
use tempest_ui_kit::components::Select;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::wasm_bindgen_test;

/// A mounted select over three options, starting at the second, and the values it reported.
struct Mounted {
    host: web_sys::HtmlElement,
    button: web_sys::HtmlElement,
    changes: RwSignal<Vec<String>>,
    _mounted: Box<dyn std::any::Any>,
}

fn mount() -> Mounted {
    let options: Vec<(String, String)> = ["Audio", "Books", "Video"]
        .iter()
        .map(|n| (n.to_lowercase(), (*n).to_owned()))
        .collect();
    let value = RwSignal::new("books".to_owned());
    let changes = RwSignal::new(Vec::new());
    let host: web_sys::HtmlElement = document().create_element("div").unwrap().unchecked_into();
    document().body().unwrap().append_child(&host).unwrap();
    let mounted = mount_to(host.clone(), move || {
        view! {
            <Select
                label="Category"
                options=options.clone()
                value=value
                on_change=move |v: String| {
                    value.set(v.clone());
                    changes.update(|c| c.push(v));
                }
            />
        }
    });
    let button: web_sys::HtmlElement = host
        .query_selector("[role=combobox]")
        .unwrap()
        .unwrap()
        .unchecked_into();
    button.focus().unwrap();
    Mounted {
        host,
        button,
        changes,
        _mounted: Box::new(mounted),
    }
}

/// Presses `key` on `target`, lets the page update and says whether something prevented the
/// key's default.
async fn press(target: &web_sys::HtmlElement, key: &str) -> bool {
    let init = web_sys::KeyboardEventInit::new();
    init.set_key(key);
    init.set_bubbles(true);
    init.set_cancelable(true);
    let event =
        web_sys::KeyboardEvent::new_with_keyboard_event_init_dict("keydown", &init).unwrap();
    target.dispatch_event(&event).unwrap();
    settle().await;
    event.default_prevented()
}

/// Lets effects run and the popup mount: a frame, then a task.
async fn settle() {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        window().request_animation_frame(&resolve).unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        window().set_timeout_with_callback(&resolve).unwrap();
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

fn listbox() -> Option<web_sys::Element> {
    document().query_selector("[role=listbox]").unwrap()
}

fn active_name(button: &web_sys::HtmlElement) -> Option<String> {
    let id = button.get_attribute("aria-activedescendant")?;
    document().get_element_by_id(&id)?.text_content()
}

#[wasm_bindgen_test]
async fn the_keyboard_opens_moves_chooses_and_closes() {
    let m = mount();
    assert_eq!(m.button.text_content().as_deref(), Some("Books"));
    assert!(listbox().is_none());

    // Down opens on the chosen option; the arrows, Home and End move.
    assert!(press(&m.button, "ArrowDown").await);
    assert_eq!(
        m.button.get_attribute("aria-expanded").as_deref(),
        Some("true")
    );
    assert!(listbox().is_some());
    assert_eq!(active_name(&m.button).as_deref(), Some("Books"));
    press(&m.button, "ArrowDown").await;
    assert_eq!(active_name(&m.button).as_deref(), Some("Video"));
    press(&m.button, "ArrowDown").await;
    assert_eq!(active_name(&m.button).as_deref(), Some("Video"));
    press(&m.button, "Home").await;
    assert_eq!(active_name(&m.button).as_deref(), Some("Audio"));
    press(&m.button, "End").await;
    assert_eq!(active_name(&m.button).as_deref(), Some("Video"));

    // Escape closes without choosing, and stops there so dialogs stay open.
    assert!(press(&m.button, "Escape").await);
    settle().await;
    assert!(listbox().is_none());
    assert!(m.changes.get_untracked().is_empty());

    // Typing opens on a match, and enter chooses it and keeps the focus.
    press(&m.button, "a").await;
    settle().await;
    assert_eq!(active_name(&m.button).as_deref(), Some("Audio"));
    press(&m.button, "Enter").await;
    settle().await;
    assert!(listbox().is_none());
    assert_eq!(m.changes.get_untracked(), ["audio"]);
    assert_eq!(m.button.text_content().as_deref(), Some("Audio"));
    assert_eq!(
        document().active_element().map(|e| e.id()),
        Some(m.button.id())
    );

    // Closed, escape is left to the page.
    assert!(!press(&m.button, "Escape").await);
    m.host.remove();
}

#[wasm_bindgen_test]
async fn clicks_choose_and_clicks_outside_close() {
    let m = mount();
    m.button.click();
    settle().await;
    let list = listbox().expect("a click opens it");
    let options = list.query_selector_all("[role=option]").unwrap();
    assert_eq!(options.length(), 3);
    let books: web_sys::Element = options.item(1).unwrap().unchecked_into();
    assert_eq!(
        books.get_attribute("aria-selected").as_deref(),
        Some("true")
    );
    options
        .item(2)
        .unwrap()
        .unchecked_into::<web_sys::HtmlElement>()
        .click();
    settle().await;
    assert!(listbox().is_none());
    assert_eq!(m.changes.get_untracked(), ["video"]);

    // A click outside closes it without choosing.
    m.button.click();
    settle().await;
    assert!(listbox().is_some());
    let click = web_sys::MouseEvent::new_with_mouse_event_init_dict("click", &{
        let init = web_sys::MouseEventInit::new();
        init.set_bubbles(true);
        init
    })
    .unwrap();
    document().body().unwrap().dispatch_event(&click).unwrap();
    settle().await;
    assert!(listbox().is_none());
    assert_eq!(m.changes.get_untracked(), ["video"]);
    m.host.remove();
}
