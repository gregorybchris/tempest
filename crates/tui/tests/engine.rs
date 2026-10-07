//! The TUI following a real engine through its API and event stream, and changing it with keys.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::AddRequest;
use tempest_engine::api::client::{ApiClient, EventStream};
use tempest_engine::api::server::ApiServer;
use tempest_engine::config::{ApiSettings, LanSettings, PeerSettings};
use tempest_engine::{Engine, EngineConfig, TransferInfo};
use tempest_tui::app::{App, Origin};
use tempest_tui::carry_out;
use tempest_tui::theme::Theme;
use tempest_tui::ui;

/// An engine keeping its data in `dir` and serving its API, and a client of it.
async fn engine(dir: &Path) -> (Engine, ApiClient, tokio::task::JoinHandle<()>) {
    let config = EngineConfig {
        peers: PeerSettings {
            port: 0,
            bind: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        },
        lan: LanSettings {
            enabled: false,
            ..LanSettings::default()
        },
        api: ApiSettings {
            port: 0,
            ..ApiSettings::default()
        },
        data_dir: Some(dir.to_owned()),
        ..EngineConfig::default()
    };
    let engine = Engine::open(config.clone()).await.unwrap();
    let server = ApiServer::bind(&config.api, dir).await.unwrap();
    let serving = tokio::spawn({
        let engine = engine.clone();
        async move {
            let _ = server.serve(engine, std::future::pending()).await;
        }
    });
    let client = ApiClient::for_data_dir(dir).unwrap().unwrap();
    (engine, client, serving)
}

/// Shares a new file named `name` in `dir`.
async fn share(client: &ApiClient, dir: &Path, name: &str) {
    let file = dir.join(name);
    std::fs::write(&file, name).unwrap();
    let request = AddRequest {
        path: Some(file),
        ..AddRequest::default()
    };
    client.add(&request).await.unwrap();
}

/// Applies events to `app` until `done` holds, failing after a while.
async fn follow_until(app: &mut App, events: &mut EventStream, done: impl Fn(&App) -> bool) {
    let followed = tokio::time::timeout(Duration::from_secs(10), async {
        while !done(app) {
            let (event, _) = events.next().await.unwrap().unwrap();
            app.apply(event);
        }
    })
    .await;
    assert!(followed.is_ok(), "the engine never got there");
}

/// Presses `keys` and carries out what they ask, reporting it to the app as the TUI does.
async fn press(app: &mut App, client: &ApiClient, keys: &[KeyCode]) {
    for code in keys {
        let mut next = app.key(KeyEvent::new(*code, KeyModifiers::NONE));
        while let Some(request) = next {
            let result = carry_out(client, &request).await;
            next = app.finished(&request, result);
        }
    }
}

fn chars(text: &str) -> Vec<KeyCode> {
    text.chars().map(KeyCode::Char).collect()
}

fn names(app: &App) -> Vec<&str> {
    app.visible().iter().map(|t| t.name.as_str()).collect()
}

