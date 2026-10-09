use std::collections::BTreeMap;
use std::path::PathBuf;

use tempest_api::{events::RemovedInfo, requests::AddRequest};

use super::*;
use crate::finding::{SortKey, StatusFilter};

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn press(app: &mut App, code: KeyCode) -> Option<Request> {
    app.key(key(code))
}

/// Presses each char of `keys` in turn, returning the last request.
pub(super) fn type_keys(app: &mut App, keys: &str) -> Option<Request> {
    keys.chars()
        .map(|c| press(app, KeyCode::Char(c)))
        .last()
        .flatten()
}

/// Categories named `names`, each saving to a folder of its name.
pub(super) fn categories(names: &[&str]) -> BTreeMap<String, PathBuf> {
    names
        .iter()
        .map(|name| (name.to_string(), PathBuf::from(format!("/srv/{name}"))))
        .collect()
}

/// The engine's default settings, as it sends them.
pub(super) fn settings() -> SettingsInfo {
    let mut settings = tempest_engine::EngineConfig::default();
    settings.ui.network_explained = true;
    let view = tempest_engine::SettingsView {
        settings,
        changed: BTreeMap::new(),
        restart_required: Vec::new(),
        alternative_limits_active: false,
    };
    serde_json::from_value(serde_json::to_value(view).unwrap()).unwrap()
}

/// Answers the settings fetch the app starts with.
pub(super) fn load(app: &mut App, view: SettingsInfo, names: &[&str]) {
    // Answers the fetch under way, or a new one.
    let _ = app.load_settings();
    let reply = Reply::Loaded(Box::new(view), categories(names));
    app.finished(&Request::Settings, Ok(reply));
}

pub(super) fn app_with(transfers: Vec<TransferInfo>) -> App {
    let mut app = App::new(Origin::Daemon, Theme::default());
    app.apply(ApiEvent::Snapshot(transfers));
    load(&mut app, settings(), &[]);
    app
}

/// Three transfers: "alpha" downloading and tagged, "beta" stopped, "gamma" sharing.
pub(super) fn three() -> App {
    let mut alpha = transfer(1, "alpha", 0);
    alpha.status = Status::Downloading;
    alpha.size = Some(300);
    alpha.tags = ["music".to_owned()].into();
    let mut beta = transfer(2, "beta", 1);
    beta.size = Some(900);
    let mut gamma = transfer(3, "gamma", 2);
    gamma.status = Status::Sharing;
    gamma.size = Some(600);
    gamma.content_dir = Some(PathBuf::from("/downloads/gamma"));
    app_with(vec![alpha, beta, gamma])
}

pub(super) fn selected_name(app: &App) -> Option<&str> {
    app.selected_transfer().map(|t| t.name.as_str())
}

pub(super) fn visible_names(app: &App) -> Vec<&str> {
    app.visible().iter().map(|t| t.name.as_str()).collect()
}

pub(super) fn target(app: &App) -> Target {
    let transfer = app.selected_transfer().unwrap();
    Target {
        id: transfer.id,
        name: transfer.name.clone(),
    }
}

#[test]
fn the_cursor_moves_within_the_list_and_survives_removals() {
    let mut app = three();
    assert_eq!(selected_name(&app), Some("alpha"));
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(selected_name(&app), Some("alpha"));
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(selected_name(&app), Some("beta"));
    press(&mut app, KeyCode::Char('G'));
    assert_eq!(selected_name(&app), Some("gamma"));
    press(&mut app, KeyCode::PageUp);
    assert_eq!(selected_name(&app), Some("alpha"));
    press(&mut app, KeyCode::End);

    // Removing the selected transfer moves the cursor to its neighbor.
    let id = app.selected.unwrap();
    app.apply(ApiEvent::Removed(RemovedInfo { id }));
    assert_eq!(selected_name(&app), Some("beta"));
    app.apply(ApiEvent::Snapshot(vec![]));
    assert_eq!(app.selected, None);
    assert_eq!(press(&mut app, KeyCode::Char(' ')), None);
}

