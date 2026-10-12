//! A transfer's detail (D-137): a drawer on wide windows and a sheet on narrow ones, with the
//! plain-language summary first, then files, peers and decisions; the chunk map and the rates
//! chart are one level further in.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use tempest_api::TransferInfo;
use tempest_api::events::DecisionInfo;
use tempest_api::explain::{Fix, explain};
use tempest_api::requests::QueueStep;
use tempest_protocol::BundleId;
use tempest_ui_kit::chart::LineChart;
use tempest_ui_kit::components::*;
use tempest_ui_kit::{ChunkMap, format};

use crate::api;
use crate::controls::{Controls, folder_of, runs};
use crate::present;
use crate::request::RequestError;
use crate::signin::Auth;
use crate::state::{Area, Deeper, Dialog, Tab, Ui};
use crate::store::Store;

/// How many cells the chunk map asks for (D-193): 32 rows of 48.
const CELLS: usize = 1536;
/// The chunk map's cells per row on wide windows.
const COLUMNS: usize = 48;
/// How often an open view asks for fresh data (D-194).
const POLL: Duration = Duration::from_secs(1);
/// How many decision events the log shows, newest first.
const DECISIONS_SHOWN: usize = 300;

/// The detail drawer of the transfer `Ui::open` names.
#[component]
pub fn Detail() -> impl IntoView {
    let store = expect_context::<Store>();
    let ui = expect_context::<Ui>();
    let transfer = Memo::new(move |_| {
        let id = ui.open.get()?;
        store.model.with(|m| m.transfer(&id).cloned())
    });
    // A transfer removed while open closes its detail.
    Effect::new(move |_| {
        if ui.open.get().is_some() && store.loaded.get() && transfer.with(Option::is_none) {
            ui.open.set(None);
        }
    });
    let title = Signal::derive(move || {
        transfer.with(|t| t.as_ref().map(|t| t.name.clone()).unwrap_or_default())
    });
    view! {
        <Drawer open=Signal::derive(move || ui.open.get().is_some() && ui.area.get() == Area::Transfers) on_close=move || ui.open.set(None) title>
            {move || {
                let id = ui.open.get()?;
                Some(match ui.deeper.get() {
                    None => view! { <Body id transfer /> }.into_any(),
                    Some(deeper) => view! { <DeeperView id deeper /> }.into_any(),
                })
            }}
        </Drawer>
    }
}

#[component]
fn Body(id: BundleId, transfer: Memo<Option<TransferInfo>>) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let tabs = Tab::ALL.map(|t| (t, t.title())).to_vec();
    view! {
        <div class="detail">
            <Explanation transfer />
            <ControlsRow transfer />
            <Tabs label="Detail" tabs current=ui.tab />
            {move || match ui.tab.get() {
                Tab::Summary => view! { <Summary transfer /> }.into_any(),
                Tab::Files => view! { <Files id /> }.into_any(),
                Tab::Peers => view! { <Peers id /> }.into_any(),
                Tab::Decisions => view! { <Decisions id /> }.into_any(),
            }}
        </div>
    }
}

/// What the transfer is doing, in a sentence, with the fix it suggests and more behind a toggle.
#[component]
fn Explanation(transfer: Memo<Option<TransferInfo>>) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let explained = Memo::new(move |_| {
        transfer.with(|t| t.as_ref().map(|t| (explain(t), present::tone(&t.status))))
    });
    move || {
        let (explanation, tone) = explained.get()?;
        let fix = explanation.fix.map(|fix| {
            let (label, act): (&str, fn(Controls, &TransferInfo)) = match fix {
                Fix::Start => ("Start", Controls::start_stop),
                Fix::Retry => ("Try again", Controls::start_stop),
                Fix::ShareAgain => ("Share again", Controls::start_stop),
                Fix::ForceStart => ("Force start", Controls::force_start),
            };
            let run = move || {
                if let Some(t) = transfer.get_untracked() {
                    act(controls, &t);
                }
            };
            view! { <Button kind=ButtonKind::Primary small=true on_click=run>{label}</Button> }
        });
        let detail = explanation.detail.map(|detail| {
            view! {
                <details class="explain__more">
                    <summary>"More info"</summary>
                    <p>{detail}</p>
                </details>
            }
        });
        Some(view! {
            <div class=format!("explain {}", tone.class())>
                <div class="explain__body">
                    <p class="explain__text">{explanation.text}</p>
                    {detail}
                </div>
                {fix}
            </div>
        })
    }
}