fn find<'a>(app: &'a App, name: &str) -> Option<&'a TransferInfo> {
    app.model.transfers().iter().find(|t| t.name == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_shows_what_the_engine_shares() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, client, serving) = engine(dir.path()).await;
    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut events = client.events(None).await.unwrap();
    share(&client, dir.path(), "field-notes.txt").await;

    follow_until(&mut app, &mut events, |app| {
        app.selected_transfer()
            .is_some_and(|t| t.status.name() == "sharing")
    })
    .await;

    let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("▌ field-notes.txt"), "{screen}");
    assert!(
        screen.contains("Sharing. No one is downloading it right now."),
        "{screen}"
    );

    engine.shutdown().await;
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn keys_rename_tag_reorder_and_remove_through_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, client, serving) = engine(dir.path()).await;
    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut events = client.events(None).await.unwrap();
    share(&client, dir.path(), "a.txt").await;
    share(&client, dir.path(), "b.txt").await;
    follow_until(&mut app, &mut events, |app| {
        names(app) == ["a.txt", "b.txt"]
    })
    .await;

    // Rename, which the list shows once the engine says so.
    let mut keys = vec![KeyCode::Char('r')];
    keys.extend(chars("-notes"));
    keys.push(KeyCode::Enter);
    press(&mut app, &client, &keys).await;
    assert_eq!(
        app.notice.as_ref().unwrap().text,
        "Renamed a.txt to a.txt-notes."
    );
    follow_until(&mut app, &mut events, |app| {
        find(app, "a.txt-notes").is_some()
    })
    .await;

    // Tags, then a filter by tag.
    let mut keys = vec![KeyCode::Char('t')];
    keys.extend(chars("field, 2026"));
    keys.push(KeyCode::Enter);
    press(&mut app, &client, &keys).await;
    follow_until(&mut app, &mut events, |app| {
        find(app, "a.txt-notes").is_some_and(|t| t.tags.len() == 2)
    })
    .await;
    press(&mut app, &client, &[KeyCode::Char('/')]).await;
    press(&mut app, &client, &chars("#fie")).await;
    assert_eq!(names(&app), ["a.txt-notes"]);
    press(&mut app, &client, &[KeyCode::Esc]).await;

    // Down the queue.
    press(&mut app, &client, &[KeyCode::Char('J')]).await;
    follow_until(&mut app, &mut events, |app| {
        names(app) == ["b.txt", "a.txt-notes"]
    })
    .await;

    // Remove, keeping the files.
    press(&mut app, &client, &[KeyCode::Char('x'), KeyCode::Enter]).await;
    follow_until(&mut app, &mut events, |app| names(app) == ["b.txt"]).await;
    assert!(dir.path().join("a.txt").exists());
    assert_eq!(
        app.notice.as_ref().unwrap().text,
        "Removed a.txt-notes. Its files stay."
    );

    // A refusal comes back as a notice.
    let mut keys = vec![KeyCode::Char('a')];
    keys.extend(chars("tempest:nonsense"));
    keys.push(KeyCode::Enter);
    press(&mut app, &client, &keys).await;
    let notice = app.notice.as_ref().unwrap();
    assert!(notice.error, "{notice:?}");
    assert!(
        notice.text.starts_with("Couldn't add the link: "),
        "{notice:?}"
    );

    engine.shutdown().await;
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_detail_fetches_files_peers_and_chunks_from_the_engine() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, client, serving) = engine(dir.path()).await;
    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut events = client.events(None).await.unwrap();
    share(&client, dir.path(), "field-notes.txt").await;
    follow_until(&mut app, &mut events, |app| {
        app.selected_transfer()
            .is_some_and(|t| t.status.name() == "sharing")
    })
    .await;

    // Files, then peers: none, since nobody is downloading it.
    press(&mut app, &client, &[KeyCode::Enter, KeyCode::Tab]).await;
    let files = app.inside().and_then(|inside| inside.files.clone());
    let files = files.unwrap().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        (files[0].path.as_str(), files[0].progress),
        ("field-notes.txt", 1.0)
    );
    press(&mut app, &client, &[KeyCode::Tab]).await;
    let peers = app.inside().and_then(|inside| inside.peers.clone());
    assert_eq!(peers, Some(Ok(Vec::new())));

    // The chunk map, sized for the terminal: one chunk, so one cell.
    app.resize(80, 24);
    press(&mut app, &client, &[KeyCode::Char('c')]).await;
    let map = app.inside().and_then(|inside| inside.chunks.clone());
    let map = map.unwrap().unwrap();
    assert!(map.running);
    assert_eq!((map.chunks, map.cells.len()), (1, 1));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &app)).unwrap();
    let screen = terminal.backend().to_string();
    assert!(screen.contains("1 chunk, 100% stored"), "{screen}");

    engine.shutdown().await;
    serving.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_areas_share_change_settings_and_keep_the_theme() {
    let dir = tempfile::tempdir().unwrap();
    let (engine, client, serving) = engine(dir.path()).await;
    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut events = client.events(None).await.unwrap();
    let request = app.load_settings().unwrap();
    let result = carry_out(&client, &request).await;
    app.finished(&request, result);

    // Share a file from the Share area.
    let file = dir.path().join("field-notes.txt");
    std::fs::write(&file, "notes").unwrap();
    press(&mut app, &client, &[KeyCode::Char('2'), KeyCode::Char('a')]).await;
    app.paste(&file.display().to_string());
    press(&mut app, &client, &[KeyCode::Enter]).await;
    let notice = app.notice.clone().unwrap();
    assert_eq!(
        notice.text, "Shared field-notes.txt. Press y to copy its link.",
        "{notice:?}"
    );
    follow_until(&mut app, &mut events, |app| app.selected_share().is_some()).await;
    assert!(app.selected_share().unwrap().link.starts_with("tempest:"));

    // A speed limit and a category from the Settings area.
    press(&mut app, &client, &[KeyCode::Char('4'), KeyCode::Enter]).await;
    press(&mut app, &client, &chars("2M")).await;
    press(&mut app, &client, &[KeyCode::Enter]).await;
    let settings = client.settings().await.unwrap();
    assert_eq!(settings.settings.limits.download, Some(2_000_000));
    assert_eq!(app.rows()[0].value, "2.0M/s");
    press(&mut app, &client, &[KeyCode::Char('a')]).await;
    press(&mut app, &client, &chars("films")).await;
    press(&mut app, &client, &[KeyCode::Tab]).await;
    let folder = dir.path().join("films");
    app.paste(&folder.display().to_string());
    press(&mut app, &client, &[KeyCode::Enter]).await;
    assert!(app.categories.as_ref().unwrap().contains_key("films"));
    assert_eq!(
        client.categories().await.unwrap()["films"].save_folder,
        folder
    );

    // `x` gives the limit the config file's value again.
    press(&mut app, &client, &[KeyCode::Home, KeyCode::Char('x')]).await;
    assert_eq!(
        client.settings().await.unwrap().settings.limits.download,
        None
    );

    // The theme is kept in the engine, so the next TUI starts with it.
    press(&mut app, &client, &[KeyCode::Char('T')]).await;
    assert_eq!(app.theme, Theme::AMBER);
    let mut next = App::new(Origin::Daemon, Theme::default());
    let request = next.load_settings().unwrap();
    let result = carry_out(&client, &request).await;
    next.finished(&request, result);
    assert_eq!(next.theme, Theme::AMBER);

    engine.shutdown().await;
    serving.abort();
}
