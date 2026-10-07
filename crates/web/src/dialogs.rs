//! The dialogs: adding a link (a quick modal, D-139), removing and renaming a transfer, and the
//! keys.

use leptos::html::Input;
use leptos::prelude::*;
use tempest_api::TransferInfo;
use tempest_protocol::BundleId;
use tempest_ui_kit::components::{Button, ButtonKind, Dialog as Modal, Tone};
use tempest_ui_kit::theme::bool_attr;

use crate::api;
use crate::controls::Controls;
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
        // The keys of where the keyboard was when it opened.
        Dialog::Help => Some(view! { <Help scope=ui.scope_now() /> }.into_any()),
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
            <label class="check">
                <input type="checkbox" prop:checked=move || stopped.get() on:change=move |e| stopped.set(event_target_checked(&e)) />
                "Add it stopped"
            </label>
        </Modal>
    }
}

/// The transfer `id`, as last known.
fn transfer(id: BundleId) -> Option<TransferInfo> {
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
