//! The Share area (D-139, D-196): a form for a file or folder on the daemon's machine, the
//! bundle being built, then its `tempest:` link to copy; and every share, with its link.

use std::time::Duration;

use leptos::html::Input;
use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::{Kind, TransferInfo};
use tempest_protocol::BundleId;
use tempest_ui_kit::components::*;
use tempest_ui_kit::format;
use tempest_ui_kit::theme::bool_attr;

use crate::api;
use crate::browse::Purpose;
use crate::controls::Controls;
use crate::edits::ShareForm;
use crate::list::Move;
use crate::picker::Browse;
use crate::present;
use crate::signin::Auth;
use crate::state::Ui;
use crate::store::Store;

/// The share form's path field, which `a` focuses.
#[derive(Clone, Copy)]
pub struct SharePath(pub NodeRef<Input>);

/// The IDs of every share, in queue order, tracked.
pub fn share_ids(store: Store) -> Vec<BundleId> {
    store.model.with(|m| {
        m.transfers()
            .iter()
            .filter(|t| t.kind == Kind::Share)
            .map(|t| t.id)
            .collect()
    })
}

/// Moves the Share area's cursor as `to` says.
pub fn go(ui: Ui, ids: &[BundleId], to: Move) {
    let at = ui
        .share
        .get_untracked()
        .and_then(|id| ids.iter().position(|i| *i == id));
    if let Some(next) = to.from(at, ids.len()) {
        ui.share.set(Some(ids[next]));
    }
}

/// What the form is doing.
#[derive(Clone, Debug, PartialEq)]
enum Progress {
    /// Waiting for a path.
    Idle,
    /// The engine is building the bundle of this path; the page has waited this many seconds.
    Building(String, u64),
    /// The bundle is shared.
    Done(Box<TransferInfo>),
}

/// The Share area.
#[component]
pub fn Share() -> impl IntoView {
    view! {
        <div class="area area--wide share-area">
            <NewShare />
            <Shares />
        </div>
    }
}

#[component]
fn NewShare() -> impl IntoView {
    let controls = expect_context::<Controls>();
    let ui = expect_context::<Ui>();
    let SharePath(path_ref) = expect_context::<SharePath>();
    let path = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let comment = RwSignal::new(String::new());
    let tags = RwSignal::new(String::new());
    let problem = RwSignal::new(None::<String>);
    let building = RwSignal::new(Progress::Idle);
    let busy = Signal::derive(move || matches!(building.get(), Progress::Building(..)));

    // Counts the seconds while the engine builds the bundle.
    if let Ok(timer) = set_interval_with_handle(
        move || {
            building.maybe_update(|b| match b {
                Progress::Building(_, seconds) => {
                    *seconds += 1;
                    true
                }
                _ => false,
            });
        },
        Duration::from_secs(1),
    ) {
        on_cleanup(move || timer.clear());
    }

    let share = move || {
        if busy.get_untracked() {
            return;
        }
        let form = ShareForm {
            path: path.get_untracked(),
            name: name.get_untracked(),
            comment: comment.get_untracked(),
            tags: tags.get_untracked(),
        };
        let request = match form.request() {
            Ok(request) => request,
            Err(error) => {
                problem.set(Some(error));
                return;
            }
        };
        problem.set(None);
        building.set(Progress::Building(form.path.trim().to_owned(), 0));
        spawn_local(async move {
            match api::share(request).await {
                Ok(shared) => {
                    for field in [path, name, comment, tags] {
                        field.set(String::new());
                    }
                    ui.share.set(Some(shared.id));
                    controls.say(Tone::Up, format!("Sharing {}", shared.name));
                    building.set(Progress::Done(Box::new(shared)));
                }
                Err(error) => {
                    problem.set(Some(error.to_string()));
                    building.set(Progress::Idle);
                }
            }
        });
    };

    // A path picked from the engine's folders fills the field, and the Share button takes
    // the focus, so enter shares it as the rest of the form stands.
    let submit = NodeRef::<leptos::html::Button>::new();
    let chosen = move |picked: String| {
        path.set(picked);
        problem.set(None);
        request_animation_frame(move || {
            if let Some(button) = submit.get_untracked() {
                let _ = button.focus();
            }
        });
    };
    let remote = matches!(
        expect_context::<RwSignal<Auth>>().get_untracked(),
        Auth::SignedIn { remote: true }
    );
    let help = match remote {
        true => {
            "Absolute path on the daemon's machine, not this device. Files are shared in place, not copied."
        }
        false => "Absolute path on the daemon's machine. Files are shared in place, not copied.",
    };
    let text_field = move |label: &'static str,
                           value: RwSignal<String>,
                           placeholder: &'static str,
                           class: &'static str| {
        view! {
            <label class=format!("field {class}")>
                <span class="field__label">{label}</span>
                <input class="input" type="text" placeholder=placeholder spellcheck="false" autocomplete="off"
                    disabled=busy prop:value=move || value.get() on:input=move |e| value.set(event_target_value(&e)) />
            </label>
        }
    };

    view! {
        <Panel title="Share a file or folder">
            <form class="share-form" on:submit=move |e| { e.prevent_default(); share(); }>
                <div class="field share-form__path">
                    <label class="field__label" for="share-path">"Path"</label>
                    <div class="path-field">
                        <input
                            id="share-path"
                            node_ref=path_ref
                            class="input input--large"
                            type="text"
                            placeholder="/path/to/file-or-folder"
                            spellcheck="false"
                            autocomplete="off"
                            disabled=busy
                            aria-invalid=move || bool_attr(problem.with(Option::is_some))
                            prop:value=move || path.get()
                            on:input=move |e| path.set(event_target_value(&e))
                        />
                        <Browse purpose=Purpose::Share value=path disabled=busy on_choose=chosen />
                    </div>
                    <span class="field__help">{help}</span>
                </div>
                {text_field("Name", name, "Defaults to the file or folder name", "")}
                {text_field("Tags", tags, "music, live", "")}
                {text_field("Comment", comment, "Optional description", "share-form__wide")}
                <div class="share-form__actions">
                    {move || problem.get().map(|p| view! { <span class="field__error" role="alert">{p}</span> })}
                    <div class="toolbar__spacer"></div>
                    <button node_ref=submit class="btn btn--primary" type="submit" disabled=busy>"Share"</button>
                </div>
            </form>
            {move || match building.get() {
                Progress::Idle => ().into_any(),
                Progress::Building(path, seconds) => view! {
                    <div class="building" role="status">
                        <div class="building__bar"><span></span></div>
                        <p>
                            "Building the bundle of " <span class="building__path">{path}</span>
                            ": reading every file and computing checksums. "
                            <span class="dim">{format!("{seconds} s")}</span>
                        </p>
                    </div>
                }.into_any(),
                Progress::Done(shared) => view! { <Ready shared=*shared /> }.into_any(),
            }}
        </Panel>
    }
}

