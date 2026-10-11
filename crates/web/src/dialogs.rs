//! The dialogs: adding a link (a quick modal, D-139); a transfer's name, tags, category,
//! folder and own limits; removing it; the list's columns and saved views (D-197); and the
//! keys.

use leptos::html::Input;
use leptos::prelude::*;
use tempest_api::columns::Column;
use tempest_api::finding::Finding;
use tempest_api::network;
use tempest_api::prefs;
use tempest_api::settings::parse_tags;
use tempest_api::ui::SavedView;
use tempest_api::{Kind, TransferInfo};
use tempest_protocol::{BundleId, Link};
use tempest_ui_kit::components::{
    Button, ButtonKind, Checkbox, Chip, Dialog as Modal, Radio, SelectField, Tone,
};
use tempest_ui_kit::theme::bool_attr;

use crate::api;
use crate::browse::Purpose;
use crate::config::Config;
use crate::controls::Controls;
use crate::edits::{self, LimitsForm};
use crate::keys::{self, Scope};
use crate::picker::Browse;
use crate::state::{Dialog, Ui};
use crate::store::Store;

/// Whichever dialog `Ui::dialog` names.
#[component]
pub fn Dialogs() -> impl IntoView {
    let ui = expect_context::<Ui>();
    move || match ui.dialog.get()? {
        Dialog::Add => Some(view! { <AddLink /> }.into_any()),
        Dialog::Download => Some(view! { <Download /> }.into_any()),
        Dialog::Remove(id) => Some(view! { <Remove id /> }.into_any()),
        Dialog::Rename(id) => Some(view! { <Rename id /> }.into_any()),
        Dialog::Tags(id) => Some(view! { <Tags id /> }.into_any()),
        Dialog::Category(id) => Some(view! { <SetCategory id /> }.into_any()),
        Dialog::Move(id) => Some(view! { <Move id /> }.into_any()),
        Dialog::Limits(id) => Some(view! { <Limits id /> }.into_any()),
        Dialog::Columns => Some(view! { <Columns /> }.into_any()),
        Dialog::Views => Some(view! { <Views /> }.into_any()),
        // The keys of where the keyboard was when it opened.
        Dialog::Help => Some(view! { <Help scope=ui.scope_now() /> }.into_any()),
        Dialog::Network => Some(view! { <Network /> }.into_any()),
    }
}

fn close() {
    expect_context::<Ui>().dialog.set(None);
}

/// Focuses `input` once it is on the page.
fn focus_on_load(input: NodeRef<Input>) {
    input.on_load(|input| {
        request_animation_frame(move || {
            let _ = input.focus();
            input.select();
        });
    });
}

/// Whether `link` looks like a link, and if not, why.
pub fn link_problem(link: &str) -> Option<&'static str> {
    let link = link.trim();
    if link.is_empty() {
        return Some("Paste a tempest: link.");
    }
    (!link.starts_with("tempest:"))
        .then_some("That isn't a tempest: link. It must start with tempest:")
}