/// The transfer's controls.
#[component]
fn ControlsRow(transfer: Memo<Option<TransferInfo>>) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let ui = expect_context::<Ui>();
    let with = move |act: fn(Controls, &TransferInfo)| {
        move || {
            if let Some(t) = transfer.get_untracked() {
                act(controls, &t);
            }
        }
    };
    let dialog = move |make: fn(BundleId) -> Dialog| {
        move || {
            if let Some(t) = transfer.get_untracked() {
                ui.dialog.set(Some(make(t.id)));
            }
        }
    };
    let queue = move |step: QueueStep| {
        move || {
            if let Some(t) = transfer.get_untracked() {
                controls.queue(&t, step);
            }
        }
    };
    let running = move || transfer.with(|t| t.as_ref().is_some_and(|t| runs(&t.status)));
    view! {
        <div class="detail__controls">
            <Button small=true title="Stop or start (space)" on_click=with(Controls::start_stop)>
                {move || if running() { "Stop" } else { "Start" }}
            </Button>
            <Button small=true title="Start now, ignoring the queue (F)" on_click=with(Controls::force_start)>
                "Force start"
            </Button>
            <Button small=true title="Recheck the files on disk (R)" on_click=with(Controls::recheck)>"Recheck"</Button>
            <Button small=true title="Rename (r)" on_click=dialog(Dialog::Rename)>"Rename"</Button>
            <Button small=true title="Edit tags (t)" on_click=dialog(Dialog::Tags)>"Tags…"</Button>
            <Button small=true title="Set the category (C)" on_click=dialog(Dialog::Category)>"Category…"</Button>
            <Button small=true title="Set its own speed and sharing limits (L)" on_click=dialog(Dialog::Limits)>"Limits…"</Button>
            <Button small=true title="Move its files to another folder (m)" on_click=dialog(Dialog::Move)>"Move…"</Button>
            <Button small=true kind=ButtonKind::Danger title="Remove (x)" on_click=dialog(Dialog::Remove)>
                "Remove…"
            </Button>
        </div>
        <div class="detail__queue">
            <span class="dim">"queue"</span>
            <Button small=true kind=ButtonKind::Ghost title="Move to the top of the queue (<)" on_click=queue(QueueStep::Top)>"Top"</Button>
            <Button small=true kind=ButtonKind::Ghost title="Move up in the queue (K)" on_click=queue(QueueStep::Up)>"↑ Up"</Button>
            <Button small=true kind=ButtonKind::Ghost title="Move down in the queue (J)" on_click=queue(QueueStep::Down)>"↓ Down"</Button>
            <Button small=true kind=ButtonKind::Ghost title="Move to the bottom of the queue (>)" on_click=queue(QueueStep::Bottom)>"Bottom"</Button>
        </div>
    }
}