#[test]
fn space_starts_what_is_stopped_and_stops_what_runs() {
    let mut app = three();
    let alpha = target(&app);
    assert_eq!(
        press(&mut app, KeyCode::Char(' ')),
        Some(Request::Transfer(alpha.clone(), Action::Stop))
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('F')),
        Some(Request::Transfer(alpha, Action::ForceStart))
    );
    press(&mut app, KeyCode::Down);
    let beta = target(&app);
    assert_eq!(
        press(&mut app, KeyCode::Char(' ')),
        Some(Request::Transfer(beta, Action::Start))
    );
}

#[test]
fn recheck_and_queue_keys_ask_the_engine() {
    let mut app = three();
    let alpha = target(&app);
    let ask = |action| Some(Request::Transfer(alpha.clone(), action));
    assert_eq!(press(&mut app, KeyCode::Char('R')), ask(Action::Recheck));
    assert_eq!(
        press(&mut app, KeyCode::Char('K')),
        ask(Action::Queue(QueueStep::Up))
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('J')),
        ask(Action::Queue(QueueStep::Down))
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('<')),
        ask(Action::Queue(QueueStep::Top))
    );
    assert_eq!(
        press(&mut app, KeyCode::Char('>')),
        ask(Action::Queue(QueueStep::Bottom))
    );
}

#[test]
fn the_add_modal_sends_every_field() {
    let mut app = three();
    assert_eq!(
        press(&mut app, KeyCode::Char('a')),
        Some(Request::Categories)
    );
    app.finished(
        &Request::Categories,
        Ok(Reply::Categories(categories(&["films", "music"]))),
    );
    app.paste("tempest:abc?n=thing");
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "/media");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "live, 2026");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char(' '));
    let Some(Request::Add(request)) = press(&mut app, KeyCode::Enter) else {
        panic!("enter adds");
    };
    assert_eq!(
        *request,
        AddRequest {
            link: Some("tempest:abc?n=thing".to_owned()),
            save_folder: Some(PathBuf::from("/media")),
            category: Some("music".to_owned()),
            tags: ["2026".to_owned(), "live".to_owned()].into(),
            stopped: true,
            ..AddRequest::default()
        }
    );
    assert!(matches!(app.modal, Some(Modal::Add(_))), "it waits");
    app.finished(&Request::Add(request), Ok(Reply::Added("thing".to_owned())));
    assert_eq!(app.modal, None, "accepted, it closes");
}

#[test]
fn the_add_modal_refuses_a_missing_link_or_relative_folder() {
    let mut app = three();
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    let Some(Modal::Add(form)) = &app.modal else {
        panic!("the modal stays open");
    };
    assert_eq!(form.error.as_deref(), Some("Paste a tempest: link."));
    type_keys(&mut app, "tempest:abc");
    press(&mut app, KeyCode::Down);
    type_keys(&mut app, "downloads");
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    let Some(Modal::Add(form)) = &app.modal else {
        panic!("the modal stays open");
    };
    assert_eq!(
        form.error.as_deref(),
        Some("The save folder must be an absolute path.")
    );
}

#[test]
fn remove_asks_whether_to_keep_the_files() {
    let mut app = three();
    let alpha = target(&app);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha.clone(),
            Action::Remove {
                delete_files: false
            }
        ))
    );
    press(&mut app, KeyCode::Delete);
    press(&mut app, KeyCode::Tab);
    assert!(matches!(
        app.modal,
        Some(Modal::Remove {
            delete_files: true,
            ..
        })
    ));
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha,
            Action::Remove { delete_files: true }
        ))
    );
}

#[test]
fn unpublish_confirms_the_tracker_or_picks_one_first() {
    let mut app = three();
    let alpha = target(&app);
    press(&mut app, KeyCode::Char('U'));
    assert_eq!(app.modal, None);
    let notice = app.notice.take().unwrap().text;
    assert_eq!(notice, "alpha isn't published to any tracker.");

    // Published on one: a confirmation, which esc dismisses.
    let mut published = app.selected_transfer().unwrap().clone();
    published.published = ["https://one.example".to_owned()].into();
    app.apply(ApiEvent::Transfer(Box::new(published.clone())));
    press(&mut app, KeyCode::Char('U'));
    assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
    assert_eq!(press(&mut app, KeyCode::Esc), None);
    press(&mut app, KeyCode::Char('U'));
    let one = Action::Unpublish("https://one.example".to_owned());
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(alpha.clone(), one))
    );

    // On two: pick one, then confirm.
    published.published.insert("https://two.example".to_owned());
    app.apply(ApiEvent::Transfer(Box::new(published)));
    press(&mut app, KeyCode::Char('U'));
    let kind = PickKind::Unpublish(alpha.clone());
    let items: Vec<String> = app.pick_items(&kind).into_iter().map(|i| i.label).collect();
    assert_eq!(items, ["https://one.example", "https://two.example"]);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    let two = Action::Unpublish("https://two.example".to_owned());
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(alpha.clone(), two.clone()))
    );
    let request = Request::Transfer(alpha, two);
    app.finished(&request, Ok(Reply::Done));
    assert_eq!(
        app.notice.unwrap().text,
        "Unpublished alpha from https://two.example."
    );
}

