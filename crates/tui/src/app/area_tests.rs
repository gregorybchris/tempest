//! Keys in the Share and Settings areas, and what the user keeps (D-195 to D-197).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tempest_engine::Kind;

use super::tests::{app_with, categories, load, press, settings, three, type_keys};
use super::*;

/// The settings change `request` asks for, if it is one.
fn changes(request: Option<Request>) -> BTreeMap<String, Value> {
    match request {
        Some(Request::ChangeSettings { changes, .. }) => changes,
        other => panic!("not a change of settings: {other:?}"),
    }
}

fn change(key: &str, value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(key.to_owned(), value)])
}

/// The Settings area, with the default settings and categories `films` and `music`.
fn settings_area() -> App {
    let mut app = three();
    load(&mut app, settings(), &["films", "music"]);
    press(&mut app, KeyCode::Char('3'));
    load(&mut app, settings(), &["films", "music"]);
    app
}

/// Moves the cursor to the row labeled `label`.
fn go_to(app: &mut App, label: &str) {
    let index = app
        .rows()
        .iter()
        .position(|row| row.label == label)
        .unwrap_or_else(|| panic!("no row {label}"));
    press(app, KeyCode::Home);
    for _ in 0..index {
        press(app, KeyCode::Char('j'));
    }
    assert_eq!(app.current_row().unwrap().label, label);
}

#[test]
fn values_are_typed_checked_and_sent() {
    let mut app = settings_area();
    assert_eq!(app.current_row().unwrap().label, "Download");
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    type_keys(&mut app, "fast");
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    let Some(Modal::Value(edit)) = &app.modal else {
        panic!("{:?}", app.modal)
    };
    assert!(edit.error.is_some(), "a bad speed is refused in place");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "1.5M");
    assert_eq!(
        changes(press(&mut app, KeyCode::Enter)),
        change("limits.download", json!(1_500_000))
    );
    assert_eq!(app.modal, None);

    go_to(&mut app, "Active uploads");
    press(&mut app, KeyCode::Enter);
    let Some(Modal::Value(edit)) = &app.modal else {
        panic!()
    };
    assert_eq!(edit.input.text(), "", "no limit is the default (D-220)");
    type_keys(&mut app, "5");
    assert_eq!(
        changes(press(&mut app, KeyCode::Enter)),
        change("queue.max_active_uploads", json!(5))
    );
}

#[test]
fn toggles_and_choices_change_at_once() {
    let mut app = settings_area();
    go_to(&mut app, "Switched on");
    assert_eq!(
        changes(press(&mut app, KeyCode::Char(' '))),
        change("limits.alternative.enabled", json!(true))
    );
    go_to(&mut app, "At the limit");
    assert_eq!(
        changes(press(&mut app, KeyCode::Enter)),
        change("sharing.action", json!("remove"))
    );
}

#[test]
fn the_schedule_is_a_form() {
    let mut app = settings_area();
    go_to(&mut app, "Schedule");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "22:00");
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "6:00");
    let sent = changes(press(&mut app, KeyCode::Enter));
    assert_eq!(
        sent["limits.alternative.schedule"],
        json!({ "days": ["mon", "tue", "wed", "thu", "fri"], "from": "22:00", "to": "06:00" })
    );
}

#[test]
fn x_resets_a_changed_setting_and_says_when_there_is_nothing_to_reset() {
    let mut view = settings();
    view.changed = change("limits.upload", json!(1000));
    let mut app = settings_area();
    load(&mut app, view, &[]);
    go_to(&mut app, "Upload");
    assert_eq!(
        press(&mut app, KeyCode::Char('x')),
        Some(Request::ResetSetting {
            key: "limits.upload".to_owned(),
            about: "upload".to_owned(),
        })
    );
    go_to(&mut app, "Download");
    assert_eq!(press(&mut app, KeyCode::Char('x')), None);
    assert!(app.notice.as_ref().unwrap().text.contains("config file"));
}

