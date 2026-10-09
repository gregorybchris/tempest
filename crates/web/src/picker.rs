//! The path picker (D-258): a dialog that browses the engine's own folders, opened by a
//! Browse button beside each field that takes a path on the engine's machine. A browser's
//! file picker can't stand in: it never reveals real paths, and remotely it shows the wrong
//! disk.
//!
//! Its logic is in [`crate::browse`]; this module draws it, keyed and windowed like the
//! transfers list (`viewport`), and remembers per browser where each kind of field last
//! picked from.

use leptos::ev;
use leptos::html::{Div, Input};
use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::folders::{EntryInfo, EntryKind, FolderInfo, MAX_ENTRIES, PlaceInfo, PlaceKind};
use tempest_api::settings::absolute;
use tempest_ui_kit::components::{Button, ButtonKind, Dialog, Select};
use tempest_ui_kit::format;
use tempest_ui_kit::theme::bool_attr;
use wasm_bindgen::JsCast;

use crate::api;
use crate::browse::{self, Action, Browser, Choice, Command, Failure, Mode, Press, Purpose};
use crate::paths;
use crate::request::RequestError;
use crate::signin::Auth;
use crate::viewport::View;

/// A row's height until one is measured; `web.css` sets it.
const ROW: f64 = 30.0;

/// A picker asked for: what for, where the field stands, and who hears the choice.
#[derive(Clone)]
struct Pick {
    purpose: Purpose,
    start: String,
    chosen: Callback<String>,
}

/// The picker, shared as context: at most one is open.
#[derive(Clone, Copy)]
pub struct Picker(RwSignal<Option<Pick>>);

impl Picker {
    /// No picker open.
    pub fn new() -> Self {
        Self(RwSignal::new(None))
    }
}

impl Default for Picker {
    fn default() -> Self {
        Self::new()
    }
}

/// The "Browse…" button beside a path field: opens the picker at the field's `value` and
/// gives `on_choose` the path chosen.
#[component]
pub fn Browse(
    /// What the path is for.
    purpose: Purpose,
    /// The field's text, where the picker opens if it is a path there.
    #[prop(into)]
    value: Signal<String>,
    /// Called with the path chosen.
    #[prop(into)]
    on_choose: Callback<String>,
    /// Whether it is disabled.
    #[prop(optional, into)]
    disabled: Signal<bool>,
    /// A smaller size, for rows.
    #[prop(optional)]
    small: bool,
) -> impl IntoView {
    let picker = expect_context::<Picker>();
    let open = move |_| {
        picker.0.set(Some(Pick {
            purpose,
            start: value.get_untracked(),
            chosen: on_choose,
        }));
    };
    let class = if small {
        "btn btn--small browse"
    } else {
        "btn browse"
    };
    view! {
        <button class=class type="button" aria-haspopup="dialog" disabled=disabled
            title="Choose from the engine's folders" on:click=open>
            "Browse…"
        </button>
    }
}

/// The picker, while one is open.
#[component]
pub fn PickerHost() -> impl IntoView {
    let picker = expect_context::<Picker>();
    move || picker.0.get().map(|pick| view! { <PickerDialog pick /> })
}

/// Where each kind of field last picked from, and whether hidden entries show, kept in this
/// browser's storage. Storage can be missing or refuse, which forgets.
mod memory {
    use leptos::prelude::window;

    use crate::browse::Purpose;

    const PREFIX: &str = "tempest.picker.";

    fn storage() -> Option<web_sys::Storage> {
        window().local_storage().ok().flatten()
    }

    fn read(name: &str) -> Option<String> {
        storage()?
            .get_item(&format!("{PREFIX}{name}"))
            .ok()
            .flatten()
    }

    fn write(name: &str, value: &str) {
        if let Some(storage) = storage() {
            let _ = storage.set_item(&format!("{PREFIX}{name}"), value);
        }
    }

    /// The folder fields like `purpose`'s last picked from.
    pub fn last(purpose: Purpose) -> Option<String> {
        read(purpose.memory())
    }

    /// Remembers that a field like `purpose`'s picked from `folder`.
    pub fn remember(purpose: Purpose, folder: &str) {
        write(purpose.memory(), folder);
    }