#[test]
fn rename_sends_the_new_name_or_none_to_reset() {
    let mut app = three();
    let alpha = target(&app);
    press(&mut app, KeyCode::Char('r'));
    let Some(Modal::Text { input, .. }) = &app.modal else {
        panic!("rename opens a text modal");
    };
    assert_eq!(input.text(), "alpha");
    type_keys(&mut app, "-2");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha.clone(),
            Action::Rename(Some("alpha-2".to_owned()))
        ))
    );
    press(&mut app, KeyCode::Char('r'));
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(alpha, Action::Rename(None)))
    );
}

#[test]
fn tags_are_edited_as_a_comma_separated_list() {
    let mut app = three();
    let alpha = target(&app);
    press(&mut app, KeyCode::Char('t'));
    let Some(Modal::Text { input, .. }) = &app.modal else {
        panic!("tags open a text modal");
    };
    assert_eq!(input.text(), "music");
    type_keys(&mut app, ", #live");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha,
            Action::Tags(["live".to_owned(), "music".to_owned()].into())
        ))
    );
}

#[test]
fn the_category_picker_lists_the_engines_categories() {
    let mut app = three();
    let alpha = target(&app);
    assert_eq!(
        press(&mut app, KeyCode::Char('C')),
        Some(Request::Categories)
    );
    assert_eq!(app.pick_items(&PickKind::Category(alpha.clone())).len(), 1);
    app.finished(
        &Request::Categories,
        Ok(Reply::Categories(categories(&["films", "music"]))),
    );
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha.clone(),
            Action::Category(Some("films".to_owned()))
        ))
    );
    press(&mut app, KeyCode::Char('C'));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(alpha, Action::Category(None)))
    );
}

#[test]
fn move_storage_needs_an_absolute_path() {
    let mut app = three();
    let alpha = target(&app);
    press(&mut app, KeyCode::Char('m'));
    let Some(Modal::Text { input, .. }) = &app.modal else {
        panic!("move opens a text modal");
    };
    assert_eq!(input.text(), "/downloads");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "elsewhere");
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    let Some(Modal::Text { error, .. }) = &app.modal else {
        panic!("the modal stays open");
    };
    assert_eq!(error.as_deref(), Some("Enter an absolute path."));
    press(&mut app, KeyCode::Home);
    type_keys(&mut app, "/");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(
            alpha,
            Action::Move(PathBuf::from("/elsewhere"))
        ))
    );
}

#[test]
fn open_shows_the_folder_here_or_names_it_when_the_engine_is_remote() {
    let mut app = three();
    let alpha = target(&app).id;
    assert_eq!(
        press(&mut app, KeyCode::Char('o')),
        Some(Request::Open(alpha, PathBuf::from("/downloads")))
    );
    press(&mut app, KeyCode::Char('G'));
    let gamma = target(&app).id;
    assert_eq!(
        press(&mut app, KeyCode::Char('o')),
        Some(Request::Open(gamma, PathBuf::from("/downloads/gamma")))
    );
    app.local = false;
    assert_eq!(press(&mut app, KeyCode::Char('o')), None);
    assert_eq!(
        app.notice.unwrap().text,
        "gamma is in /downloads/gamma on the engine's machine."
    );
}

#[test]
fn esc_closes_every_modal_without_asking_anything() {
    let mut app = three();
    for opener in ['a', 'x', 'r', 't', 'C', 'm', 'f', 's', ':', '?', '/'] {
        press(&mut app, KeyCode::Char(opener));
        assert!(app.modal.is_some(), "{opener} opens a modal");
        assert_eq!(press(&mut app, KeyCode::Esc), None, "{opener}");
        assert_eq!(app.modal, None, "esc closes what {opener} opened");
    }
}

