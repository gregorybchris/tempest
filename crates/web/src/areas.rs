//! The Catalog and Settings areas: the sections [`crate::rows`] makes, each row with the
//! control that changes it in place (D-196). The keyboard's cursor moves through the rows;
//! `enter` uses the row's control, `x` its remove or reset button, and `a` the add field of
//! the cursor's section.

use leptos::html::Input;
use leptos::prelude::*;
use serde_json::json;
use tempest_api::requests::QueueStep;
use tempest_api::schedule::{Schedule, Weekday};
use tempest_api::settings::{ValueKind, keys};
use tempest_api::{Status, TransferInfo};
use tempest_protocol::BundleId;
use tempest_ui_kit::components::*;
use tempest_ui_kit::theme::{ThemePicker, bool_attr};
use wasm_bindgen::JsCast;

use crate::config::Config;
use crate::controls::Controls;
use crate::list::Move;
use crate::notify;
use crate::rows::{self, Adds, Control, Extras, Row, Section};
use crate::state::{Area, Dialog, Ui};
use crate::store::Store;

/// The sections of the Catalog and Settings areas, for the areas and the keyboard.
#[derive(Clone, Copy)]
pub struct Sections {
    /// The Catalog area's.
    pub catalog: Memo<Vec<Section>>,
    /// The Settings area's.
    pub settings: Memo<Vec<Section>>,
}

impl Sections {
    /// The sections as the engine's settings, categories, transfers and the page's columns
    /// make them.
    pub fn new(config: Config, store: Store, ui: Ui) -> Self {
        let catalog = Memo::new(move |_| {
            config
                .info
                .with(|info| info.as_ref().map(rows::catalog_sections))
                .unwrap_or_default()
        });
        let tags = Memo::new(move |_| store.model.with(|m| rows::tag_counts(m.transfers())));
        let settings = Memo::new(move |_| {
            let extras = Extras {
                categories: config.categories.get(),
                tags: tags.get(),
                columns: ui.columns.get(),
            };
            config
                .info
                .with(|info| info.as_ref().map(|i| rows::settings_sections(i, &extras)))
                .unwrap_or_default()
        });
        Self { catalog, settings }
    }

    /// The sections of `area`, untracked.
    pub fn of(self, area: Area) -> Vec<Section> {
        match area {
            Area::Catalog => self.catalog.get_untracked(),
            Area::Settings => self.settings.get_untracked(),
            _ => Vec::new(),
        }
    }

    /// Moves the cursor among the rows of the area shown.
    pub fn go(self, ui: Ui, to: Move) {
        let ids: Vec<String> = self
            .of(ui.area.get_untracked())
            .into_iter()
            .flat_map(|s| s.rows)
            .map(|r| r.id)
            .collect();
        let current = ui.row.get_untracked();
        if let Some(next) = rows::step(&ids, current.as_deref(), to) {
            if let Some(element) = document().get_element_by_id(&dom_id(&next)) {
                let options = web_sys::ScrollIntoViewOptions::new();
                options.set_block(web_sys::ScrollLogicalPosition::Nearest);
                element.scroll_into_view_with_scroll_into_view_options(&options);
            }
            ui.row.set(Some(next));
        }
    }

    /// Uses the control marked `marker` in the cursor's row: focuses a field, or clicks a
    /// button. With `add`, the add field of the cursor's section instead.
    pub fn use_control(self, ui: Ui, marker: &str) {
        let area = ui.area.get_untracked();
        let sections = self.of(area);
        let row = ui.row.get_untracked();
        let element = match marker {
            "data-add" => {
                // The cursor's section, or the first with an add field.
                let section = sections
                    .iter()
                    .find(|s| {
                        s.adds.is_some() && s.rows.iter().any(|r| Some(&r.id) == row.as_ref())
                    })
                    .or_else(|| sections.iter().find(|s| s.adds.is_some()));
                section.and_then(|s| document().get_element_by_id(&format!("section-{}", s.id)))
            }
            _ => row.and_then(|row| document().get_element_by_id(&dom_id(&row))),
        };
        let Some(control) = element
            .and_then(|e| e.query_selector(&format!("[{marker}]")).ok().flatten())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        else {
            return;
        };
        match control.dyn_ref::<web_sys::HtmlInputElement>() {
            Some(input) if input.type_() == "text" => {
                let _ = input.focus();
                input.select();
            }
            _ => control.click(),
        }
    }
}