    /// Whether hidden entries show.
    pub fn hidden() -> bool {
        read("hidden").as_deref() == Some("true")
    }

    /// Remembers whether hidden entries show.
    pub fn set_hidden(hidden: bool) {
        write("hidden", if hidden { "true" } else { "false" });
    }
}

/// What couldn't be read at `path`, from the engine's answer.
fn failure(path: &str, error: &RequestError) -> Failure {
    Failure {
        path: path.to_owned(),
        code: error.code().unwrap_or("unreachable").to_owned(),
        message: error.to_string(),
    }
}

/// A place's mark in the list of places.
fn place_glyph(place: &PlaceInfo) -> &'static str {
    match place.kind {
        PlaceKind::Home => "~",
        PlaceKind::SaveFolder => "↓",
        PlaceKind::Volume => "⏏",
        PlaceKind::Drive => "◫",
        PlaceKind::Root => "/",
        PlaceKind::Folder => match place.name.as_str() {
            "Desktop" => "▭",
            "Documents" => "≡",
            "Downloads" => "↓",
            "Movies" | "Videos" => "▶",
            "Music" => "♪",
            "Pictures" => "◩",
            _ => "▸",
        },
    }
}

/// The DOM ID of the row of the entry at `key` in listing `generation`.
fn row_id(generation: u64, key: usize) -> String {
    format!("pick-{generation}-{key}")
}

/// The height of the list's rows, measured once one is drawn.
fn row_height(list: &web_sys::HtmlElement) -> f64 {
    list.query_selector(".picker__row")
        .ok()
        .flatten()
        .map(|row| row.get_bounding_client_rect().height())
        .filter(|&h| h > 0.0)
        .unwrap_or(ROW)
}