#[component]
fn AddLink() -> impl IntoView {
    let ui = expect_context::<Ui>();
    let controls = expect_context::<Controls>();
    let config = expect_context::<Config>();
    let link = RwSignal::new(String::new());
    let folder = RwSignal::new(String::new());
    let stopped = RwSignal::new(false);
    let tried = RwSignal::new(false);
    let input = NodeRef::<Input>::new();
    focus_on_load(input);
    let problem = move || {
        tried
            .get()
            .then(|| link.with(|l| link_problem(l)))
            .flatten()
    };
    let folder_problem = move || {
        tried
            .get()
            .then(|| folder.with(|f| edits::save_folder_problem(f)))
            .flatten()
    };
    // The folder a download goes to without one: the daemon's save folder.
    let default_folder = move || {
        config
            .info
            .with(|i| i.as_ref().and_then(|i| i.settings.save_folder.clone()))
            .unwrap_or_else(|| "the default folder".to_owned())
    };
    let add = move || {
        tried.set(true);
        if link.with(|l| link_problem(l)).is_some()
            || folder.with(|f| edits::save_folder_problem(f)).is_some()
        {
            return;
        }
        let (text, stopped) = (
            link.get_untracked().trim().to_owned(),
            stopped.get_untracked(),
        );
        let save_folder = Some(folder.get_untracked().trim().to_owned()).filter(|f| !f.is_empty());
        ui.dialog.set(None);
        leptos::task::spawn_local(async move {
            match api::add_link(text, save_folder, stopped).await {
                Ok(added) => {
                    controls.say(Tone::Down, format!("Added {}", added.name));
                    ui.cursor.set(Some(added.id));
                }
                Err(error) => controls.say(Tone::Bad, error.to_string()),
            }
        });
    };
    view! {
        <Modal
            open=true
            on_close=close
            title="Add a link"
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=add>"Add"</Button>
            }
        >
            <form class="form-stack" on:submit=move |e| { e.prevent_default(); add(); }>
                <label class="field">
                    <span class="field__label">"Link"</span>
                    <input
                        node_ref=input
                        class="input"
                        type="text"
                        placeholder="tempest:…"
                        spellcheck="false"
                        autocomplete="off"
                        aria-invalid=move || bool_attr(problem().is_some())
                        prop:value=move || link.get()
                        on:input=move |e| link.set(event_target_value(&e))
                    />
                    {move || problem().map(|p| view! { <span class="field__error">{p}</span> })}
                    <span class="field__help">"It starts when the queue has a free slot."</span>
                </label>
                <div class="field">
                    <label class="field__label" for="add-folder">"Save to"</label>
                    <div class="path-field">
                        <input
                            id="add-folder"
                            class="input"
                            type="text"
                            placeholder=default_folder
                            spellcheck="false"
                            autocomplete="off"
                            aria-invalid=move || bool_attr(folder_problem().is_some())
                            prop:value=move || folder.get()
                            on:input=move |e| folder.set(event_target_value(&e))
                        />
                        <Browse purpose=Purpose::Download value=folder on_choose=move |path| folder.set(path) />
                    </div>
                    {move || folder_problem().map(|p| view! { <span class="field__error">{p}</span> })}
                    <span class="field__help">"Folder on the daemon's machine. Leave empty to use the default."</span>
                </div>
                <button type="submit" hidden=true></button>
            </form>
            <Checkbox checked=stopped on_change=move |on| stopped.set(on)>"Add without starting"</Checkbox>
        </Modal>
    }
}

/// What to ask before downloading the offered link: where it comes from and what it names.
#[component]
fn Download() -> impl IntoView {
    let ui = expect_context::<Ui>();
    let controls = expect_context::<Controls>();
    let config = expect_context::<Config>();
    let text = ui.offer.get_untracked().unwrap_or_default();
    let cancel = move || {
        ui.offer.set(None);
        close();
    };
    let stored = StoredValue::new(text);
    let download = move || {
        ui.offer.set(None);
        close();
        let text = stored.get_value();
        leptos::task::spawn_local(async move {
            match api::add_link(text, None, false).await {
                Ok(added) => {
                    controls.say(Tone::Down, format!("Added {}", added.name));
                    ui.cursor.set(Some(added.id));
                }
                Err(error) => controls.say(Tone::Bad, error.to_string()),
            }
        });
    };
    let folder = move || {
        config
            .info
            .with(|i| i.as_ref().and_then(|i| i.settings.save_folder.clone()))
            .unwrap_or_else(|| "the default folder".to_owned())
    };
    let body = move || {
        match stored.get_value().parse::<Link>().ok() {
        Some(link) => {
            let name = link
                .name
                .clone()
                .unwrap_or_else(|| "Unnamed bundle".to_owned());
            let trackers = link.trackers.join(", ");
            view! {
                <dl class="facts">
                    <dt>"Name"</dt>
                    <dd>{name}<span class="facts__path">" (from the link, not checked)"</span></dd>
                    <dt>"Bundle ID"</dt>
                    <dd class="facts__path">{link.bundle.to_string()}</dd>
                    {(!trackers.is_empty()).then(|| view! { <dt>"Tracker"</dt><dd>{trackers}</dd> })}
                    <dt>"Save to"</dt>
                    <dd>{folder}</dd>
                </dl>
                <span class="field__help">"It starts when the queue has a free slot."</span>
            }
            .into_any()
        }
        None => view! { <span class="field__error">"This link is incomplete or invalid. Copy it again."</span> }
            .into_any(),
    }
    };
    let valid = stored.with_value(|t| t.parse::<Link>().is_ok());
    view! {
        <Modal
            open=true
            on_close=cancel
            title="Download this bundle?"
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=cancel>"Cancel"</Button>
                {valid.then(|| view! { <Button kind=ButtonKind::Primary on_click=download>"Download"</Button> })}
            }
        >
            <div class="form-stack">{body}</div>
        </Modal>
    }
}