/// The share just made: its link to copy.
#[component]
fn Ready(shared: TransferInfo) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let ui = expect_context::<Ui>();
    let (id, link) = (shared.id, shared.link.clone());
    view! {
        <div class="ready">
            <p class="ready__title">
                <span class="tone--up">"● "</span>
                {format!("{} is shared", shared.name)}
                <span class="dim">{format!(" · {}", present::size(shared.size))}</span>
            </p>
            <div class="linkbox">
                <code class="linkbox__link">{shared.link.clone()}</code>
                <Button kind=ButtonKind::Primary small=true on_click=move || controls.copy(link.clone())>"Copy link"</Button>
            </div>
            <p class="dim note">"Anyone with this link can download it while the daemon is online and sharing."</p>
            <Button small=true kind=ButtonKind::Ghost on_click=move || ui.show(id)>"Show in Transfers →"</Button>
        </div>
    }
}

#[component]
fn Shares() -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let ids = Memo::new(move |_| share_ids(store));
    let cursor = Selector::new(move || ui.share.get());
    view! {
        <Panel title=Signal::derive(move || format!("Shares ({})", ids.with(Vec::len)))>
            {move || ids.with(Vec::is_empty).then(|| view! {
                <Empty title="No shares yet" text="Enter a path above or use Browse." />
            })}
                <div class="table-scroll" hidden=move || ids.with(Vec::is_empty)>
                    <table class="table shares" aria-label="Shares">
                        <thead>
                            <tr>
                                <th>"Name"</th>
                                <th class="num col-hide-narrow">"Size"</th>
                                <th>"Status"</th>
                                <th class="num col-hide-narrow" title="Upload rate">"↑"</th>
                                <th class="num col-hide-narrow">"Uploaded"</th>
                                <th class="num col-hide-narrow">"Peers"</th>
                                <th>"Link"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <For each=move || ids.get() key=|id| *id let:id>
                                <ShareRow id cursor=cursor.clone() />
                            </For>
                        </tbody>
                    </table>
                </div>
        </Panel>
    }
}

#[component]
fn ShareRow(id: BundleId, cursor: Selector<Option<BundleId>>) -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let controls = expect_context::<Controls>();
    let Some(row) = store.row(id) else {
        return ().into_any();
    };
    let text = move |f: fn(&TransferInfo) -> String| move || row.with(f);
    let tone = Signal::derive(move || row.with(|t| present::tone(&t.status)));
    view! {
        <tr
            id=format!("share-{id}")
            aria-selected=move || bool_attr(cursor.selected(&Some(id)))
            on:click=move |_| ui.share.set(Some(id))
            on:dblclick=move |_| ui.show(id)
        >
            <td class="table__clip" title=text(|t| t.name.clone())>
                <button class="linklike" type="button" title="Show in Transfers" on:click=move |_| ui.show(id)>
                    {text(|t| t.name.clone())}
                </button>
            </td>
            <td class="num col-hide-narrow">{text(|t| present::size(t.size))}</td>
            <td><Badge tone label=Signal::derive(move || row.with(|t| present::label(&t.status).to_owned())) /></td>
            <td class="num col-hide-narrow">{text(|t| present::rate(t.upload_rate))}</td>
            <td class="num col-hide-narrow">{text(|t| format::bytes(t.uploaded))}</td>
            <td class="num col-hide-narrow">{text(present::peers)}</td>
            <td class="shares__link">
                <Button small=true title="Copy the link (y)" on_click=move || controls.copy(row.with_untracked(|t| t.link.clone()))>
                    "Copy link"
                </Button>
            </td>
        </tr>
    }
    .into_any()
}