#[component]
fn PickerDialog(pick: Pick) -> impl IntoView {
    let picker = expect_context::<Picker>();
    let auth = expect_context::<RwSignal<Auth>>();
    let remote = matches!(auth.get_untracked(), Auth::SignedIn { remote: true });
    let Pick {
        purpose,
        start,
        chosen,
    } = pick;
    let mode = purpose.mode();
    let browser = RwSignal::new(Browser::new(mode, memory::hidden()));
    // A new listing gets new row IDs, so no row is reused for another folder's entry.
    let generation = RwSignal::new(0_u64);
    let loading = RwSignal::new(true);
    let places = RwSignal::new(Vec::<PlaceInfo>::new());
    let editing = RwSignal::new(None::<String>);
    let edit_error = RwSignal::new(None::<String>);
    // Answers to all but the latest request are dropped.
    let sequence = StoredValue::new(0_u64);
    let list = NodeRef::<Div>::new();
    let filter_box = NodeRef::<Input>::new();
    let path_box = NodeRef::<Input>::new();
    let view_box = RwSignal::new(View {
        top: 0.0,
        height: 480.0,
        row: ROW,
    });
    let offset = -(js_sys::Date::new_0().get_timezone_offset() as i64);

    let close = move || picker.0.set(None);
    let next_request = move || {
        let next = sequence.get_value() + 1;
        sequence.set_value(next);
        next
    };
    let current = move |request: u64| sequence.try_get_value() == Some(request);
    let focus_list = move || {
        request_animation_frame(move || {
            if let Some(list) = list.get_untracked() {
                let _ = list.focus();
            }
        });
    };
    let measure = move || {
        let Some(list) = list.get_untracked() else {
            return;
        };
        let view = View {
            top: -f64::from(list.scroll_top()),
            height: f64::from(list.client_height()),
            row: row_height(&list),
        };
        if view_box.with_untracked(|v| *v != view) {
            view_box.set(view);
        }
    };
    // Scrolls the list just enough to show the cursor's row.
    let reveal = move || {
        let (Some(list), Some(at)) = (
            list.get_untracked(),
            browser.with_untracked(Browser::cursor),
        ) else {
            return;
        };
        measure();
        let view = view_box.get_untracked();
        let by = view.scroll_to(at, 0.0, view.height);
        if by != 0.0 {
            list.set_scroll_top(list.scroll_top() + by.round() as i32);
            measure();
        }
    };
    let settle = move || {
        request_animation_frame(move || {
            measure();
            reveal();
        });
    };
    let show = move |folder: FolderInfo, select: Option<String>| {
        browser.update(|b| b.show(folder, select.as_deref()));
        generation.update(|g| *g += 1);
        if let Some(list) = list.get_untracked() {
            list.set_scroll_top(0);
        }
        settle();
    };
    let fail = move |failure: Failure| {
        browser.update(|b| b.fail(failure));
        generation.update(|g| *g += 1);
    };
    let navigate = move |path: String, select: Option<String>| {
        let request = next_request();
        loading.set(true);
        editing.set(None);
        edit_error.set(None);
        spawn_local(async move {
            let answer = api::folder(Some(&path)).await;
            if !current(request) {
                return;
            }
            loading.set(false);
            match answer {
                Ok(folder) => show(folder, select),
                Err(error) => fail(failure(&path, &error)),
            }
        });
    };
    let finish = move |choice: Choice| {
        memory::remember(purpose, &choice.from);
        close();
        chosen.try_run(choice.path);
    };
    let act = move |action: Action| match action {
        Action::Open(path) => navigate(path, None),
        Action::Back(path, name) => navigate(path, Some(name)),
        Action::Choose(choice) => finish(choice),
    };
    let set_filter = move |text: String| {
        browser.update(|b| b.set_filter(&text));
        settle();
    };
    let set_hidden = move |hidden: bool| {
        browser.update(|b| b.set_hidden(hidden));
        memory::set_hidden(hidden);
        settle();
    };
    let start_edit = move |first: Option<char>| {
        let text = match first {
            Some(c) => c.to_string(),
            None => browser.with_untracked(|b| b.location().unwrap_or_default().to_owned()),
        };
        edit_error.set(None);
        editing.set(Some(text));
        request_animation_frame(move || {
            if let Some(input) = path_box.get_untracked() {
                let _ = input.focus();
                if first.is_none() {
                    input.select();
                }
            }
        });
    };
    let cancel_edit = move || {
        editing.set(None);
        edit_error.set(None);
        focus_list();
    };
    // Goes to a typed path. Mistakes show under the field, which stays open.
    let jump = move |text: String| {
        let home = places.with_untracked(|p| {
            p.iter()
                .find(|p| p.kind == PlaceKind::Home)
                .map(|p| p.path.clone())
        });
        let path = paths::expand(text.trim(), home.as_deref());
        if !absolute(&path) {
            edit_error.set(Some(
                r"Type an absolute path, such as /home or C:\Users.".to_owned(),
            ));
            return;
        }
        let request = next_request();
        loading.set(true);
        spawn_local(async move {
            let answer = api::folder(Some(&path)).await;
            if !current(request) {
                return;
            }
            loading.set(false);
            match answer {
                Ok(folder) => {
                    editing.set(None);
                    show(folder, None);
                    focus_list();
                }
                // A file: its folder, with it under the cursor.
                Err(error) if error.code() == Some("not_a_folder") => {
                    match (mode, paths::parent(&path), paths::name(&path)) {
                        (Mode::File | Mode::Any, Some(parent), Some(name)) => {
                            navigate(parent, Some(name.to_owned()));
                            focus_list();
                        }
                        _ => edit_error.set(Some(
                            "That's a file. Choose a folder, or type the one it's in.".to_owned(),
                        )),
                    }
                }
                Err(error) => edit_error.set(Some(match error.code() {
                    Some("not_found") => "Nothing is there on the engine's machine.".to_owned(),
                    Some("permission_denied") => {
                        "The engine's user isn't allowed to read that folder.".to_owned()
                    }
                    _ => error.to_string(),
                })),
            }
        });
    };

    // Opens where the field's path is, else where this kind of field last picked from, else
    // where the engine starts browsing. A file opens its folder with it under the cursor.
    let start = start.trim().to_owned();
    let mut candidates = Vec::new();
    if absolute(&start) {
        candidates.push(start.clone());
        candidates.extend(paths::parent(&start));
    }
    candidates.extend(memory::last(purpose));
    let first = next_request();
    spawn_local(async move {
        let mut last = None;
        for candidate in candidates.into_iter().map(Some).chain([None]) {
            let answer = api::folder(candidate.as_deref()).await;
            if !current(first) {
                return;
            }
            let error = match answer {
                Ok(folder) => {
                    loading.set(false);
                    show(folder, None);
                    return;
                }
                Err(error) => error,
            };
            let file = candidate
                .as_deref()
                .filter(|_| error.code() == Some("not_a_folder"));
            if let Some((parent, name)) =
                file.and_then(|path| Some((paths::parent(path)?, paths::name(path)?.to_owned())))
                && let Ok(folder) = api::folder(Some(&parent)).await
                && current(first)
            {
                loading.set(false);
                show(folder, Some(name));
                return;
            }
            last = Some(failure(candidate.as_deref().unwrap_or("/"), &error));
        }
        loading.set(false);
        if let Some(failure) = last {
            fail(failure);
        }
    });
    spawn_local(async move {
        if let Ok(found) = api::places().await {
            let _ = places.try_set(found);
        }
    });
    focus_list();
    let on_resize = window_event_listener(ev::resize, move |_| measure());
    on_cleanup(move || on_resize.remove());

    // Rows per page, for page up and down.
    let page = move || {
        view_box
            .with_untracked(|v| (v.height / v.row.max(1.0)).floor() as usize)
            .saturating_sub(1)
    };
    let run = move |command: Command| match command {
        Command::Move(step) => {
            browser.update(|b| b.go(step, page()));
            reveal();
        }
        Command::Enter => {
            if let Some(action) = browser.with_untracked(Browser::enter) {
                act(action);
            }
        }
        Command::Open => {
            if let Some(action) = browser.with_untracked(Browser::open) {
                act(action);
            }
        }
        Command::Up => {
            if let Some(action) = browser.with_untracked(Browser::up) {
                act(action);
            }
        }
        Command::Choose => {
            if let Some(choice) = browser.with_untracked(Browser::choice) {
                finish(choice);
            }
        }
        Command::Hidden => set_hidden(!browser.with_untracked(Browser::hidden)),
        Command::EditPath(first) => start_edit(first),
        Command::Escape => {
            if browser.with_untracked(|b| b.filter().is_empty()) {
                close();
            } else {
                set_filter(String::new());
                focus_list();
            }
        }
        Command::Type(c) => {
            let mut text = browser.with_untracked(|b| b.filter().to_owned());
            text.push(c);
            set_filter(text);
            if let Some(input) = filter_box.get_untracked() {
                let _ = input.focus();
            }
        }
        Command::Erase => {
            let mut text = browser.with_untracked(|b| b.filter().to_owned());
            text.pop();
            set_filter(text);
        }
    };
    let on_key = move |e: ev::KeyboardEvent| {
        if e.default_prevented() {
            return;
        }
        let Some(target) = e
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        let key = e.key();
        // Enter and space on a button are the button's.
        if target.tag_name() == "BUTTON" && matches!(key.as_str(), "Enter" | " ") {
            return;
        }
        // The path field takes every key; it handles enter and esc itself.
        let in_path = path_box
            .get_untracked()
            .is_some_and(|p| p.unchecked_ref::<web_sys::HtmlElement>() == &target);
        if in_path {
            return;
        }
        let filter = filter_box.get_untracked();
        let in_filter = filter
            .as_ref()
            .is_some_and(|f| f.unchecked_ref::<web_sys::HtmlElement>() == &target);
        let filter_empty = browser.with_untracked(|b| b.filter().is_empty());
        let at_end = filter.as_ref().is_some_and(|f| {
            let end = f.value().encode_utf16().count() as u32;
            f.selection_start().ok().flatten() == Some(end)
                && f.selection_end().ok().flatten() == Some(end)
        });
        let press = Press {
            key: &key,
            command: e.ctrl_key() || e.meta_key(),
            alt: e.alt_key(),
            in_filter,
            filter_empty,
            at_end,
        };
        if let Some(command) = browse::command(press) {
            e.prevent_default();
            run(command);
        }
    };

    let title = purpose.title().to_owned();
    let actions = move || {
        let statement = move || {
            browser.with(|b| match b.choice() {
                Some(choice) => view! {
                    <span class="picker__verb">{purpose.verb()}</span>
                    " "
                    <span class="picker__chosen" title=choice.path.clone()><bdi>{choice.path.clone()}</bdi></span>
                }
                .into_any(),
                None if b.folder().is_none() => view! { <span class="dim">"Nothing to choose here."</span> }.into_any(),
                None => view! { <span class="dim">"Choose a file: enter on one, or a double click."</span> }.into_any(),
            })
        };
        let label = move || {
            let current = browser.with(|b| b.choice().is_some_and(|c| c.current));
            match (mode, current) {
                (Mode::Folder, true) => "Use this folder",
                _ => "Choose",
            }
        };
        let none = Signal::derive(move || loading.get() || browser.with(|b| b.choice().is_none()));
        view! {
            <p class="picker__statement" aria-live="polite">{statement}</p>
            <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
            <Button kind=ButtonKind::Primary disabled=none title=browse::hints(mac())[4].0 on_click=move || run(Command::Choose)>
                {label}
            </Button>
        }
    };

    view! {
        <Dialog open=true on_close=close title=title class="dialog--picker" actions=actions>
            <div class="picker" on:keydown=on_key>
                <PathBar browser editing edit_error path_box navigate start_edit cancel_edit jump remote />
                <div class="picker__toolbar">
                    <PlaceMenu browser places navigate />
                    <div class="search picker__filter">
                        <input
                            node_ref=filter_box
                            class="input"
                            type="search"
                            placeholder="type to filter"
                            aria-label="Filter the entries"
                            aria-controls="picker-list"
                            spellcheck="false"
                            autocomplete="off"
                            prop:value=move || browser.with(|b| b.filter().to_owned())
                            on:input=move |e| set_filter(event_target_value(&e))
                        />
                    </div>
                    <button
                        class="toggle picker__hidden"
                        type="button"
                        title="Show hidden entries (.)"
                        aria-pressed=move || bool_attr(browser.with(Browser::hidden))
                        on:click=move |_| set_hidden(!browser.with_untracked(Browser::hidden))
                    >
                        "hidden"
                    </button>
                    <span class="picker__count dim" aria-live="polite">{move || count(&browser.read())}</span>
                </div>
                <div class="picker__main">
                    <Places browser places navigate />
                    <Entries browser generation loading list view_box measure act focus_list offset set_filter />
                </div>
                <ul class="picker__hints" aria-hidden="true">
                    {browse::hints(mac()).into_iter().map(|(keys, what)| view! {
                        <li><kbd>{keys}</kbd>" "{what}</li>
                    }).collect_view()}
                </ul>
            </div>
        </Dialog>
    }
}