#[test]
fn the_filter_applies_as_it_is_typed() {
    let mut app = three();
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('/'));
    type_keys(&mut app, "#mus");
    assert_eq!(visible_names(&app), ["alpha"]);
    assert_eq!(selected_name(&app), Some("alpha"), "the cursor follows");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.modal, None);
    assert_eq!(app.finding.text, "#mus");
    assert!(app.hints().iter().any(|hint| hint.keys == "esc"));

    // Reopening edits the filter; esc there clears it.
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(visible_names(&app), ["alpha", "beta", "gamma"]);

    // In the list, esc clears the filters.
    press(&mut app, KeyCode::Char('/'));
    type_keys(&mut app, "zzz");
    press(&mut app, KeyCode::Enter);
    assert!(app.visible().is_empty());
    assert_eq!(app.selected, None);
    press(&mut app, KeyCode::Esc);
    assert_eq!(visible_names(&app).len(), 3);
}

#[test]
fn status_and_sort_pickers_change_the_list() {
    let mut app = three();
    press(&mut app, KeyCode::Char('f'));
    let items = app.pick_items(&PickKind::Status);
    assert_eq!(
        (items[0].label.as_str(), items[0].note.as_str()),
        ("All", "3")
    );
    assert_eq!(items[2].note, "1", "one sharing");
    type_keys(&mut app, "jj");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.finding.status, Some(StatusFilter::Sharing));
    assert_eq!(visible_names(&app), ["gamma"]);
    assert_eq!(selected_name(&app), Some("gamma"));

    press(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.finding.status, None);

    press(&mut app, KeyCode::Char('s'));
    type_keys(&mut app, "jj");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.finding.sort.key, SortKey::Size);
    assert_eq!(visible_names(&app), ["beta", "gamma", "alpha"]);
    press(&mut app, KeyCode::Char('S'));
    assert_eq!(visible_names(&app), ["alpha", "gamma", "beta"]);
    assert_eq!(app.finding.describe(), "by size ▲");
}

#[test]
fn the_palette_runs_what_matches() {
    let mut app = three();
    press(&mut app, KeyCode::Char(':'));
    type_keys(&mut app, "rena");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.modal,
        Some(Modal::Text {
            kind: TextKind::Rename,
            ..
        })
    ));
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char(':'));
    type_keys(&mut app, "queue");
    press(&mut app, KeyCode::Down);
    let alpha = target(&app);
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::Transfer(alpha, Action::Queue(QueueStep::Down)))
    );

    press(&mut app, KeyCode::Char(':'));
    type_keys(&mut app, "zzz");
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    assert_eq!(app.modal, None);
}

#[test]
fn areas_switch_by_number_or_bracket_and_wrap() {
    let mut app = app_with(vec![]);
    assert_eq!(press(&mut app, KeyCode::Char('3')), Some(Request::Settings));
    assert_eq!(app.area, Area::Settings);
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(app.area, Area::Transfers);
    // One fetch of the settings at a time.
    assert_eq!(press(&mut app, KeyCode::Char('[')), None);
    assert_eq!(app.area, Area::Settings);
    // Transfer keys do nothing outside Transfers.
    assert_eq!(press(&mut app, KeyCode::Char('x')), None);
    assert_eq!(app.modal, None);
    assert_eq!(
        press(&mut app, KeyCode::Char('2')),
        Some(Request::Categories)
    );
    assert_eq!(press(&mut app, KeyCode::Char(' ')), None);
    assert_eq!(app.modal, None);
}

#[test]
fn hints_list_only_keys_that_work() {
    let has = |app: &App, command: Command| {
        let (keys, _) = command.hint(app.screen()).unwrap();
        app.hints().iter().any(|hint| hint.keys == keys)
    };
    let mut app = app_with(vec![]);
    assert!(!has(&app, Command::StartStop), "nothing to start");
    assert!(!has(&app, Command::Down), "nothing to move to");
    assert!(has(&app, Command::Add));
    app.apply(ApiEvent::Snapshot(vec![transfer(1, "a", 0)]));
    assert!(has(&app, Command::StartStop));
    assert!(!has(&app, Command::ClearFilters));
    press(&mut app, KeyCode::Char('2'));
    assert!(!has(&app, Command::StartStop), "not on Share");
    assert_eq!(app.hints().last().unwrap().keys, "?");
    press(&mut app, KeyCode::Char('?'));
    assert_eq!(app.hints().len(), 1);
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(app.modal, None, "any key closes help");
}