/// The transfer `id`, as last known.
pub(crate) fn transfer(id: BundleId) -> Option<TransferInfo> {
    expect_context::<Store>()
        .model
        .with_untracked(|m| m.transfer(&id).cloned())
}

#[component]
fn Remove(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let name = transfer(id).map(|t| t.name).unwrap_or_default();
    // A share's files are the user's originals, not copies Tempest made.
    let share = transfer(id).is_some_and(|t| t.kind == Kind::Share);
    let remove = move |delete_files: bool| {
        let name = transfer(id).map(|t| t.name).unwrap_or_default();
        close();
        let done = match (delete_files, share) {
            (true, true) => format!("Removed {name} and deleted its original files"),
            (true, false) => format!("Removed {name} and deleted its files"),
            (false, _) => format!("Removed {name}. Files kept."),
        };
        controls.run(api::remove(id, delete_files), done);
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Remove {name}?")
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button on_click=move || remove(false)>"Remove, keep files"</Button>
                <Button kind=ButtonKind::Danger on_click=move || remove(true)>
                    {if share { "Remove and delete originals" } else { "Remove and delete files" }}
                </Button>
            }
        >
            {match share {
                true => view! {
                    <p>
                        "Stops sharing and removes it from the list. The files are your originals; "
                        "deleting them can't be undone."
                    </p>
                }.into_any(),
                false => view! {
                    <p>"Stops the transfer and removes it from the list. Downloaded files stay on disk unless you delete them."</p>
                }.into_any(),
            }}
        </Modal>
    }
}

#[component]
fn Rename(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let current = transfer(id).map(|t| t.name).unwrap_or_default();
    let name = RwSignal::new(current.clone());
    let input = NodeRef::<Input>::new();
    focus_on_load(input);
    let save = move || {
        let old = transfer(id).map(|t| t.name).unwrap_or_default();
        let text = name.get_untracked().trim().to_owned();
        close();
        let (value, done) = match text.is_empty() {
            true => (None, format!("Restored the original name of {old}")),
            false => (Some(text.clone()), format!("Renamed {old} to {text}")),
        };
        controls.run(api::rename(id, value), done);
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Rename {current}")
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Rename"</Button>
            }
        >
            <form on:submit=move |e| { e.prevent_default(); save(); }>
                <label class="field">
                    <span class="field__label">"Name"</span>
                    <input
                        node_ref=input
                        class="input"
                        type="text"
                        spellcheck="false"
                        autocomplete="off"
                        prop:value=move || name.get()
                        on:input=move |e| name.set(event_target_value(&e))
                    />
                    <span class="field__help">"Changes the name shown here, not the files. Leave empty to restore the original name."</span>
                </label>
            </form>
        </Modal>
    }
}