#[component]
fn Summary(transfer: Memo<Option<TransferInfo>>) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let controls = expect_context::<Controls>();
    let field = move |f: fn(&TransferInfo) -> String| {
        Signal::derive(move || transfer.with(|t| t.as_ref().map(f).unwrap_or_default()))
    };
    view! {
        <Stats>
            <Stat label="progress" value=field(|t| format!("{} of {}", format::percent(t.progress), present::size(t.size)))/>
            <Stat label="↓ download" value=field(|t| present::rate(t.download_rate))/>
            <Stat label="↑ upload" value=field(|t| present::rate(t.upload_rate))/>
            <Stat label="time left" value=field(|t| present::eta(t.eta))/>
            <Stat label="peers" value=field(peers)/>
            <Stat label="ratio" value=field(|t| present::ratio(t.ratio))/>
            <Stat label="downloaded" value=field(|t| format::bytes(t.downloaded))/>
            <Stat label="uploaded" value=field(|t| format::bytes(t.uploaded))/>
            <Stat label="time sharing" value=field(|t| format::duration(t.seeding_time))/>
            <Stat label="added" value=field(|t| present::date(t.added)) title=field(|t| format::datetime(t.added))/>
            <Stat label="category" value=field(|t| t.category.clone().unwrap_or_else(|| "none".to_owned()))/>
            <Stat label="queue" value=field(|t| if t.forced { "forced".to_owned() } else { format!("#{}", t.queue_position + 1) })/>
        </Stats>
        <dl class="facts">
            <dt>"tags"</dt>
            <dd>
                {move || transfer.with(|t| {
                    let tags = t.as_ref().map(|t| t.tags.clone()).unwrap_or_default();
                    match tags.is_empty() {
                        true => view! { <span class="dim">"none"</span> }.into_any(),
                        false => tags.into_iter().map(|tag| view! { <Chip label=tag /> }).collect_view().into_any(),
                    }
                })}
            </dd>
            <dt>"folder"</dt>
            <dd class="facts__folder">
                <span class="facts__path">
                    {move || transfer.with(|t| t.as_ref().map(|t| folder_of(t).display().to_string()))}
                </span>
                <FolderActions transfer />
            </dd>
            <dt>"link"</dt>
            <dd class="facts__link">
                <span class="facts__url" title=move || transfer.with(|t| t.as_ref().map(|t| t.link.clone()))>
                    {move || transfer.with(|t| t.as_ref().map(|t| t.link.clone()))}
                </span>
                <span class="facts__actions">
                    <Button
                        small=true
                        kind=ButtonKind::Ghost
                        title="Copy the link, which anyone can open to download this (y)"
                        on_click=move || {
                            if let Some(t) = transfer.get_untracked() {
                                controls.copy_link(&t);
                            }
                        }
                    >
                        "Copy link"
                    </Button>
                </span>
            </dd>
        </dl>
        <div class="detail__deeper">
            <Button on_click=move || ui.deeper.set(Some(Deeper::ChunkMap))>
                "Chunk map " <kbd>"c"</kbd>
            </Button>
            <Button on_click=move || ui.deeper.set(Some(Deeper::Rates))>
                "Rates chart " <kbd>"g"</kbd>
            </Button>
        </div>
    }
}

/// Opens the transfer's folder in the daemon's file manager, which only a browser on the
/// daemon's own machine can ask for, and copies its path, which any browser can.
#[component]
fn FolderActions(transfer: Memo<Option<TransferInfo>>) -> impl IntoView {
    let controls = expect_context::<Controls>();
    let auth = expect_context::<RwSignal<Auth>>();
    let remote = move || matches!(auth.get(), Auth::SignedIn { remote: true });
    let saved = move || transfer.with(|t| t.as_ref().is_some_and(|t| t.content_dir.is_some()));
    let label = file_manager();
    let reason = move || {
        if remote() {
            "Can't open folders: this browser is connected to a remote daemon. Copy the path instead."
                .to_owned()
        } else if !saved() {
            "No files saved yet.".to_owned()
        } else {
            format!("{label}, on the daemon's machine (o)")
        }
    };
    let with = move |act: fn(Controls, &TransferInfo)| {
        move || {
            if let Some(t) = transfer.get_untracked() {
                act(controls, &t);
            }
        }
    };
    view! {
        <span class="facts__actions">
            <Button small=true title=Signal::derive(move || Some(reason())) disabled=Signal::derive(move || remote() || !saved()) on_click=with(Controls::reveal)>
                {label}
            </Button>
            <Button small=true kind=ButtonKind::Ghost title="Copy the folder's path" on_click=with(Controls::copy_path)>
                "Copy path"
            </Button>
        </span>
    }
}

/// What opening a folder is called on this machine's file manager: the browser is on the
/// daemon's machine whenever the button works.
fn file_manager() -> &'static str {
    let platform = window().navigator().platform().unwrap_or_default();
    if platform.starts_with("Mac") {
        "Show in Finder"
    } else if platform.starts_with("Win") {
        "Show in Explorer"
    } else {
        "Open folder"
    }
}