/// How many entries show: "128 items", or "3 of 128" while filtering.
fn count(browser: &Browser) -> String {
    let Some(folder) = browser.folder() else {
        return String::new();
    };
    let all = folder
        .entries
        .iter()
        .filter(|e| browser.hidden() || !e.hidden)
        .count();
    match (browser.filter().is_empty(), all) {
        (true, 1) => "1 item".to_owned(),
        (true, n) => format!("{} items", format::count(n as u64)),
        (false, n) => format!(
            "{} of {}",
            format::count(browser.len() as u64),
            format::count(n as u64)
        ),
    }
}

/// The path as breadcrumbs, each a button, or a field to type a path into.
#[component]
fn PathBar(
    browser: RwSignal<Browser>,
    editing: RwSignal<Option<String>>,
    edit_error: RwSignal<Option<String>>,
    path_box: NodeRef<Input>,
    navigate: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
    start_edit: impl Fn(Option<char>) + Copy + Send + Sync + 'static,
    cancel_edit: impl Fn() + Copy + Send + Sync + 'static,
    jump: impl Fn(String) + Copy + Send + Sync + 'static,
    remote: bool,
) -> impl IntoView {
    let crumbs_box = NodeRef::<leptos::html::Ol>::new();
    let location = Memo::new(move |_| browser.with(|b| b.location().map(str::to_owned)));
    // Long paths keep their end in view.
    Effect::new(move |_| {
        location.track();
        request_animation_frame(move || {
            if let Some(crumbs) = crumbs_box.get_untracked() {
                crumbs.set_scroll_left(crumbs.scroll_width());
            }
        });
    });
    let crumbs = move || {
        let path = location.get().unwrap_or_default();
        let crumbs = paths::crumbs(&path);
        let separator = if path.starts_with('/') { "/" } else { "\\" };
        let last = crumbs.len().saturating_sub(1);
        crumbs
            .into_iter()
            .enumerate()
            .map(|(i, crumb)| {
                // A root such as `/` or `C:\` ends with its own separator.
                let joins = i < last && !crumb.label.ends_with(['/', '\\']);
                let target = crumb.path.clone();
                view! {
                    <li class="pathbar__crumb">
                        <button
                            type="button"
                            class="pathbar__step"
                            aria-current=(i == last).then_some("location")
                            title=crumb.path.clone()
                            on:click=move |_| navigate(target.clone(), None)
                        >
                            {crumb.label}
                        </button>
                        {joins.then(|| view! { <span class="pathbar__sep" aria-hidden="true">{separator}</span> })}
                    </li>
                }
            })
            .collect_view()
    };
    let field = move || {
        editing.get().is_some().then(|| {
            view! {
                <form class="pathbar__form" on:submit=move |e| {
                    e.prevent_default();
                    jump(editing.get_untracked().unwrap_or_default());
                }>
                    <input
                        node_ref=path_box
                        class="input pathbar__input"
                        type="text"
                        aria-label="Go to a path on the engine's machine"
                        aria-invalid=move || bool_attr(edit_error.with(Option::is_some))
                        aria-describedby="picker-path-error"
                        spellcheck="false"
                        autocomplete="off"
                        placeholder="/path/to/a/folder"
                        prop:value=move || editing.get().unwrap_or_default()
                        on:input=move |e| {
                            editing.set(Some(event_target_value(&e)));
                            edit_error.set(None);
                        }
                        on:keydown=move |e| {
                            if e.key() == "Escape" {
                                e.prevent_default();
                                cancel_edit();
                            }
                        }
                    />
                    <button class="btn btn--small" type="submit">"go"</button>
                    <button class="btn btn--small btn--ghost" type="button" on:click=move |_| cancel_edit()>"cancel"</button>
                </form>
            }
        })
    };
    let host = if remote {
        "The engine's machine, not this device"
    } else {
        "The engine's machine"
    };
    view! {
        <div class="pathbar" data-editing=move || editing.with(Option::is_some).then_some("")>
            <span class="pathbar__host" title=host>"engine:"</span>
            {field}
            <ol class="pathbar__crumbs" node_ref=crumbs_box aria-label="Path"
                hidden=move || editing.with(Option::is_some)
                on:click=move |e| {
                    // A click beside the steps types a path.
                    if e.target() == e.current_target() {
                        start_edit(None);
                    }
                }>
                {crumbs}
            </ol>
            <button class="btn btn--small btn--ghost pathbar__edit" type="button" title="Type a path (ctrl+L)"
                hidden=move || editing.with(Option::is_some)
                on:click=move |_| start_edit(None)>
                "type path"
            </button>
        </div>
        {move || edit_error.get().map(|error| view! {
            <p class="field__error pathbar__error" id="picker-path-error" role="alert">{error}</p>
        })}
        {remote.then(|| view! {
            <p class="picker__remote dim">"These are the engine's folders, on its machine, not this device's."</p>
        })}
    }
}