/// A row's DOM ID: row IDs can hold any text, such as URLs, so they are hashed.
fn dom_id(row: &str) -> String {
    // FNV-1a.
    let hash = row.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("row-{hash:016x}")
}

/// The Catalog area: trackers, relays and, later, what trackers list.
#[component]
pub fn Catalog() -> impl IntoView {
    let sections = expect_context::<Sections>().catalog;
    let ui = expect_context::<Ui>();
    view! {
        <div class="area area--wide">
            <Loading />
            <SectionsView sections />
            <Panel title="catalogs">
                <div class="catalogs">
                    <div class="search">
                        <input class="input" type="search" disabled=true placeholder="search what trackers list…" aria-label="Search catalogs (later)" />
                    </div>
                    <Empty
                        title="Browsing catalogs arrives in a later release"
                        text="Trackers will list what their peers share, to search and add from here (roadmap M9). For now, add a download from its tempest: link."
                    >
                        <Button kind=ButtonKind::Primary on_click=move || ui.dialog.set(Some(Dialog::Add))>"+ Add a link"</Button>
                    </Empty>
                </div>
            </Panel>
        </div>
    }
}

/// The Settings area.
#[component]
pub fn Settings() -> impl IntoView {
    let sections = expect_context::<Sections>().settings;
    let config = expect_context::<Config>();
    let restart = move || {
        let groups = config
            .info
            .with(|i| i.as_ref().map(|i| i.restart_required.clone()))
            .unwrap_or_default();
        (!groups.is_empty()).then(|| {
            view! {
                <div class="banner tone--warn" role="status">
                    {format!("Restart the engine to apply the changes to {}.", groups.join(", "))}
                </div>
            }
        })
    };
    view! {
        <div class="area area--wide">
            <Loading />
            {restart}
            <SectionsView sections />
            <p class="dim note">"• changed here rather than in the config file; reset gives it the file's value again."</p>
        </div>
    }
}

/// A note while the settings load.
#[component]
fn Loading() -> impl IntoView {
    let config = expect_context::<Config>();
    move || {
        config
            .info
            .with(Option::is_none)
            .then(|| view! { <Empty title="Loading" text="Asking the engine for its settings…" /> })
    }
}

/// Every section, each row redrawn only when it changes.
#[component]
fn SectionsView(sections: Memo<Vec<Section>>) -> impl IntoView {
    let ids = Memo::new(move |_| sections.with(|s| s.iter().map(|s| s.id).collect::<Vec<_>>()));
    view! {
        <div class="sections">
            <For each=move || ids.get() key=|id| *id let:id>
                <SectionView id sections />
            </For>
        </div>
    }
}

#[component]
fn SectionView(id: &'static str, sections: Memo<Vec<Section>>) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let section = move || sections.with(|s| s.iter().find(|s| s.id == id).cloned());
    let Some(first) = section() else {
        return ().into_any();
    };
    let note = move || section().and_then(|s| s.note);
    let rows = move || section().map(|s| s.rows).unwrap_or_default();
    let cursor = Selector::new(move || ui.row.get());
    view! {
        <section class="settings-section" id=format!("section-{id}")>
            <Panel title=first.title.to_lowercase()>
                {move || note().map(|n| view! { <p class="settings-section__note dim">{n}</p> })}
                <ul class="rows">
                    // Keyed by the whole row: a row redraws only when something in it changed.
                    <For each=rows key=|row| format!("{row:?}") let:row>
                        <RowView row cursor=cursor.clone() />
                    </For>
                </ul>
                {first.adds.map(|adds| view! { <AddField adds /> })}
                {(id == "queue").then(|| view! { <QueueList /> })}
            </Panel>
        </section>
    }
    .into_any()
}