/// A labeled text input that takes the dialog's focus when `focus` is set.
#[component]
fn Field(
    label: &'static str,
    value: RwSignal<String>,
    #[prop(optional)] placeholder: &'static str,
    #[prop(optional)] help: &'static str,
    #[prop(optional)] focus: bool,
) -> impl IntoView {
    let input = NodeRef::<Input>::new();
    if focus {
        focus_on_load(input);
    }
    view! {
        <label class="field">
            <span class="field__label">{label}</span>
            <input
                node_ref=input
                class="input"
                type="text"
                placeholder=placeholder
                spellcheck="false"
                autocomplete="off"
                prop:value=move || value.get()
                on:input=move |e| value.set(event_target_value(&e))
            />
            {(!help.is_empty()).then(|| view! { <span class="field__help">{help}</span> })}
        </label>
    }
}

/// A dialog's form error, when there is one.
fn problem_line(problem: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        problem
            .get()
            .map(|p| view! { <p class="field__error" role="alert">{p}</p> })
    }
}

#[component]
fn Tags(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let transfer = transfer(id);
    let name = transfer
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let current: Vec<String> = transfer
        .map(|t| t.tags.into_iter().collect())
        .unwrap_or_default();
    let text = RwSignal::new(current.join(", "));
    let save = move || {
        let tagged = self::transfer(id).map(|t| t.name).unwrap_or_default();
        let tags = parse_tags(&text.get_untracked());
        close();
        let done = match tags.is_empty() {
            true => format!("Removed tags from {tagged}"),
            false => format!(
                "Tagged {tagged}: {}",
                tags.iter()
                    .map(|t| format!("#{t}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        };
        controls.run(api::set_tags(id, tags), done);
    };
    let preview = move || {
        parse_tags(&text.get())
            .into_iter()
            .map(|tag| view! { <Chip label=tag /> })
            .collect_view()
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Tags of {name}")
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Save tags"</Button>
            }
        >
            <form on:submit=move |e| { e.prevent_default(); save(); }>
                <Field label="Tags" value=text placeholder="music, live" help="Separate tags with commas. Filter by #tag in the list." focus=true />
            </form>
            <div class="chips">{preview}</div>
        </Modal>
    }
}

#[component]
fn SetCategory(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let config = expect_context::<Config>();
    let transfer = transfer(id);
    let name = transfer
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let chosen = RwSignal::new(transfer.and_then(|t| t.category).unwrap_or_default());
    let save = move || {
        let categorized = self::transfer(id).map(|t| t.name).unwrap_or_default();
        let category = Some(chosen.get_untracked()).filter(|c| !c.is_empty());
        close();
        let done = match &category {
            Some(category) => format!("Category of {categorized}: {category}"),
            None => format!("Removed the category of {categorized}"),
        };
        controls.run(api::set_category(id, category), done);
    };
    let options = move || {
        let mut names = config.category_names();
        let current = chosen.get_untracked();
        if !current.is_empty() && !names.contains(&current) {
            names.push(current);
        }
        names
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Category of {name}")
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Save"</Button>
            }
        >
            <div class="choices" role="radiogroup" aria-label="Category">
                <Radio name="category" checked=Signal::derive(move || chosen.with(String::is_empty)) on_select=move || chosen.set(String::new())>
                    "None"
                </Radio>
                {move || options().into_iter().map(|option| {
                    let (o, value) = (option.clone(), option.clone());
                    view! {
                        <Radio name="category" checked=Signal::derive(move || chosen.with(|c| *c == o)) on_select=move || chosen.set(value.clone())>
                            {option}
                        </Radio>
                    }
                }).collect_view()}
            </div>
            <p class="dim note">"New downloads in a category are saved to its folder. Edit categories in Settings."</p>
        </Modal>
    }
}

#[component]
fn Move(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let transfer = transfer(id);
    let name = transfer
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_default();
    let folder = RwSignal::new(
        transfer
            .map(|t| t.save_folder.display().to_string())
            .unwrap_or_default(),
    );
    let problem = RwSignal::new(None::<String>);
    let input = NodeRef::<Input>::new();
    focus_on_load(input);
    let save = move || {
        let moved = self::transfer(id).map(|t| t.name).unwrap_or_default();
        let text = folder.get_untracked().trim().to_owned();
        if let Some(why) = edits::folder_problem(&text) {
            problem.set(Some(why.to_owned()));
            return;
        }
        close();
        controls.run(
            api::move_to(id, text.clone()),
            format!("Moving {moved} to {text}"),
        );
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Move {name}")
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Move files"</Button>
            }
        >
            <form on:submit=move |e| { e.prevent_default(); save(); }>
                <div class="field">
                    <label class="field__label" for="move-folder">"Folder"</label>
                    <div class="path-field">
                        <input
                            id="move-folder"
                            node_ref=input
                            class="input"
                            type="text"
                            placeholder="/path/to/folder"
                            spellcheck="false"
                            autocomplete="off"
                            prop:value=move || folder.get()
                            on:input=move |e| folder.set(event_target_value(&e))
                        />
                        <Browse purpose=Purpose::Move value=folder on_choose=move |path| { folder.set(path); problem.set(None); } />
                    </div>
                    <span class="field__help">"Absolute path on the daemon's machine. The transfer pauses while its files move."</span>
                </div>
            </form>
            {problem_line(problem)}
        </Modal>
    }
}

#[component]
fn Limits(id: BundleId) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let config = expect_context::<Config>();
    let Some(transfer) = transfer(id) else {
        return ().into_any();
    };
    let own = transfer.settings.clone();
    let form = LimitsForm::of(&own);
    let fields = [
        RwSignal::new(form.download),
        RwSignal::new(form.upload),
        RwSignal::new(form.ratio),
        RwSignal::new(form.time),
        RwSignal::new(form.idle_timeout),
    ];
    // A share has no limits unless it sets its own (D-224).
    let share = transfer.kind == Kind::Share;
    let action = RwSignal::new(form.action);
    let problem = RwSignal::new(None::<String>);
    // What the Settings say, for the placeholders.
    let settings = config.info.get_untracked().map(|i| i.settings.sharing);
    let settings_says = |value: Option<String>| match (share, value) {
        (true, _) => "no limit".to_owned(),
        (false, Some(value)) => format!("Settings: {value}"),
        (false, None) => "Settings: no limit".to_owned(),
    };
    let settings_ratio = settings_says(
        settings
            .as_ref()
            .and_then(|s| s.ratio)
            .map(|r| r.to_string()),
    );
    let settings_time = settings_says(settings.as_ref().and_then(|s| s.time.clone()));
    let settings_idle = settings_says(settings.as_ref().and_then(|s| s.idle_timeout.clone()));
    let save = move || {
        let name = self::transfer(id).map(|t| t.name).unwrap_or_default();
        let form = LimitsForm {
            download: fields[0].get_untracked(),
            upload: fields[1].get_untracked(),
            ratio: fields[2].get_untracked(),
            time: fields[3].get_untracked(),
            idle_timeout: fields[4].get_untracked(),
            action: action.get_untracked(),
        };
        let current = self::transfer(id).map(|t| t.settings).unwrap_or_default();
        match form.apply(&current) {
            Ok(settings) => {
                close();
                controls.run(
                    api::set_transfer_settings(id, settings),
                    format!("Saved limits for {name}"),
                );
            }
            Err(error) => problem.set(Some(error)),
        }
    };
    let first = NodeRef::<Input>::new();
    focus_on_load(first);
    let input = move |i: usize, label: &'static str, placeholder: String| {
        let value = fields[i];
        let node = if i == 0 { first } else { NodeRef::new() };
        view! {
            <label class="field">
                <span class="field__label">{label}</span>
                <input node_ref=node class="input" type="text" placeholder=placeholder spellcheck="false" autocomplete="off"
                    prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e)) />
            </label>
        }
    };
    view! {
        <Modal
            open=true
            on_close=close
            title=format!("Limits of {}", transfer.name)
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Save limits"</Button>
            }
        >
            <form class="form-grid" on:submit=move |e| { e.prevent_default(); save(); }>
                <h3 class="form-grid__title">"Speed"</h3>
                {input(0, "Download", "no limit".to_owned())}
                {input(1, "Upload", "no limit".to_owned())}
                <h3 class="form-grid__title">"Sharing"</h3>
                {input(2, "Ratio", settings_ratio.clone())}
                {input(3, "Time", settings_time.clone())}
                {input(4, "Idle timeout", settings_idle.clone())}
                <div class="form-grid__wide">
                    <SelectField
                        label="When a limit is reached"
                        options=edits::ACTIONS.map(|(value, name)| (value.to_owned(), name.to_owned())).to_vec()
                        value=action
                    />
                </div>
                <button type="submit" hidden=true></button>
            </form>
            <p class="dim note">"Speeds: 500K, 2.5M (bytes per second). Times: 30m, 12h, 7d. Leave a field empty to use the Settings value (shares have none). Type none for no limit."</p>
            {problem_line(problem)}
        </Modal>
    }
    .into_any()
}