/// The places, as a column beside the entries.
#[component]
fn Places(
    browser: RwSignal<Browser>,
    places: RwSignal<Vec<PlaceInfo>>,
    navigate: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let location = Memo::new(move |_| browser.with(|b| b.location().map(str::to_owned)));
    let items = move || {
        places
            .get()
            .into_iter()
            .map(|place| {
                let glyph = place_glyph(&place);
                let (path, here) = (place.path.clone(), place.path.clone());
                let current = move || location.with(|l| l.as_deref() == Some(here.as_str()));
                view! {
                    <li>
                        <button
                            class="picker__place"
                            type="button"
                            title=place.path.clone()
                            aria-current=move || current().then_some("location")
                            on:click=move |_| navigate(path.clone(), None)
                        >
                            <span class="picker__place-glyph" aria-hidden="true">{glyph}</span>
                            <span class="picker__place-name">{place.name}</span>
                        </button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <nav class="picker__places" aria-label="Places">
            <h3 class="picker__places-title">"places"</h3>
            <ul>{items}</ul>
        </nav>
    }
}

/// The places as a menu, for narrow windows, which have no room for the column.
#[component]
fn PlaceMenu(
    browser: RwSignal<Browser>,
    places: RwSignal<Vec<PlaceInfo>>,
    navigate: impl Fn(String, Option<String>) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let options = Signal::derive(move || {
        places.with(|p| {
            p.iter()
                .map(|p| (p.path.clone(), format!("{} {}", place_glyph(p), p.name)))
                .collect::<Vec<_>>()
        })
    });
    // The place the picker is at, if it is at one; else the menu shows its placeholder.
    let value = Signal::derive(move || {
        let location = browser.with(|b| b.location().unwrap_or_default().to_owned());
        if places.with(|p| p.iter().any(|p| p.path == location)) {
            location
        } else {
            String::new()
        }
    });
    view! {
        <div class="picker__place-menu">
            <Select options value label="Places" placeholder="places" on_change=move |path: String| navigate(path, None) />
        </div>
    }
}

/// The entries, windowed: only the rows in view are drawn.
#[component]
fn Entries(
    browser: RwSignal<Browser>,
    generation: RwSignal<u64>,
    loading: RwSignal<bool>,
    list: NodeRef<Div>,
    view_box: RwSignal<View>,
    measure: impl Fn() + Copy + Send + Sync + 'static,
    act: impl Fn(Action) + Copy + Send + Sync + 'static,
    focus_list: impl Fn() + Copy + Send + Sync + 'static,
    offset: i64,
    set_filter: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let len = Memo::new(move |_| browser.with(Browser::len));
    let drawn = Memo::new(move |_| view_box.with(|v| v.rows(len.get())));
    let rows = Memo::new(move |_| {
        let range = drawn.get();
        let generation = generation.get();
        browser.with(|b| {
            range
                .filter_map(|at| Some((generation, b.key(at)?, at)))
                .collect::<Vec<_>>()
        })
    });
    let cursor = Selector::new(move || browser.with(|b| b.cursor().and_then(|c| b.key(c))));
    let active = move || {
        let generation = generation.get();
        browser.with(|b| {
            b.cursor()
                .and_then(|c| b.key(c))
                .map(|k| row_id(generation, k))
        })
    };
    let spacer = move |rows: Signal<usize>| {
        let height = move || format!("{}px", rows.get() as f64 * view_box.with(|v| v.row));
        view! { <div class="picker__spacer" aria-hidden="true" style:height=height></div> }
    };
    let above = Signal::derive(move || drawn.with(|r| r.start));
    let below = Signal::derive(move || len.get() - drawn.with(|r| r.end));
    let empty = move || {
        browser.with(|b| {
            if let Some(failure) = b.failure() {
                let (title, text) = match failure.code.as_str() {
                    "permission_denied" => (
                        "No permission",
                        "The engine's user isn't allowed to read this folder.".to_owned(),
                    ),
                    "not_found" => (
                        "Not there",
                        "No folder by this name on the engine's machine.".to_owned(),
                    ),
                    _ => ("Can't read this folder", failure.message.clone()),
                };
                let up = b.up();
                return Some(view! {
                    <div class="picker__empty picker__empty--bad" role="alert">
                        <div class="picker__empty-title">{title}</div>
                        <p>{text}</p>
                        {up.map(|up| view! {
                            <Button small=true on_click=move || act(up.clone())>"← Go up"</Button>
                        })}
                    </div>
                }.into_any());
            }
            if b.len() > 0 {
                return None;
            }
            let folder = b.folder()?;
            Some(if b.all_filtered() && !b.filter().is_empty() {
                let filter = b.filter().to_owned();
                view! {
                    <div class="picker__empty">
                        <div class="picker__empty-title">{format!("Nothing matches “{filter}”")}</div>
                        <p><kbd>"esc"</kbd>" clears the filter."</p>
                        <Button small=true kind=ButtonKind::Ghost on_click=move || { set_filter(String::new()); focus_list(); }>
                            "clear filter"
                        </Button>
                    </div>
                }.into_any()
            } else if !folder.entries.is_empty() {
                view! {
                    <div class="picker__empty">
                        <div class="picker__empty-title">"Only hidden entries"</div>
                        <p><kbd>"."</kbd>" shows them."</p>
                    </div>
                }.into_any()
            } else {
                view! {
                    <div class="picker__empty">
                        <div class="picker__empty-title">"Empty folder"</div>
                        <p>"Nothing in here yet."</p>
                    </div>
                }.into_any()
            })
        })
    };
    let reading = move || {
        (loading.get() && browser.with(|b| b.location().is_none())).then(|| {
            view! {
                <div class="picker__empty" role="status">
                    <div class="picker__empty-title">"Reading the folder"<span class="picker__caret" aria-hidden="true"></span></div>
                </div>
            }
        })
    };
    let truncated = move || {
        browser
            .with(|b| b.folder().is_some_and(|f| f.truncated))
            .then(|| {
                view! {
                    <p class="picker__truncated dim">
                        {format!("Showing the first {} entries. Type a path to go straight to one.", format::count(MAX_ENTRIES as u64))}
                    </p>
                }
            })
    };
    let on_scroll = move |_| measure();
    view! {
        <div class="picker__entries" aria-busy=move || bool_attr(loading.get())>
            <div class="picker__loading" aria-hidden="true" hidden=move || !loading.get()></div>
            <div class="picker__head" aria-hidden="true">
                <span></span>
                <span>"name"</span>
                <span class="num">"size"</span>
                <span class="num picker__date">"modified"</span>
            </div>
            <div
                class="picker__list"
                id="picker-list"
                node_ref=list
                role="listbox"
                tabindex="0"
                aria-label="Entries"
                aria-activedescendant=active
                on:scroll=on_scroll
                on:click=move |e| {
                    if e.target() == e.current_target() {
                        browser.update(Browser::deselect);
                    }
                }
            >
                {spacer(above)}
                <For each=move || rows.get() key=|row| *row let:row>
                    <EntryRow browser generation=row.0 key=row.1 at=row.2 cursor=cursor.clone() act offset />
                </For>
                {spacer(below)}
            </div>
            {reading}
            {empty}
            {truncated}
        </div>
    }
}

#[component]
fn EntryRow(
    browser: RwSignal<Browser>,
    generation: u64,
    key: usize,
    at: usize,
    cursor: Selector<Option<usize>>,
    act: impl Fn(Action) + Copy + Send + Sync + 'static,
    offset: i64,
) -> impl IntoView {
    let Some((entry, choosable)) =
        browser.with_untracked(|b| Some((b.entry(at)?.clone(), b.choosable(at))))
    else {
        return ().into_any();
    };
    let EntryInfo {
        name,
        kind,
        symlink,
        size,
        modified,
        hidden,
    } = entry;
    let folder = kind == EntryKind::Folder;
    let title = name.clone();
    let glyph = if folder {
        "glyph glyph--folder"
    } else {
        "glyph glyph--file"
    };
    let size = size.map(format::bytes).unwrap_or_default();
    let when = modified
        .map(|m| browse::modified(m, offset))
        .unwrap_or_default();
    let click = move |_| browser.update(|b| b.select(at));
    let double = move |_| {
        browser.update(|b| b.select(at));
        if let Some(action) = browser.with_untracked(Browser::enter) {
            act(action);
        }
    };
    view! {
        <div
            class="picker__row"
            id=row_id(generation, key)
            role="option"
            aria-selected=move || bool_attr(cursor.selected(&Some(key)))
            aria-disabled=(!choosable).then_some("true")
            data-kind=if folder { "folder" } else { "file" }
            data-hidden=hidden.then_some("")
            on:click=click
            on:dblclick=double
        >
            <span class=glyph aria-hidden="true"></span>
            <span class="picker__name" title=title>
                {name}
                {folder.then(|| view! { <span class="picker__slash" aria-hidden="true">"/"</span> })}
                {symlink.then(|| view! { <span class="picker__link" title="A link to elsewhere">" ↪"</span> })}
            </span>
            <span class="picker__size num">{size}</span>
            <span class="picker__date num">{when}</span>
        </div>
    }
    .into_any()
}

/// Whether the browser runs on a Mac, where ⌘ stands in for ctrl.
fn mac() -> bool {
    window()
        .navigator()
        .platform()
        .is_ok_and(|platform| platform.starts_with("Mac"))
}