#[component]
fn RowView(row: Row, cursor: Selector<Option<String>>) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let config = expect_context::<Config>();
    let id = row.id.clone();
    let selected = {
        let id = id.clone();
        move || bool_attr(cursor.selected(&Some(id.clone())))
    };
    let reset = row.reset.map(|key| {
        let label = row.label.clone();
        view! {
            <button class="btn btn--ghost btn--small" type="button" data-remove=""
                title="Give it the config file's value again (x)"
                on:click=move |_| config.reset(key, label.clone())>
                "reset"
            </button>
        }
    });
    let help = row.help.clone();
    view! {
        <li class="row" id=dom_id(&id) aria-selected=selected on:click=move |_| ui.row.set(Some(id.clone()))>
            <div class="row__label">
                {row.label.clone()}
                {row.reset.map(|_| view! { <span class="row__changed" title="Changed here">" •"</span> })}
            </div>
            <div class="row__control">
                <ControlView row=row.clone() />
                {reset}
            </div>
            {(!help.is_empty()).then(|| view! { <p class="row__help">{help}</p> })}
        </li>
    }
}

/// A text field that saves on `enter`, starting with `text`, with a save button while it
/// differs.
#[component]
fn InlineField(
    text: String,
    #[prop(optional)] placeholder: &'static str,
    #[prop(optional)] title: &'static str,
    #[prop(into)] save: Callback<String, Result<(), String>>,
) -> impl IntoView {
    let start = StoredValue::new(text.clone());
    let draft = RwSignal::new(text);
    let problem = RwSignal::new(None::<String>);
    let dirty = move || draft.with(|d| start.with_value(|s| d != s));
    let commit = move || match save.run(draft.get_untracked()) {
        Ok(()) => problem.set(None),
        Err(error) => problem.set(Some(error)),
    };
    view! {
        <form class="inline" on:submit=move |e| { e.prevent_default(); commit(); }>
            <input
                class="input"
                type="text"
                data-change=""
                placeholder=placeholder
                title=title
                spellcheck="false"
                autocomplete="off"
                aria-invalid=move || bool_attr(problem.with(Option::is_some))
                prop:value=move || draft.get()
                on:input=move |e| { draft.set(event_target_value(&e)); problem.set(None); }
                on:keydown=move |e| {
                    if e.key() == "Escape" {
                        draft.set(start.get_value());
                        problem.set(None);
                    }
                }
            />
            <Show when=dirty>
                <button class="btn btn--primary btn--small" type="submit">"save"</button>
            </Show>
            {move || problem.get().map(|p| view! { <span class="field__error inline__error">{p}</span> })}
        </form>
    }
}

/// A switch that changes `key` when clicked.
fn switch(on: bool, flip: impl Fn() + Send + Sync + 'static) -> impl IntoView {
    view! {
        <Switch on=on on_change=move |_| flip() attr:data-change="">
            {if on { "on" } else { "off" }}
        </Switch>
    }
}