/// "3 of 12, 1 through a relay".
fn peers(t: &TransferInfo) -> String {
    let mut text = format!("{} of {}", t.connections, t.peers);
    if t.relayed > 0 {
        text.push_str(&format!(", {} relayed", t.relayed));
    }
    text
}

/// Fetches with `fetch` now and every [`POLL`] while the calling view shows, one request at a
/// time (D-194). A failure shows in the view. An answer like the last one changes nothing, so
/// the view keeps its rows, and their selection and scroll.
fn poll<T: Clone + PartialEq + Send + Sync + 'static>(
    fetch: impl Fn() -> Pin<Box<dyn Future<Output = Result<T, RequestError>>>> + 'static,
) -> RwSignal<Option<Result<T, String>>> {
    let data = RwSignal::new(None);
    let busy = StoredValue::new(false);
    let tick = move || {
        if busy.get_value() {
            return;
        }
        busy.set_value(true);
        let request = fetch();
        spawn_local(async move {
            let answer = request.await.map_err(|e| e.to_string());
            let _ = data.try_maybe_update(|shown| {
                let changed = shown.as_ref() != Some(&answer);
                if changed {
                    *shown = Some(answer);
                }
                (changed, ())
            });
            let _ = busy.try_set_value(false);
        });
    };
    tick();
    if let Ok(interval) = set_interval_with_handle(tick, POLL) {
        on_cleanup(move || interval.clear());
    }
    data
}

/// What a polled view shows: its data, a note while it loads, or why it failed.
fn polled<T: Clone + Send + Sync + 'static>(
    data: RwSignal<Option<Result<T, String>>>,
    show: impl Fn(T) -> AnyView + Send + Sync + 'static,
) -> impl IntoView {
    move || match data.get() {
        None => view! { <p class="dim">"Loading…"</p> }.into_any(),
        Some(Err(error)) => view! { <p class="tone--bad note">{error}</p> }.into_any(),
        Some(Ok(value)) => show(value),
    }
}