#[test]
fn help_lists_the_areas_sections() {
    let mut app = three();
    let titles =
        |app: &App| -> Vec<&str> { app.help_sections().iter().map(|(s, _)| s.title()).collect() };
    assert_eq!(
        titles(&app),
        ["Move", "Transfer", "Queue", "Detail", "Find", "App"]
    );
    press(&mut app, KeyCode::Char('3'));
    assert_eq!(titles(&app), ["Move", "Change", "App"]);
    press(&mut app, KeyCode::Char('2'));
    assert_eq!(titles(&app), ["Move", "Transfer", "Share", "App"]);
}

#[test]
fn esc_dismisses_a_notice_before_doing_anything_else() {
    let mut app = three();
    press(&mut app, KeyCode::Char('/'));
    type_keys(&mut app, "#mus");
    press(&mut app, KeyCode::Enter);
    let request = Request::Transfer(target(&app), Action::Start);
    app.finished(&request, Err("The engine is shutting down.".to_owned()));
    assert!(app.notice.is_some());

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.notice, None, "the first esc closes the notice");
    assert_eq!(
        visible_names(&app),
        ["alpha"],
        "and leaves the filter alone"
    );

    press(&mut app, KeyCode::Esc);
    assert_eq!(
        visible_names(&app),
        ["alpha", "beta", "gamma"],
        "the next clears it"
    );
}

#[test]
fn notices_report_successes_and_failures_and_fade() {
    let mut app = three();
    let request = Request::Transfer(target(&app), Action::Start);
    app.finished(&request, Err("The engine is shutting down.".to_owned()));
    let notice = app.notice.clone().unwrap();
    assert_eq!(
        notice.text,
        "Couldn't start alpha: The engine is shutting down."
    );
    assert!(notice.error);
    for _ in 0..NOTICE_TICKS {
        app.tick();
    }
    assert_eq!(app.notice, None);

    let cases = [
        (Action::Start, "Started alpha."),
        (
            Action::Remove { delete_files: true },
            "Removed alpha and deleted its files.",
        ),
        (Action::Rename(None), "alpha has its bundle's name again."),
        (
            Action::Tags(["a".to_owned(), "b".to_owned()].into()),
            "Tagged alpha: a, b.",
        ),
        (
            Action::Queue(QueueStep::Top),
            "Moved alpha to the top of the queue.",
        ),
    ];
    for (action, text) in cases {
        app.finished(&Request::Transfer(target(&app), action), Ok(Reply::Done));
        let notice = app.notice.clone().unwrap();
        assert_eq!((notice.text.as_str(), notice.error), (text, false));
    }
    let add = Request::Add(Box::default());
    app.finished(&add, Ok(Reply::Added("thing".to_owned())));
    assert_eq!(app.notice.clone().unwrap().text, "Added thing.");
    app.finished(&add, Err("The link is not valid.".to_owned()));
    assert_eq!(
        app.notice.clone().unwrap().text,
        "Couldn't add the link: The link is not valid."
    );
}

#[test]
fn enter_opens_the_detail_and_esc_goes_back_one_level() {
    let mut app = three();
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        None,
        "the summary needs no fetch"
    );
    assert_eq!((app.screen(), app.tab), (Screen::Detail, Tab::Summary));
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(app.screen(), Screen::Chunks);
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(app.screen(), Screen::Rates);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::Detail);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), Screen::List);
    // Back in the list, esc clears the filters again.
    assert_eq!(
        Command::for_key(app.screen(), key(KeyCode::Esc)),
        Some(Command::ClearFilters)
    );
}

#[test]
fn g_is_the_first_transfer_in_the_list_and_the_rates_in_the_detail() {
    let mut app = three();
    type_keys(&mut app, "jj");
    type_keys(&mut app, "g");
    assert_eq!(selected_name(&app), Some("alpha"));
    type_keys(&mut app, "j");
    press(&mut app, KeyCode::Enter);
    type_keys(&mut app, "g");
    assert_eq!(app.screen(), Screen::Rates);
    assert_eq!(selected_name(&app), Some("beta"), "the selection stays");
}

