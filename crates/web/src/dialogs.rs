//! The dialogs: adding a link (a quick modal, D-139); a transfer's name, tags, category,
//! folder and own limits; removing it; the list's columns and saved views (D-197); and the
//! keys.

use leptos::html::Input;
use leptos::prelude::*;
use tempest_api::TransferInfo;
use tempest_api::columns::Column;
use tempest_api::finding::Finding;
use tempest_api::network;
use tempest_api::prefs;
use tempest_api::settings::parse_tags;
use tempest_api::ui::SavedView;
use tempest_protocol::BundleId;
use tempest_ui_kit::components::{
    Button, ButtonKind, Checkbox, Chip, Dialog as Modal, Radio, SelectField, Tone,
};
use tempest_ui_kit::theme::bool_attr;

use crate::api;
use crate::config::Config;
use crate::controls::Controls;
use crate::edits::{self, LimitsForm};
use crate::keys::{self, Scope};
use crate::state::{Dialog, Ui};
use crate::store::Store;

/// Whichever dialog `Ui::dialog` names.
#[component]
pub fn Dialogs() -> impl IntoView {
    let ui = expect_context::<Ui>();
    move || match ui.dialog.get()? {
        Dialog::Add => Some(view! { <AddLink /> }.into_any()),
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
    (!link.starts_with("tempest:")).then_some("Links start with tempest:")
}

#[component]
fn AddLink() -> impl IntoView {
    let ui = expect_context::<Ui>();
    let controls = expect_context::<Controls>();
    let link = RwSignal::new(String::new());
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
    let add = move || {
        tried.set(true);
        if link.with(|l| link_problem(l)).is_some() {
            return;
        }
        let (text, stopped) = (
            link.get_untracked().trim().to_owned(),
            stopped.get_untracked(),
        );
        ui.dialog.set(None);
        leptos::task::spawn_local(async move {
            match api::add_link(text, stopped).await {
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
            <form on:submit=move |e| { e.prevent_default(); add(); }>
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
                    <span class="field__help">"The download starts through the queue, into the default folder."</span>
                </label>
            </form>
            <Checkbox checked=stopped on_change=move |on| stopped.set(on)>"Add it stopped"</Checkbox>
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
    let remove = move |delete_files: bool| {
        let name = transfer(id).map(|t| t.name).unwrap_or_default();
        close();
        let done = match delete_files {
            true => format!("Removed {name} and deleted its files"),
            false => format!("Removed {name}; its files stay"),
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
                <Button kind=ButtonKind::Danger on_click=move || remove(true)>"Delete files too"</Button>
            }
        >
            <p>"It stops and leaves the list. Its files stay on disk unless you delete them too."</p>
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
        let text = name.get_untracked().trim().to_owned();
        close();
        let (value, done) = match text.is_empty() {
            true => (None, "Gave it the bundle's own name".to_owned()),
            false => (Some(text.clone()), format!("Renamed to {text}")),
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
                    <span class="field__help">"Only this list shows it. Leave it empty for the bundle's own name."</span>
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
        let tags = parse_tags(&text.get_untracked());
        close();
        let done = match tags.is_empty() {
            true => "Removed its tags".to_owned(),
            false => format!(
                "Tagged it {}",
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
                <Field label="Tags" value=text placeholder="music, live" help="Separated by commas. #tag in the filter shows them." focus=true />
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
        let category = Some(chosen.get_untracked()).filter(|c| !c.is_empty());
        close();
        let done = match &category {
            Some(category) => format!("It's in {category} now"),
            None => "It has no category now".to_owned(),
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
                    "none"
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
            <p class="dim note">"Categories and their folders are in Settings. A new download in a category goes to its folder."</p>
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
    let save = move || {
        let text = folder.get_untracked().trim().to_owned();
        if let Some(why) = edits::folder_problem(&text) {
            problem.set(Some(why.to_owned()));
            return;
        }
        close();
        controls.run(
            api::move_to(id, text.clone()),
            format!("Moving its files to {text}"),
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
                <Field label="Folder" value=folder placeholder="/path/to/folder" help="An absolute path on the engine's machine. The transfer pauses while its files move." focus=true />
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
    ];
    let action = RwSignal::new(form.action);
    let problem = RwSignal::new(None::<String>);
    // What the engine's settings say, for the placeholders.
    let engine = config.info.get_untracked().map(|i| i.settings.sharing);
    let engine_ratio = engine
        .as_ref()
        .and_then(|s| s.ratio)
        .map_or("the engine's: none".to_owned(), |r| {
            format!("the engine's: {r}")
        });
    let engine_time = engine
        .and_then(|s| s.time)
        .map_or("the engine's: none".to_owned(), |t| {
            format!("the engine's: {t}")
        });
    let save = move || {
        let form = LimitsForm {
            download: fields[0].get_untracked(),
            upload: fields[1].get_untracked(),
            ratio: fields[2].get_untracked(),
            time: fields[3].get_untracked(),
            action: action.get_untracked(),
        };
        let current = self::transfer(id).map(|t| t.settings).unwrap_or_default();
        match form.apply(&current) {
            Ok(settings) => {
                close();
                controls.run(
                    api::set_transfer_settings(id, settings),
                    "Saved its limits".to_owned(),
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
                {input(0, "Download", "no limit of its own".to_owned())}
                {input(1, "Upload", "no limit of its own".to_owned())}
                <h3 class="form-grid__title">"Sharing"</h3>
                {input(2, "Ratio", engine_ratio.clone())}
                {input(3, "Time", engine_time.clone())}
                <div class="form-grid__wide">
                    <SelectField
                        label="At the limit"
                        options=edits::ACTIONS.map(|(value, name)| (value.to_owned(), name.to_owned())).to_vec()
                        value=action
                    />
                </div>
                <button type="submit" hidden=true></button>
            </form>
            <p class="dim note">"Speeds such as 500K or 2.5M, within the engine's limits; times such as 12h. Empty keeps the engine's setting; none means no limit of its own."</p>
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
            <p class="dim note">"Name, progress and status always show. Narrow windows drop columns, least important first. The TUI shows the same ones."</p>
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
                        }>"delete"</Button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Modal open=true on_close=close title="Saved views">
            {move || ui.views.with(Vec::is_empty).then(|| view! {
                <p class="dim">"No views yet. A view is a filter, a status and an order, saved under a name and shared with the TUI."</p>
            })}
            <ul class="views">{rows}</ul>
            <form class="views__save" on:submit=move |e| { e.prevent_default(); save(); }>
                <Field label="Save the list as it is" value=name placeholder="a name, such as music" focus=true />
                <Button kind=ButtonKind::Primary on_click=save>"Save view"</Button>
            </form>
            <p class="dim note">{move || format!("Now: {}", current())}</p>
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
            title="Before you start"
            actions=move || view! {
                <Button kind=ButtonKind::Primary on_click=close>"Got it"</Button>
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
            Some("Links start with tempest:")
        );
        assert_eq!(link_problem(" tempest:abc "), None);
    }
}
