//! The TUI's model and screen following a real engine through its API and event stream.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempest_engine::api::AddRequest;
use tempest_engine::api::client::ApiClient;
use tempest_engine::api::server::ApiServer;
use tempest_engine::config::{ApiSettings, LanSettings, PeerSettings};
use tempest_engine::{Engine, EngineConfig};
use tempest_tui::app::{App, Origin};
use tempest_tui::theme::Theme;
use tempest_tui::ui;

#[tokio::test(flavor = "multi_thread")]
async fn the_list_shows_what_the_engine_shares() {
    let dir = tempfile::tempdir().unwrap();
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
        data_dir: Some(dir.path().to_owned()),
        ..EngineConfig::default()
    };
    let engine = Engine::open(config.clone()).await.unwrap();
    let server = ApiServer::bind(&config.api, dir.path()).await.unwrap();
    let serving = tokio::spawn(server.serve(engine.clone(), std::future::pending()));
    let client = ApiClient::for_data_dir(dir.path()).unwrap().unwrap();

    let mut app = App::new(Origin::Daemon, Theme::default());
    let mut events = client.events(None).await.unwrap();
    let file = dir.path().join("field-notes.txt");
    std::fs::write(&file, b"notes").unwrap();
    client
        .add(&AddRequest {
            path: Some(file),
            ..AddRequest::default()
        })
        .await
        .unwrap();

    // Follow events until the share is complete and running.
    let shared = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (event, _) = events.next().await.unwrap().unwrap();
            app.apply(event);
            if app
                .selected_transfer()
                .is_some_and(|t| t.status.name() == "sharing")
            {
                break;
            }
        }
    })
    .await;
    assert!(shared.is_ok(), "the share never showed as sharing");

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