#[test]
fn tabs_cycle_both_ways_and_fetch_what_they_show() {
    let mut app = three();
    press(&mut app, KeyCode::Enter);
    let id = app.selected.unwrap();
    assert_eq!(
        press(&mut app, KeyCode::Tab),
        Some(Request::Look(id, Look::Files))
    );
    assert_eq!(app.tab, Tab::Files);
    // One fetch at a time: the next tick waits for the answer.
    assert_eq!(app.tick(), []);
    app.finished(
        &Request::Look(id, Look::Files),
        Ok(Reply::Files(Vec::new())),
    );
    assert_eq!(app.tick(), [Request::Look(id, Look::Files)]);
    assert_eq!(app.notice, None, "fetches make no notices");

    assert_eq!(
        type_keys(&mut app, "l"),
        Some(Request::Look(id, Look::Peers))
    );
    assert_eq!(
        type_keys(&mut app, "l"),
        None,
        "decisions come from the events"
    );
    assert_eq!(app.tab, Tab::Decisions);
    press(&mut app, KeyCode::Right);
    assert_eq!(app.tab, Tab::Summary);
    press(&mut app, KeyCode::BackTab);
    type_keys(&mut app, "h");
    press(&mut app, KeyCode::Left);
    assert_eq!(app.tab, Tab::Files);

    // The chunk map asks for as many cells as the terminal shows.
    app.resize(100, 30);
    let cells = chunkmap::cells_for_screen(100, 30);
    assert_eq!(
        type_keys(&mut app, "c"),
        Some(Request::Look(id, Look::Chunks(cells)))
    );
}

#[test]
fn a_failed_fetch_shows_in_its_view() {
    let mut app = three();
    press(&mut app, KeyCode::Enter);
    let request = press(&mut app, KeyCode::Tab).unwrap();
    app.finished(&request, Err("the engine is shutting down".to_owned()));
    let files = app.inside().unwrap().files.clone();
    assert_eq!(
        files,
        Some(Err(
            "Couldn't load the files: the engine is shutting down".to_owned()
        ))
    );
    assert_eq!(app.notice, None);
}

