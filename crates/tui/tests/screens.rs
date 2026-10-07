//! Snapshots of the TUI's screens, drawn into a test terminal. `cargo insta review` updates them.

use std::collections::BTreeSet;
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempest_engine::api::DecisionInfo;
use tempest_engine::api::client::ApiEvent;
use tempest_engine::inspect::{CellInfo, ChunkMapInfo, FileInfo, PeerInfo};
use tempest_engine::{Kind, Status, TransferInfo};
use tempest_protocol::Hash;
use tempest_tui::app::{App, Look, Origin, Reply, Request};
use tempest_tui::chunkmap;
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
            have: 207_640,
            chunks: 256_348,
            downloaded: 3_400_000_000,
            uploaded: 408_000_000,
            tags: ["music".to_owned()].into(),
            category: Some("music".to_owned()),
            content_dir: Some(PathBuf::from("/home/ada/Downloads/ensemble-2026.tar")),
            link: "tempest:7f3a9c2be1d04a5f8e6b7c9d0a1b2c3d4e5f60718293a4b5c6d7e8f9a0b1c2d3?n=ensemble-2026.tar".to_owned(),
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
            tags: ["live".to_owned(), "music".to_owned()].into(),
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

fn press_code(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn with_categories(app: &mut App) {
    let categories = vec!["films".to_owned(), "music".to_owned()];
    app.finished(&Request::Categories, Ok(Reply::Categories(categories)));
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

#[test]
fn the_add_modal() {
    let mut app = app();
    press(&mut app, "a");
    with_categories(&mut app);
    app.paste("tempest:7f3a9c2be1d04a5f8e6b7c9d0a1b2c3d4e5f60718293a4b5c6d7e8f9?n=ensemble");
    press_code(&mut app, KeyCode::Tab);
    press_code(&mut app, KeyCode::Tab);
    press_code(&mut app, KeyCode::Right);
    insta::assert_snapshot!(draw(&app, 80, 24).backend());
    // A refused form says why.
    press_code(&mut app, KeyCode::Up);
    press(&mut app, "media");
    press_code(&mut app, KeyCode::Enter);
    insta::assert_snapshot!("add_refused", draw(&app, 80, 24).backend());
}

#[test]
fn the_remove_confirmation() {
    let mut app = app();
    press(&mut app, "xd");
    insta::assert_snapshot!(draw(&app, 80, 24).backend());
}

#[test]
fn text_modals() {
    let mut app = app();
    press(&mut app, "r");
    insta::assert_snapshot!("rename", draw(&app, 80, 12).backend());
    press_code(&mut app, KeyCode::Esc);
    press(&mut app, "t");
    insta::assert_snapshot!("tags", draw(&app, 80, 12).backend());
    press_code(&mut app, KeyCode::Esc);
    press(&mut app, "m");
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    press(&mut app, "media");
    press_code(&mut app, KeyCode::Enter);
    insta::assert_snapshot!("move_refused", draw(&app, 80, 12).backend());
}

#[test]
fn pickers() {
    let mut app = app();
    press(&mut app, "C");
    insta::assert_snapshot!("category_loading", draw(&app, 80, 14).backend());
    with_categories(&mut app);
    press(&mut app, "j");
    insta::assert_snapshot!("category", draw(&app, 80, 14).backend());
    press_code(&mut app, KeyCode::Esc);
    press(&mut app, "f");
    insta::assert_snapshot!("status", draw(&app, 80, 24).backend());
    press_code(&mut app, KeyCode::Esc);
    press(&mut app, "s");
    insta::assert_snapshot!("sort", draw(&app, 80, 24).backend());
}

#[test]
fn the_command_palette() {
    let mut app = app();
    press(&mut app, ":");
    insta::assert_snapshot!("palette", draw(&app, 80, 24).backend());
    press(&mut app, "queue");
    press_code(&mut app, KeyCode::Down);
    insta::assert_snapshot!("palette_filtered", draw(&app, 80, 24).backend());
}

#[test]
fn a_filtered_and_sorted_list() {
    let mut app = app();
    press(&mut app, "/#music");
    insta::assert_snapshot!("filter_prompt", draw(&app, 120, 12).backend());
    press_code(&mut app, KeyCode::Enter);
    // Sort by size, smallest first.
    press(&mut app, "sjj");
    press_code(&mut app, KeyCode::Enter);
    press(&mut app, "S");
    insta::assert_snapshot!("filtered_and_sorted", draw(&app, 120, 12).backend());
    press(&mut app, "/zzz");
    press_code(&mut app, KeyCode::Enter);
    insta::assert_snapshot!("nothing_matches", draw(&app, 80, 12).backend());
}

#[test]
fn notices_replace_the_hints() {
    let mut app = app();
    let request = Request::Categories;
    app.finished(&request, Err("cannot reach the engine".to_owned()));
    insta::assert_snapshot!(draw(&app, 80, 8).backend());
}

/// Answers the fetch the app just started for the first transfer.
fn answer(app: &mut App, look: Look, reply: Reply) {
    app.finished(&Request::Look(Hash::new([0; 32]), look), Ok(reply));
}

fn file(path: &str, size: u64, progress: f64) -> FileInfo {
    let chunks = size.div_ceil(16_384) as u32;
    FileInfo {
        path: path.to_owned(),
        size,
        chunks,
        have: (f64::from(chunks) * progress) as u32,
        progress,
    }
}

fn peer(seed: char, address: &str, relayed: bool, rates: (u64, u64)) -> PeerInfo {
    PeerInfo {
        peer_id: seed.to_string().repeat(64),
        address: address.to_owned(),
        path: if relayed { "relayed" } else { "direct" }.to_owned(),
        direction: "dialed".to_owned(),
        open: true,
        progress: Some(1.0),
        we_want: true,
        they_want: relayed,
        serving_us: rates.0 > 0,
        we_serve: rates.1 > 0,
        snubbed: false,
        download_rate: rates.0,
        upload_rate: rates.1,
        downloaded: 0,
        uploaded: 0,
        rtt_ms: Some(30),
    }
}

#[test]
fn the_detail_opens_below_the_list_with_its_tabs() {
    let mut app = app();
    press_code(&mut app, KeyCode::Enter);
    insta::assert_snapshot!("detail_summary", draw(&app, 80, 24).backend());

    press(&mut app, "l");
    insta::assert_snapshot!("detail_files_loading", draw(&app, 80, 24).backend());
    let files = vec![
        file("ensemble-2026/disc-1.flac", 2_100_000_000, 1.0),
        file("ensemble-2026/disc-2.flac", 2_050_000_000, 0.62),
        file(
            "ensemble-2026/booklet with a rather long name.pdf",
            48_000_000,
            0.0,
        ),
        file("ensemble-2026/cover.jpg", 2_000_000, 1.0),
    ];
    answer(&mut app, Look::Files, Reply::Files(files));
    insta::assert_snapshot!("detail_files", draw(&app, 80, 24).backend());

    press(&mut app, "l");
    let peers = vec![
        peer('a', "203.0.113.7:4777", false, (6_100_000, 0)),
        peer('b', "198.51.100.23:51234", true, (2_400_000, 800_000)),
        peer('c', "[2001:db8::42]:4777", false, (1_300_000, 400_000)),
    ];
    answer(&mut app, Look::Peers, Reply::Peers(peers));
    insta::assert_snapshot!("detail_peers", draw(&app, 80, 24).backend());

    press(&mut app, "l");
    let id = Hash::new([0; 32]);
    let events = [
        (0.0, "started", "Started with 207,640 of 256,348 chunks."),
        (0.4, "connected", "Connected to aaaaaaaa directly."),
        (
            0.5,
            "requested",
            "Asked aaaaaaaa for chunks 207,640 to 207,703: the rarest it has.",
        ),
        (
            1.2,
            "serving",
            "Serving bbbbbbbb: it sends to us the fastest.",
        ),
        (64.3, "endgame", "Asking every peer for the last 12 chunks."),
    ];
    for (time, kind, text) in events {
        app.apply(ApiEvent::Decision(DecisionInfo {
            id,
            time,
            kind: kind.to_owned(),
            reason: None,
            text: text.to_owned(),
        }));
    }
    app.apply(ApiEvent::DecisionsMissed(tempest_engine::api::MissedInfo {
        id,
        count: 3,
    }));
    insta::assert_snapshot!("detail_decisions", draw(&app, 80, 24).backend());

    // Decisions scroll back from the newest.
    press(&mut app, "kkk");
    insta::assert_snapshot!("detail_decisions_scrolled", draw(&app, 80, 14).backend());

    // esc goes back one level.
    press_code(&mut app, KeyCode::Esc);
    insta::assert_snapshot!("detail_closed", draw(&app, 80, 12).backend());
}

#[test]
fn the_chunk_map_fills_the_screen() {
    let mut app = app();
    press_code(&mut app, KeyCode::Enter);
    press(&mut app, "c");
    insta::assert_snapshot!("chunks_loading", draw(&app, 80, 12).backend());

    let cells = chunkmap::cells_for_screen(80, 24);
    let map = ChunkMapInfo {
        chunks: 256_348,
        running: true,
        cells: (0..cells)
            .map(|index| match index * 10 / cells {
                0..=5 => CellInfo::from([89, 0, 0, 3]),
                6 => CellInfo::from([40, 9, 40, 2]),
                7 => CellInfo::from([0, 0, 89, index % 5]),
                _ => CellInfo::from([0, 0, 89, 0]),
            })
            .collect(),
    };
    answer(&mut app, Look::Chunks(cells), Reply::Chunks(map));
    let terminal = draw(&app, 80, 24);
    insta::assert_snapshot!("chunks", terminal.backend());
    // Stored cells are lit in the download color, those being downloaded in the hot one.
    let buffer = terminal.backend().buffer();
    let theme = Theme::default();
    assert_eq!(buffer[(0, 5)].fg, theme.down);
    assert_eq!(buffer[(0, 5)].bg, theme.down);
    let in_flight = chunkmap::place(cells as usize * 65 / 100, 80);
    let cell = &buffer[(in_flight.0, 5 + (in_flight.1 / 2) as u16)];
    assert!(cell.fg == theme.hot || cell.bg == theme.hot);

    // A transfer that isn't running has no rarity to show.
    let stopped = ChunkMapInfo {
        chunks: 10,
        running: false,
        cells: (0..10)
            .map(|n| CellInfo::from([u32::from(n < 4), 0, u32::from(n >= 4), 0]))
            .collect(),
    };
    app.tick();
    answer(&mut app, Look::Chunks(cells), Reply::Chunks(stopped));
    insta::assert_snapshot!("chunks_stopped", draw(&app, 80, 10).backend());

    press_code(&mut app, KeyCode::Esc);
    assert_eq!(app.screen(), tempest_tui::app::Screen::Detail);
}

#[test]
fn the_rates_chart_fills_the_screen() {
    let mut app = app();
    press_code(&mut app, KeyCode::Enter);
    press(&mut app, "g");
    insta::assert_snapshot!("rates_empty", draw(&app, 80, 12).backend());
    for second in 0..120u64 {
        let wave = (second % 40).abs_diff(20);
        let mut busy = transfers().remove(0);
        busy.download_rate = 4_000_000 + wave * 300_000;
        busy.upload_rate = 500_000 + (second % 7) * 100_000;
        app.apply(ApiEvent::Transfer(Box::new(busy)));
        app.tick();
    }
    insta::assert_snapshot!("rates", draw(&app, 80, 24).backend());
    press(&mut app, "c");
    assert_eq!(app.screen(), tempest_tui::app::Screen::Chunks);
}

#[test]
fn help_in_the_detail_lists_its_keys() {
    let mut app = app();
    press_code(&mut app, KeyCode::Enter);
    press(&mut app, "?");
    insta::assert_snapshot!(draw(&app, 80, 24).backend());
}