#[component]
fn ControlView(row: Row) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let config = expect_context::<Config>();
    let controls = expect_context::<Controls>();
    let label = row.label.clone();
    match row.control {
        Control::Value { key, kind, text } => {
            let about = label.clone();
            let save = move |draft: String| {
                let value = kind.parse(&draft)?;
                config.change([(key, value)], format!("Saved {}", about.to_lowercase()));
                Ok(())
            };
            let placeholder = match kind {
                ValueKind::Count => "no limit",
                ValueKind::Folder => "the default",
                _ => "none",
            };
            view! { <InlineField text placeholder title=kind.help() save /> }.into_any()
        }
        Control::Toggle { key, on } => switch(on, move || {
            config.change([(key, json!(!on))], format!("{label}: {}", if on { "off" } else { "on" }));
        })
        .into_any(),
        Control::Choice { key, options, chosen } => {
            let at = options.iter().position(|(value, _)| *value == chosen).unwrap_or(0);
            view! {
                <div class="segmented" role="group" aria-label=label.clone()>
                    {options.iter().enumerate().map(|(i, (value, name))| {
                        let (value, pressed) = (*value, i == at);
                        // `enter` picks the next choice.
                        let next = i == (at + 1) % options.len();
                        view! {
                            <button
                                class="segmented__option"
                                type="button"
                                aria-pressed=bool_attr(pressed)
                                data-change=next.then_some("")
                                on:click=move |_| {
                                    if !pressed {
                                        config.change([(key, json!(value))], format!("At the limit: {name}"));
                                    }
                                }
                            >
                                {*name}
                            </button>
                        }
                    }).collect_view()}
                </div>
            }
            .into_any()
        }
        Control::Schedule(schedule) => view! { <ScheduleEditor schedule /> }.into_any(),
        Control::Theme => view! { <ThemePicker theme=ui.theme /> }
            .into_any(),
        Control::Scanlines => switch(ui.scanlines.get_untracked(), move || ui.scanlines.update(|s| *s = !*s)).into_any(),
        Control::Clean => switch(ui.clean.get_untracked(), move || ui.clean.update(|c| *c = !*c)).into_any(),
        Control::Notifications => view! {
            {move || {
                let on = ui.notify.get();
                switch(on, move || notify::switch(!on, ui.notify, controls))
            }}
        }
        .into_any(),
        Control::Columns => view! {
            <Button small=true title="Choose columns (| in the list)" on_click=move || ui.dialog.set(Some(Dialog::Columns))>
                <span data-change="">"choose…"</span>
            </Button>
        }
        .into_any(),
        Control::Category { name, folder } => {
            let (save_name, remove_name) = (name.clone(), name.clone());
            let save = move |draft: String| {
                let draft = draft.trim().to_owned();
                if !tempest_api::settings::absolute(&draft) {
                    return Err("Type an absolute path on the engine's machine.".to_owned());
                }
                config.set_category(save_name.clone(), draft);
                Ok(())
            };
            view! {
                <InlineField text=folder placeholder="/path/to/folder" save />
                <button class="btn btn--ghost btn--small" type="button" data-remove="" title="Remove the category (x)"
                    on:click=move |_| config.remove_category(remove_name.clone())>
                    "remove"
                </button>
            }
            .into_any()
        }
        Control::Tracker(url) => list_entry(config, keys::TRACKERS, url, rows::tracker_problem).into_any(),
        Control::Relay(relay) => list_entry(config, keys::RELAYS, relay, rows::relay_problem).into_any(),
        Control::Tag(tag) => view! {
            <span class="row__value">{row.value.clone()}</span>
            <Button small=true title="Show its transfers" on_click=move || {
                ui.finding.update(|f| { f.text = format!("#{tag}"); f.status = None; });
                ui.area.set(Area::Transfers);
            }>
                <span data-change="">"show →"</span>
            </Button>
        }
        .into_any(),
    }
}

/// The trackers' or relays' list in the settings, as last known.
fn list_of(config: Config, key: &str) -> Vec<String> {
    config
        .info
        .with_untracked(|i| {
            i.as_ref().map(|i| match key {
                keys::TRACKERS => i.settings.trackers.clone(),
                _ => i.settings.relays.known.clone(),
            })
        })
        .unwrap_or_default()
}

/// An entry of the trackers or relays: a field that changes it, and a remove button.
fn list_entry(
    config: Config,
    key: &'static str,
    entry: String,
    problem: fn(&str) -> Option<&'static str>,
) -> impl IntoView {
    let (old, gone) = (entry.clone(), entry.clone());
    let save = move |draft: String| {
        let draft = draft.trim().to_owned();
        if let Some(why) = problem(&draft) {
            return Err(why.to_owned());
        }
        let list = rows::edited(&list_of(config, key), Some(&old), Some(&draft));
        config.change([(key, json!(list))], format!("Changed to {draft}"));
        Ok(())
    };
    view! {
        <InlineField text=entry save />
        <button class="btn btn--ghost btn--small" type="button" data-remove="" title="Remove it (x)"
            on:click=move |_| {
                let list = rows::edited(&list_of(config, key), Some(&gone), None);
                config.change([(key, json!(list))], format!("Removed {gone}"));
            }>
            "remove"
        </button>
    }
}