#[test]
fn categories_are_added_changed_and_removed() {
    let mut app = settings_area();
    press(&mut app, KeyCode::Char('a'));
    type_keys(&mut app, "films");
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "/srv/other");
    assert_eq!(press(&mut app, KeyCode::Enter), None, "films exists");
    press(&mut app, KeyCode::BackTab);
    type_keys(&mut app, "-4k");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::SetCategory {
            name: "films-4k".to_owned(),
            folder: PathBuf::from("/srv/other"),
            new: true,
        })
    );
    // Once it is saved, the categories are fetched again.
    let request = Request::SetCategory {
        name: "films-4k".to_owned(),
        folder: PathBuf::from("/srv/other"),
        new: true,
    };
    assert_eq!(
        app.finished(&request, Ok(Reply::Done)),
        Some(Request::Categories)
    );

    go_to(&mut app, "music");
    press(&mut app, KeyCode::Enter);
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "/media/music");
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::SetCategory {
            name: "music".to_owned(),
            folder: PathBuf::from("/media/music"),
            new: false,
        })
    );

    press(&mut app, KeyCode::Char('x'));
    assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
    assert_eq!(
        press(&mut app, KeyCode::Enter),
        Some(Request::RemoveCategory("music".to_owned()))
    );
    press(&mut app, KeyCode::Char('x'));
    assert_eq!(press(&mut app, KeyCode::Esc), None, "esc keeps it");
}

#[test]
fn a_tag_shows_its_transfers() {
    let mut app = settings_area();
    go_to(&mut app, "#music");
    assert_eq!(app.current_row().unwrap().value, "1 transfer");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.area, Area::Transfers);
    assert_eq!(super::tests::visible_names(&app), ["alpha"]);
}

#[test]
fn trackers_and_relays_are_added_and_removed() {
    let mut view = settings();
    view.settings.trackers = vec!["https://a.example".to_owned()];
    let mut app = three();
    press(&mut app, KeyCode::Char('3'));
    load(&mut app, view, &[]);
    assert_eq!(app.screen(), Screen::Settings);
    go_to(&mut app, "https://a.example");

    press(&mut app, KeyCode::Char('a'));
    type_keys(&mut app, "ftp://b.example");
    assert_eq!(press(&mut app, KeyCode::Enter), None, "not http");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "https://a.example");
    assert_eq!(press(&mut app, KeyCode::Enter), None, "already listed");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "https://b.example");
    assert_eq!(
        changes(press(&mut app, KeyCode::Enter)),
        change(
            "trackers",
            json!(["https://a.example", "https://b.example"])
        )
    );

    assert_eq!(
        changes(press(&mut app, KeyCode::Char('x'))),
        change("trackers", json!([])),
        "x removes the tracker under the cursor"
    );

    // In the relays section, `a` adds a relay.
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.current_row().unwrap().edit, Edit::NewRelay);
    press(&mut app, KeyCode::Char('a'));
    let key = "ab".repeat(32);
    type_keys(&mut app, &format!("{key}@relay.example.org"));
    assert_eq!(
        changes(press(&mut app, KeyCode::Enter)),
        change("relays.known", json!([format!("{key}@relay.example.org")]))
    );
}

