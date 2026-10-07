//! Snapshots of the TUI's screens, drawn into a test terminal. `cargo insta review` updates them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::client::ApiEvent;
use tempest_engine::{Kind, Status, TransferInfo};
use tempest_protocol::Hash;
use tempest_tui::app::{App, Origin};
use tempest_tui::model::Connection;
use tempest_tui::theme::Theme;
use tempest_tui::ui;

fn transfer(seed: u8, name: &str, status: Status) -> TransferInfo {
    TransferInfo {
        id: Hash::new([seed; 32]),
        name: name.to_owned(),
        kind: Kind::Download,
        status,
        size: None,
        have: 0,
        chunks: 0,
        progress: 0.0,
        eta: None,
        ratio: None,
        downloaded: 0,
        uploaded: 0,
        seeding_time: 0,
        download_rate: 0,
        upload_rate: 0,
        connections: 0,
        relayed: 0,
        peers: 0,
        added: 0,
        completed: None,
        queue_position: u32::from(seed),
        forced: false,
        tags: BTreeSet::new(),
        category: None,
        save_folder: PathBuf::from("/home/ada/Downloads"),
        content_dir: None,
        link: String::new(),
        settings: Default::default(),
    }
}

/// The transfers of the layout sketch in `specs/interfaces.md`, and a few more states.
fn transfers() -> Vec<TransferInfo> {
    vec![
        TransferInfo {
            size: Some(4_200_000_000),
            progress: 0.81,
            eta: Some(180),
            ratio: Some(0.12),
            download_rate: 9_800_000,
            upload_rate: 1_200_000,
            connections: 7,
            relayed: 2,
            peers: 12,
            ..transfer(0, "ensemble-2026.tar", Status::Downloading)
        },
        TransferInfo {
            kind: Kind::Share,
            size: Some(812_000_000),
            progress: 1.0,
            ratio: Some(2.4),
            upload_rate: 400_000,
            connections: 3,
            peers: 3,
            ..transfer(1, "field-recordings", Status::Sharing)
        },
        TransferInfo {
            size: Some(1_900_000_000),
            progress: 0.31,
            peers: 4,
            ..transfer(
                2,
                "corpus-v3.bundle",
                Status::Stalled {
                    reason: "Found 4 peers, but couldn't connect to any.".to_owned(),
                },
            )
        },
        TransferInfo {
            size: Some(3_100_000_000),
            ..transfer(
                3,
                "debian-13-netinst-and-a-very-long-name.iso",
                Status::Queued,
            )
        },
        TransferInfo {
            size: Some(52_000_000),
            progress: 0.45,
            ..transfer(4, "old-photos", Status::Stopped)
        },
        TransferInfo {
            size: Some(97_000),
            progress: 0.02,
            ..transfer(
                5,
                "notes.txt",
                Status::Errored {
                    reason: "The disk is full.".to_owned(),
                },
            )
        },
    ]
}

fn app() -> App {
    let mut app = App::new(Origin::Daemon, Theme::default());
    app.model.connection = Connection::Live;
    app.apply(ApiEvent::Snapshot(transfers()));
    app
}

fn press(app: &mut App, keys: &str) {
    for c in keys.chars() {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn draw(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui::draw(frame, app)).unwrap();
    terminal
}

#[test]
fn transfers_at_80_columns() {
    insta::assert_snapshot!(draw(&app(), 80, 24).backend());
}

#[test]
fn transfers_on_a_wide_terminal() {
    let mut app = app();
    press(&mut app, "jj");
    insta::assert_snapshot!(draw(&app, 120, 16).backend());
}

#[test]
fn transfers_on_a_narrow_terminal_drop_columns() {
    let mut app = app();
    press(&mut app, "G");
    insta::assert_snapshot!(draw(&app, 56, 14).backend());
}

#[test]
fn transfers_while_connecting_and_when_empty() {
    let connecting = App::new(Origin::Embedded, Theme::default());
    insta::assert_snapshot!("connecting", draw(&connecting, 80, 10).backend());
    let mut empty = connecting;
    empty.model.connection = Connection::Live;
    empty.apply(ApiEvent::Snapshot(vec![]));
    insta::assert_snapshot!("empty", draw(&empty, 80, 10).backend());
}

#[test]
fn transfers_when_the_engine_is_lost() {
    let mut app = app();
    app.model.connection = Connection::Lost {
        error: "cannot reach the engine at http://127.0.0.1:4776".to_owned(),
    };
    insta::assert_snapshot!(draw(&app, 80, 12).backend());
}

#[test]
fn help_lists_the_keys_of_the_screen() {
    let mut app = app();
    press(&mut app, "?");
    insta::assert_snapshot!(draw(&app, 80, 24).backend());
}

#[test]
fn other_areas_keep_the_navigation() {
    let mut app = app();
    press(&mut app, "2");
    insta::assert_snapshot!(draw(&app, 80, 8).backend());
}

#[test]
fn every_theme_colors_the_screen() {
    let mut app = app();
    for theme in Theme::ALL {
        app.theme = theme;
        let terminal = draw(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        // The background fills the screen, and the active area is lit in the download color.
        assert_eq!(buffer[(79, 23)].bg, theme.bg, "{}", theme.name);
        assert_eq!(buffer[(2, 0)].bg, theme.down, "{}", theme.name);
        let header: String = (0..80).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(header.contains(&format!("[{}]", theme.name)), "{header}");
        press(&mut app, "T");
    }
}