#[component]
fn Columns() -> impl IntoView {
    let ui = expect_context::<Ui>();
    let config = expect_context::<Config>();
    let chosen = RwSignal::new(ui.columns.get_untracked());
    let save = move || {
        let columns = chosen.get_untracked();
        close();
        ui.columns.set(columns.clone());
        config.change(
            [(prefs::COLUMNS, prefs::columns_value(&columns))],
            "Saved the columns".to_owned(),
        );
    };
    let reset = move || chosen.set(Column::DEFAULT.to_vec());
    view! {
        <Modal
            open=true
            on_close=close
            title="Columns"
            actions=move || view! {
                <Button kind=ButtonKind::Ghost on_click=reset>"Defaults"</Button>
                <Button kind=ButtonKind::Ghost on_click=close>"Cancel"</Button>
                <Button kind=ButtonKind::Primary on_click=save>"Save"</Button>
            }
        >
            <p class="dim note">"Name, progress and status always show. Narrow windows hide the others, least important first."</p>
            <div class="choices choices--columns">
                {Column::OPTIONAL.map(|column| {
                    let on = Signal::derive(move || chosen.with(|c| c.contains(&column)));
                    let flip = move |_| chosen.update(|c| match c.iter().position(|x| *x == column) {
                        Some(i) => { c.remove(i); }
                        None => c.push(column),
                    });
                    view! { <Checkbox checked=on on_change=flip>{column.title()}</Checkbox> }
                })}
            </div>
        </Modal>
    }
}