/// The field that adds a category, tracker or relay.
#[component]
fn AddField(adds: Adds) -> impl IntoView {
    let config = expect_context::<Config>();
    let first = RwSignal::new(String::new());
    let folder = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let input = NodeRef::<Input>::new();
    let (label, placeholder) = match adds {
        Adds::Category => ("Add a category", "name, such as films"),
        Adds::Tracker => ("Add a tracker", "https://tracker.example.org/announce"),
        Adds::Relay => ("Add a relay", "<key>@host:port"),
    };
    let add = move || {
        let text = first.get_untracked().trim().to_owned();
        let result = match adds {
            Adds::Category => {
                let path = folder.get_untracked().trim().to_owned();
                if text.is_empty() {
                    Err("Give the category a name.")
                } else if !tempest_api::settings::absolute(&path) {
                    Err("Type the folder its downloads go to, as an absolute path.")
                } else {
                    config.set_category(text, path);
                    Ok(())
                }
            }
            Adds::Tracker | Adds::Relay => {
                let (key, check): (&'static str, fn(&str) -> Option<&'static str>) = match adds {
                    Adds::Tracker => (keys::TRACKERS, rows::tracker_problem),
                    _ => (keys::RELAYS, rows::relay_problem),
                };
                match check(&text) {
                    Some(why) => Err(why),
                    None => {
                        let list = rows::edited(&list_of(config, key), None, Some(&text));
                        config.change([(key, json!(list))], format!("Added {text}"));
                        Ok(())
                    }
                }
            }
        };
        match result {
            Ok(()) => {
                first.set(String::new());
                folder.set(String::new());
                problem.set(None);
            }
            Err(why) => problem.set(Some(why.to_owned())),
        }
    };
    let field = move |value: RwSignal<String>,
                      placeholder: &'static str,
                      node: Option<NodeRef<Input>>| {
        let node = node.unwrap_or_default();
        view! {
            <input node_ref=node class="input" type="text" placeholder=placeholder spellcheck="false" autocomplete="off"
                data-add="" prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e)) />
        }
    };
    view! {
        <form class="add-field" aria-label=label on:submit=move |e| { e.prevent_default(); add(); }>
            <span class="add-field__label">{format!("+ {}", label.to_lowercase())}</span>
            {field(first, placeholder, Some(input))}
            {(adds == Adds::Category).then(|| view! {
                <input class="input" type="text" placeholder="/folder/for/its/downloads" spellcheck="false" autocomplete="off"
                    prop:value=move || folder.get() on:input=move |e| folder.set(event_target_value(&e)) />
            })}
            <button class="btn btn--small" type="submit">"add"</button>
            {move || problem.get().map(|p| view! { <span class="field__error add-field__error">{p}</span> })}
        </form>
    }
}