#[component]
fn Files(id: BundleId) -> impl IntoView {
    let data = poll(move || Box::pin(api::files(id)));
    polled(data, |files| {
        if files.is_empty() {
            return view! { <Empty title="No files yet" text="The manifest arrives from a peer." /> }.into_any();
        }
        view! {
            <div class="table-scroll">
                <table class="table">
                    <thead><tr><th>"File"</th><th class="num">"Size"</th><th>"Stored"</th></tr></thead>
                    <tbody>
                        {files.into_iter().map(|f| view! {
                            <tr>
                                <td class="table__clip" title=f.path.clone()>{f.path.clone()}</td>
                                <td class="num">{format::bytes(f.size)}</td>
                                <td class="cell-progress"><ProgressBar fraction=f.progress /></td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        }
        .into_any()
    })
}

#[component]
fn Peers(id: BundleId) -> impl IntoView {
    let data = poll(move || Box::pin(api::peers(id)));
    polled(data, |peers| {
        if peers.is_empty() {
            return view! { <Empty title="No peers connected" text="Connected peers appear here while the transfer runs." /> }.into_any();
        }
        view! {
            <div class="table-scroll">
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Peer"</th><th title="Direct, or through a relay">"Route"</th><th class="num" title="Download rate from this peer">"↓"</th><th class="num" title="Upload rate to this peer">"↑"</th>
                            <th class="num" title="How much of the bundle the peer has">"Progress"</th><th>"State"</th><th class="num" title="Round-trip time">"Latency"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {peers.into_iter().map(|p| view! {
                            <tr title=p.address.clone()>
                                <td>{format::short_id(&p.peer_id).to_owned()}</td>
                                <td class=if p.path == "relayed" { "tone--warn" } else { "" }>{p.path.clone()}</td>
                                <td class="num">{present::rate(p.download_rate)}</td>
                                <td class="num">{present::rate(p.upload_rate)}</td>
                                <td class="num">{p.progress.map_or_else(|| "?".to_owned(), format::percent)}</td>
                                <td>{present::peer_state(&p)}</td>
                                <td class="num">{p.rtt_ms.map_or_else(|| "–".to_owned(), |ms| format!("{ms} ms"))}</td>
                            </tr>
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        }
        .into_any()
    })
}

#[component]
fn Decisions(id: BundleId) -> impl IntoView {
    let store = expect_context::<Store>();
    // One memo for the log, so it redraws only when decisions arrive.
    let log = Memo::new(move |_| {
        store.model.with(|m| match m.decisions(&id) {
            None => (0, Vec::new()),
            Some(d) => {
                let events = d.events.iter().rev().take(DECISIONS_SHOWN).cloned();
                (d.missed, events.collect::<Vec<DecisionInfo>>())
            }
        })
    });
    move || {
        let (missed, events) = log.get();
        if events.is_empty() {
            return view! {
                <Empty title="No decisions yet" text="The session's decisions show here as it makes them: whom it asks for what, and why." />
            }
            .into_any();
        }
        view! {
            {(missed > 0).then(|| view! { <p class="dim note">{format!("{missed} earlier events weren't kept.")}</p> })}
            <ol class="decisions">
                {events.into_iter().map(|d| view! {
                    <li>
                        <span class="decisions__time dim">{present::clock(d.time)}</span>
                        <span class="decisions__kind">{d.kind}</span>
                        <span>{d.text}</span>
                    </li>
                }).collect_view()}
            </ol>
        }
        .into_any()
    }
}

/// The chunk map or the rates chart, one level into the detail.
#[component]
fn DeeperView(id: BundleId, deeper: Deeper) -> impl IntoView {
    let ui = expect_context::<Ui>();
    let (title, view) = match deeper {
        Deeper::ChunkMap => ("Chunk map", view! { <Chunks id /> }.into_any()),
        Deeper::Rates => ("Rates, last ten minutes", view! { <Rates id /> }.into_any()),
    };
    view! {
        <div class="detail">
            <div class="deeper__head">
                <Button kind=ButtonKind::Ghost small=true title="esc" on_click=move || ui.deeper.set(None)>
                    "‹ Back"
                </Button>
                <h3 class="deeper__title">{title}</h3>
            </div>
            {view}
        </div>
    }
}

#[component]
fn Chunks(id: BundleId) -> impl IntoView {
    let data = poll(move || Box::pin(api::chunks(id, CELLS)));
    let cells = Signal::derive(move || {
        data.with(|d| match d {
            Some(Ok(map)) => present::cells(&map.cells),
            _ => Vec::new(),
        })
    });
    let running = Signal::derive(move || data.with(|d| matches!(d, Some(Ok(map)) if map.running)));
    let error = move || data.with(|d| d.as_ref().and_then(|d| d.as_ref().err().cloned()));
    view! {
        {move || error().map(|e| view! { <p class="tone--bad note">{e}</p> })}
        <Show
            when=move || data.with(|d| matches!(d, Some(Ok(map)) if map.chunks > 0))
            fallback=move || view! {
                <Empty title="No chunks yet" text="The map appears once a peer sends the manifest." />
            }
        >
            <ChunkMap cells running columns=COLUMNS />
        </Show>
    }
}

#[component]
fn Rates(id: BundleId) -> impl IntoView {
    let store = expect_context::<Store>();
    let data = Signal::derive(move || {
        store.sampled.track();
        store.model.with(|m| {
            m.history(&id)
                .map(present::rates_chart)
                .unwrap_or_else(|| present::rates_chart(&Default::default()))
        })
    });
    let enough = move || {
        store.sampled.track();
        store
            .model
            .with(|m| m.history(&id).is_some_and(|h| h.samples().len() > 1))
    };
    view! {
        <Show
            when=enough
            fallback=|| view! { <Empty title="No data yet" text="Rates are recorded once a second while this page is open." /> }
        >
            <LineChart data label="Download and upload rates" height=280.0 />
        </Show>
    }
}