/// A saved view in words: "#music, downloading, by size".
fn describe(view: &SavedView) -> String {
    let finding = prefs::finding(view);
    let mut parts = Vec::new();
    if !finding.text.is_empty() {
        parts.push(format!("“{}”", finding.text));
    }
    if let Some(status) = finding.status {
        parts.push(status.title().to_lowercase());
    }
    parts.push(finding.sort.describe());
    parts.join(" · ")
}

#[component]
fn Views() -> impl IntoView {
    let ui = expect_context::<Ui>();
    let config = expect_context::<Config>();
    let name = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let save_views = move |views: Vec<SavedView>, done: String| {
        ui.views.set(views.clone());
        config.change([(prefs::VIEWS, prefs::views_value(&views))], done);
    };
    let save = move || {
        let text = name.get_untracked();
        if let Some(why) = edits::name_problem(&text) {
            problem.set(Some(why.to_owned()));
            return;
        }
        let view = ui.finding.with_untracked(|f| prefs::saved(&text, f));
        let done = format!("Saved the view {}", view.name);
        save_views(ui.views.with_untracked(|v| prefs::with_view(v, view)), done);
        name.set(String::new());
        problem.set(None);
    };
    let current = move || {
        ui.finding.with(|f| {
            let view = prefs::saved("", f);
            describe(&view)
        })
    };
    let rows = move || {
        ui.views
            .get()
            .into_iter()
            .map(|view| {
                let shown = describe(&view);
                let (apply, remove) = (view.clone(), view.name.clone());
                let active = ui.finding.with(|f| *f == prefs::finding(&view));
                view! {
                    <li class="views__row" aria-current=bool_attr(active)>
                        <button class="views__apply" type="button" on:click=move |_| {
                            ui.finding.set(prefs::finding(&apply));
                            close();
                        }>
                            <span class="views__name">{view.name.clone()}</span>
                            <span class="views__what dim">{shown}</span>
                        </button>
                        <Button small=true kind=ButtonKind::Ghost title="Delete this view" on_click=move || {
                            let views: Vec<SavedView> = ui.views.with_untracked(|v| v.iter().filter(|v| v.name != remove).cloned().collect());
                            save_views(views, format!("Deleted the view {remove}"));
                        }>"Delete"</Button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Modal open=true on_close=close title="Saved views">
            {move || ui.views.with(Vec::is_empty).then(|| view! {
                <p class="dim">"No saved views. A view stores the current filter, status and sort order under a name."</p>
            })}
            <ul class="views">{rows}</ul>
            <form class="views__save" on:submit=move |e| { e.prevent_default(); save(); }>
                <Field label="Save current filters as a view" value=name placeholder="Name, such as music" focus=true />
                <Button kind=ButtonKind::Primary on_click=save>"Save view"</Button>
            </form>
            <p class="dim note">{move || format!("Current: {}", current())}</p>
            {problem_line(problem)}
            <Button small=true kind=ButtonKind::Ghost on_click=move || { ui.finding.set(Finding::default()); close(); }>
                "Show everything"
            </Button>
        </Modal>
    }
}

/// The keys that work in `scope`, from the bindings table (D-191).
#[component]
fn Help(scope: Scope) -> impl IntoView {
    let rows = move || {
        keys::help(scope)
        .map(|binding| {
            let shown: Vec<_> = binding.keys.iter().map(|k| keys::shown(k)).collect();
            view! {
                <tr>
                    <td class="help__keys">{shown.into_iter().map(|k| view! { <kbd>{k}</kbd> }).collect_view()}</td>
                    <td>{binding.help}</td>
                </tr>
            }
        })
        .collect_view()
    };
    view! {
        <Modal open=true on_close=close title="Keys">
            <table class="table help">
                <tbody>
                    <tr>
                        <td class="help__keys"><kbd>"esc"</kbd></td>
                        <td>"Close what is open, or go back one level"</td>
                    </tr>
                    {rows}
                </tbody>
            </table>
        </Modal>
    }
}

/// What the network defaults share, said once on the first run (D-227).
#[component]
fn Network() -> impl IntoView {
    view! {
        <Modal
            open=true
            on_close=close
            title="Network features are on"
            actions=move || view! {
                <Button kind=ButtonKind::Primary on_click=close>"Close"</Button>
            }
        >
            <p>{network::INTRO}</p>
            <ul class="network">
                {network::FEATURES.map(|feature| view! {
                    <li><strong>{feature.label}"."</strong>" "{feature.says}</li>
                }).collect_view()}
            </ul>
        </Modal>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_must_look_like_links() {
        assert_eq!(link_problem("  "), Some("Paste a tempest: link."));
        assert_eq!(
            link_problem("magnet:?xt"),
            Some("That isn't a tempest: link. It must start with tempest:")
        );
        assert_eq!(link_problem(" tempest:abc "), None);
    }
}