/// The alternative limits' schedule: days and two times, saved together.
#[component]
fn ScheduleEditor(schedule: Option<Schedule>) -> impl IntoView {
    let config = expect_context::<Config>();
    let days = RwSignal::new(std::array::from_fn::<bool, 7, _>(|i| {
        schedule
            .as_ref()
            .map_or(i < 5, |s| s.days.contains(&Weekday::ALL[i]))
    }));
    // A new schedule starts as working hours, as its days do.
    let time =
        |t: Option<String>, default: &str| RwSignal::new(t.unwrap_or_else(|| default.to_owned()));
    let from = time(schedule.as_ref().map(|s| s.from.to_string()), "09:00");
    let to = time(schedule.as_ref().map(|s| s.to.to_string()), "17:00");
    let problem = RwSignal::new(None::<String>);
    let save = move |clear: bool| {
        let (f, t) = match clear {
            true => (String::new(), String::new()),
            false => (from.get_untracked(), to.get_untracked()),
        };
        match Schedule::from_form(days.get_untracked(), &f, &t) {
            Ok(schedule) => {
                problem.set(None);
                let done = match &schedule {
                    Some(s) => format!("They apply {}", s.describe()),
                    None => "They have no schedule now".to_owned(),
                };
                config.change([(keys::ALT_SCHEDULE, json!(schedule))], done);
            }
            Err(error) => problem.set(Some(error)),
        }
    };
    let has = schedule.is_some();
    view! {
        <form class="schedule" on:submit=move |e| { e.prevent_default(); save(false); }>
            <div class="days" role="group" aria-label="Days">
                {Weekday::ALL.map(|day| {
                    let i = day as usize;
                    view! {
                        <button class="days__day" type="button" aria-pressed=move || bool_attr(days.with(|d| d[i]))
                            on:click=move |_| days.update(|d| d[i] = !d[i])>
                            {day.short()}
                        </button>
                    }
                })}
            </div>
            <input class="input input--time" type="text" placeholder="09:00" aria-label="From" data-change=""
                prop:value=move || from.get() on:input=move |e| from.set(event_target_value(&e)) />
            <span class="dim">"to"</span>
            <input class="input input--time" type="text" placeholder="17:00" aria-label="To"
                prop:value=move || to.get() on:input=move |e| to.set(event_target_value(&e)) />
            <button class="btn btn--small btn--primary" type="submit">"save"</button>
            {has.then(|| view! {
                <button class="btn btn--small btn--ghost" type="button" on:click=move |_| save(true)>"clear"</button>
            })}
            {move || problem.get().map(|p| view! { <span class="field__error inline__error">{p}</span> })}
        </form>
    }
}

/// The transfers waiting in the queue, in order, with buttons to move them.
#[component]
fn QueueList() -> impl IntoView {
    let store = expect_context::<Store>();
    let waiting = Memo::new(move |_| {
        store.model.with(|m| {
            m.transfers()
                .iter()
                .filter(|t| t.status == Status::Queued)
                .map(|t| t.id)
                .collect::<Vec<BundleId>>()
        })
    });
    view! {
        <div class="queue">
            <h3 class="queue__title">
                {move || match waiting.with(Vec::len) {
                    0 => "Nothing is waiting in the queue.".to_owned(),
                    n => format!("Waiting, in the order they start ({n})"),
                }}
            </h3>
            <ol class="queue__list">
                <For each=move || waiting.get() key=|id| *id let:id>
                    <QueueRow id />
                </For>
            </ol>
        </div>
    }
}

#[component]
fn QueueRow(id: BundleId) -> impl IntoView {
    let store = expect_context::<Store>();
    let controls = expect_context::<Controls>();
    let ui = expect_context::<Ui>();
    let Some(row) = store.row(id) else {
        return ().into_any();
    };
    let step = move |step: QueueStep| move || controls.queue(&row.get_untracked(), step);
    let name = move || row.with(|t: &TransferInfo| t.name.clone());
    view! {
        <li class="queue__row">
            <span class="queue__place dim">{move || row.with(|t| format!("#{}", t.queue_position + 1))}</span>
            <button class="linklike queue__name" type="button" title="Show in Transfers" on:click=move |_| ui.show(id)>{name}</button>
            <span class="queue__buttons">
                <Button small=true kind=ButtonKind::Ghost title="Top of the queue (<)" on_click=step(QueueStep::Top)>"top"</Button>
                <Button small=true kind=ButtonKind::Ghost title="Up the queue (K)" on_click=step(QueueStep::Up)>"↑"</Button>
                <Button small=true kind=ButtonKind::Ghost title="Down the queue (J)" on_click=step(QueueStep::Down)>"↓"</Button>
                <Button small=true kind=ButtonKind::Ghost title="Bottom of the queue (>)" on_click=step(QueueStep::Bottom)>"end"</Button>
            </span>
        </li>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dom_ids_are_stable_and_plain() {
        let id = dom_id("tracker:https://t.example.org/a b");
        assert_eq!(id, dom_id("tracker:https://t.example.org/a b"));
        assert_ne!(id, dom_id("tracker:https://t.example.org/a"));
        assert!(id.starts_with("row-") && id[4..].bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