#[test]
fn shares_are_made_from_the_form() {
    let mut app = three();
    let mut gamma = app.model.transfers()[2].clone();
    gamma.kind = Kind::Share;
    app.apply(ApiEvent::Transfer(Box::new(gamma)));
    assert_eq!(
        press(&mut app, KeyCode::Char('2')),
        Some(Request::Categories)
    );
    app.finished(
        &Request::Categories,
        Ok(Reply::Categories(categories(&["films"]))),
    );
    assert_eq!(app.selected_share().unwrap().name, "gamma");
    // Keys go to the area until `a` opens the form.
    assert_eq!(type_keys(&mut app, "q"), None);
    assert!(app.quit);
    app.quit = false;
    press(&mut app, KeyCode::Char('a'));
    assert!(app.share.editing);
    assert_eq!(press(&mut app, KeyCode::Enter), None);
    assert!(app.share.form.error.is_some(), "a path is needed");
    type_keys(&mut app, "relative/q");
    assert!(!app.quit, "the form takes the keys");
    press(&mut app, KeyCode::Enter);
    assert!(app.share.form.error.as_ref().unwrap().contains("absolute"));

    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_keys(&mut app, "/srv/notes");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "Field notes");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "live, #2026");
    let request = press(&mut app, KeyCode::Enter).unwrap();
    let Request::Share(add) = &request else {
        panic!("{request:?}")
    };
    assert_eq!(add.path, Some(PathBuf::from("/srv/notes")));
    assert_eq!(add.name, None, "an empty name is the file's");
    assert_eq!(add.comment, "Field notes");
    assert_eq!(add.category.as_deref(), Some("films"));
    assert_eq!(add.tags, parse_tags("live, 2026"));
    assert_eq!(app.share.building, Some(PathBuf::from("/srv/notes")));
    assert!(!app.share.editing);

    let mut made = transfer(9, "notes", 3);
    made.kind = Kind::Share;
    app.apply(ApiEvent::Transfer(Box::new(made.clone())));
    app.finished(&request, Ok(Reply::Shared(Box::new(made.clone()))));
    assert_eq!(app.share.building, None);
    assert_eq!(app.share.form.text("Path"), "", "the form empties");
    assert_eq!(app.selected_share().unwrap().id, made.id);
    assert_eq!(
        press(&mut app, KeyCode::Char('y')),
        Some(Request::Copy {
            text: made.link.clone(),
            what: "notes's link".to_owned(),
        })
    );
    press(&mut app, KeyCode::Enter);
    assert_eq!((app.area, app.view), (Area::Transfers, View::Detail));
    assert_eq!(app.selected, Some(made.id));
}

#[test]
fn a_failed_share_goes_back_to_the_form() {
    let mut app = three();
    press(&mut app, KeyCode::Char('2'));
    press(&mut app, KeyCode::Char('a'));
    type_keys(&mut app, "/nowhere");
    let request = press(&mut app, KeyCode::Enter).unwrap();
    app.finished(&request, Err("no such file".to_owned()));
    assert!(app.share.editing);
    assert_eq!(app.share.form.text("Path"), "/nowhere");
    assert!(
        app.share
            .form
            .error
            .as_ref()
            .unwrap()
            .contains("no such file")
    );
    assert!(app.notice.as_ref().unwrap().error);
}

#[test]
fn the_theme_columns_and_views_come_from_the_settings_once() {
    let mut view = settings();
    view.settings.ui.theme = "amber".to_owned();
    view.settings.ui.columns = Some(vec!["tags".to_owned()]);
    let mut app = App::new(Origin::Daemon, Theme::default());
    assert_eq!(app.tick(), [Request::Settings], "fetched at the start");
    app.finished(
        &Request::Settings,
        Ok(Reply::Loaded(Box::new(view.clone()), BTreeMap::new())),
    );
    assert_eq!(app.theme, Theme::AMBER);
    assert_eq!(app.columns, [Column::Tags]);
    assert_eq!(app.tick(), [], "and not again outside the settings areas");

    // `T` keeps the next theme; a later fetch doesn't take it back.
    assert_eq!(
        changes(press(&mut app, KeyCode::Char('T'))),
        change("ui.theme", json!("vector"))
    );
    load(&mut app, view.clone(), &[]);
    assert_eq!(app.theme, Theme::VECTOR);
    assert_eq!(
        app.columns,
        [Column::Tags],
        "no change is waiting, so it is taken"
    );

    // While a change to the columns waits, a fetch keeps them.
    press(&mut app, KeyCode::Char('|'));
    press(&mut app, KeyCode::Char(' '));
    let request = press(&mut app, KeyCode::Enter);
    assert_eq!(
        changes(request.clone()),
        change("ui.columns", json!(["size", "tags"]))
    );
    assert_eq!(app.columns, [Column::Size, Column::Tags]);
    load(&mut app, view.clone(), &[]);
    assert_eq!(app.columns, [Column::Size, Column::Tags]);
    let mut saved = view;
    saved.settings.ui.columns = Some(vec!["size".to_owned(), "tags".to_owned()]);
    app.finished(&request.unwrap(), Ok(Reply::Settings(Box::new(saved))));
    assert_eq!(app.columns, [Column::Size, Column::Tags]);
}