#[test]
fn the_detail_scrolls_within_its_rows() {
    let mut app = three();
    press(&mut app, KeyCode::Enter);
    for _ in 0..30 {
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(app.scroll, SUMMARY_ROWS - 1);
    press(&mut app, KeyCode::Home);
    assert_eq!(app.scroll, 0);
    assert_eq!(selected_name(&app), Some("alpha"), "the cursor stays put");

    // Decisions count back from the newest: up scrolls back, down toward the newest.
    let id = app.selected.unwrap();
    for n in 0..5 {
        app.apply(ApiEvent::Decision(tempest_api::events::DecisionInfo {
            id,
            time: f64::from(n),
            kind: "requested".to_owned(),
            reason: None,
            text: n.to_string(),
        }));
    }
    press(&mut app, KeyCode::BackTab);
    assert_eq!(app.tab, Tab::Decisions);
    type_keys(&mut app, "kkkkkkk");
    assert_eq!(app.scroll, 4);
    type_keys(&mut app, "j");
    assert_eq!(app.scroll, 3);
    type_keys(&mut app, "G");
    assert_eq!(app.scroll, 0);
}

#[test]
fn losing_the_selection_closes_the_detail() {
    let mut app = app_with(vec![transfer(1, "only", 0)]);
    press(&mut app, KeyCode::Enter);
    type_keys(&mut app, "c");
    app.apply(ApiEvent::Removed(RemovedInfo {
        id: app.selected.unwrap(),
    }));
    assert_eq!(app.screen(), Screen::List);
}

#[test]
fn hints_follow_the_screen() {
    let mut app = three();
    let keys = |app: &App| -> Vec<&str> { app.hints().iter().map(|hint| hint.keys).collect() };
    assert!(keys(&app).contains(&"enter"));
    press(&mut app, KeyCode::Enter);
    let detail = keys(&app);
    assert_eq!(detail[..5], ["esc", "j/k", "h/l", "c", "g"]);
    assert!(!detail.contains(&"/"), "no filtering from the detail");
    assert_eq!(app.screen_title(), "Detail");
    type_keys(&mut app, "c");
    assert_eq!(keys(&app)[..2], ["esc", "g"]);
    assert_eq!(app.help_sections()[0].0, Section::View);
}

fn transfer(seed: u8, name: &str, position: u32) -> TransferInfo {
    TransferInfo::example(seed, name, position)
}

#[test]
fn completing_or_failing_shows_a_notice_once() {
    let mut app = three();
    assert_eq!(app.notice, None, "the first look says nothing");

    let mut done = app.model.transfers()[0].clone();
    done.status = Status::Sharing;
    done.completed = Some(1);
    app.apply(ApiEvent::Transfer(Box::new(done.clone())));
    let notice = app.notice.take().unwrap();
    assert_eq!(
        notice.text,
        "alpha is complete. Every chunk is on disk and checked."
    );
    assert!(!notice.error);
    // The same state again says nothing more.
    app.apply(ApiEvent::Transfer(Box::new(done)));
    assert_eq!(app.notice, None);

    let mut failed = app.model.transfers()[1].clone();
    failed.status = Status::Errored {
        reason: "The disk is full.".to_owned(),
    };
    app.apply(ApiEvent::Transfer(Box::new(failed)));
    let notice = app.notice.take().unwrap();
    assert_eq!(notice.text, "beta stopped. The disk is full.");
    assert!(notice.error);

    // A transfer that arrives already complete, such as a new share, is not news.
    let mut new = transfer(9, "delta", 3);
    new.completed = Some(1);
    app.apply(ApiEvent::Transfer(Box::new(new)));
    assert_eq!(app.notice, None);
}

#[test]
fn the_first_run_explains_the_network_defaults_once() {
    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut view = settings();
    view.settings.ui.network_explained = false;
    let _ = app.load_settings();
    let reply = Reply::Loaded(Box::new(view.clone()), categories(&[]));
    let recorded = app.finished(&Request::Settings, Ok(reply)).unwrap();
    assert_eq!(app.modal, Some(Modal::Network));
    let Request::ChangeSettings { changes, .. } = &recorded else {
        panic!("{recorded:?}");
    };
    assert_eq!(changes.get("ui.network_explained"), Some(&json!(true)));

    // Any key closes it, and the engine's answer settles it.
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(app.modal, None);
    view.settings.ui.network_explained = true;
    let answer = Reply::Settings(Box::new(view.clone()));
    app.finished(&recorded, Ok(answer));
    let _ = app.load_settings();
    let reply = Reply::Loaded(Box::new(view), categories(&[]));
    assert_eq!(app.finished(&Request::Settings, Ok(reply)), None);
    assert_eq!(app.modal, None);
}

#[test]
fn quitting_an_embedded_engine_asks_what_to_do_with_active_transfers() {
    // On a daemon, quitting leaves the transfers alone.
    let mut app = three();
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);
    assert_eq!(app.exit(), Exit::Stop);

    // On the TUI's own engine, two transfers are active, and the question is asked.
    let mut app = three();
    app.origin = Origin::Embedded;
    assert_eq!(app.active(), 2);
    press(&mut app, KeyCode::Char('q'));
    assert!(!app.quit);
    assert_eq!(app.modal, Some(Modal::Quit { active: 2 }));
    // Esc stays; other keys leave the question open.
    press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.modal, Some(Modal::Quit { active: 2 }));
    press(&mut app, KeyCode::Esc);
    assert!(!app.quit && app.modal.is_none());

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Char('d'));
    assert!(app.quit);
    assert_eq!(app.exit(), Exit::HandOff);

    let mut app = three();
    app.origin = Origin::Embedded;
    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Char('s'));
    assert!(app.quit);
    assert_eq!(app.exit(), Exit::Stop);

    // With nothing active, there is nothing to ask.
    let mut app = app_with(vec![transfer(1, "idle", 0)]);
    app.origin = Origin::Embedded;
    press(&mut app, KeyCode::Char('q'));
    assert!(app.quit);

    // Ctrl-C asks too, and a second one stops everything.
    let mut app = three();
    app.origin = Origin::Embedded;
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    app.key(ctrl_c);
    assert!(!app.quit && app.modal.is_some());
    app.key(ctrl_c);
    assert!(app.quit);
    assert_eq!(app.exit(), Exit::Stop);
}