#[test]
fn views_are_saved_shown_and_deleted() {
    let mut app = app_with(vec![]);
    type_keys(&mut app, "/#music");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('V'));
    assert_eq!(press(&mut app, KeyCode::Enter), None, "a view needs a name");
    type_keys(&mut app, "Music");
    let sent = changes(press(&mut app, KeyCode::Enter));
    assert_eq!(
        sent["ui.views"],
        json!([{ "name": "Music", "query": "#music" }])
    );
    assert_eq!(app.current_view(), Some("Music"));

    press(&mut app, KeyCode::Esc);
    assert_eq!(app.finding, Finding::default());
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.finding.text, "#music");

    press(&mut app, KeyCode::Char('v'));
    let sent = changes(press(&mut app, KeyCode::Char('x')));
    assert_eq!(sent["ui.views"], json!([]));
    assert!(app.views.is_empty());
}

#[test]
fn a_transfers_limits_keep_its_other_settings() {
    let mut app = three();
    let mut alpha = app.selected_transfer().unwrap().clone();
    alpha.settings = BTreeMap::from([
        ("limits.upload".to_owned(), json!(4_000_000)),
        ("upload.reciprocal_slots".to_owned(), json!(6)),
    ]);
    app.apply(ApiEvent::Transfer(Box::new(alpha.clone())));
    press(&mut app, KeyCode::Char('L'));
    let Some(Modal::Form(form)) = &app.modal else {
        panic!("{:?}", app.modal)
    };
    assert_eq!(form.1.text("Upload"), "4M");
    type_keys(&mut app, "500K");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    type_keys(&mut app, "none");
    // Past the time and the idle timeout to the action.
    for _ in 0..3 {
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    let request = press(&mut app, KeyCode::Enter);
    let Some(Request::Transfer(_, Action::Settings(sent))) = request else {
        panic!("{request:?}")
    };
    assert_eq!(
        sent,
        BTreeMap::from([
            ("limits.download".to_owned(), json!(500_000)),
            ("limits.upload".to_owned(), json!(4_000_000)),
            ("sharing.ratio".to_owned(), Value::Null),
            ("sharing.action".to_owned(), json!("remove")),
            ("upload.reciprocal_slots".to_owned(), json!(6)),
        ])
    );
}

#[test]
fn help_scrolls_only_as_far_as_it_overflows() {
    let mut app = three();
    app.resize(80, 24);
    press(&mut app, KeyCode::Char('?'));
    for _ in 0..100 {
        press(&mut app, KeyCode::Char('j'));
    }
    let Some(Modal::Help { scroll }) = app.modal else {
        panic!()
    };
    assert!(scroll > 0 && scroll == crate::ui::help_overflow(&app));
    app.resize(200, 60);
    assert_eq!(
        crate::ui::help_overflow(&app),
        0,
        "a big screen fits it all"
    );
}

fn transfer(seed: u8, name: &str, position: u32) -> tempest_engine::TransferInfo {
    tempest_engine::TransferInfo::example(seed, name, position)
}

#[test]
fn settings_that_failed_to_load_are_asked_for_again() {
    let mut view = settings();
    view.settings.ui.theme = "amber".to_owned();
    let mut app = App::new(Origin::Daemon, Theme::default());
    assert_eq!(app.tick(), [Request::Settings]);
    app.finished(
        &Request::Settings,
        Err("cannot reach the engine".to_owned()),
    );
    for _ in 1..SETTINGS_TICKS {
        assert_eq!(app.tick(), [], "not at once");
    }
    assert_eq!(app.tick(), [Request::Settings], "but a few seconds later");
    app.finished(
        &Request::Settings,
        Ok(Reply::Loaded(Box::new(view), BTreeMap::new())),
    );
    assert_eq!(app.theme, Theme::AMBER, "the saved theme still applies");
}
