//! The API served on loopback: plain HTTP for local access, and HTTPS with the access token for
//! remote access, found by clients through the data directory.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use tempest_engine::api::client::{ApiClient, ClientError};
use tempest_engine::api::server::{ApiServer, CERT_FILE, DISCOVERY_FILE, Discovery, TOKEN_FILE};
use tempest_engine::api::{Access, AddRequest};
use tempest_engine::config::{ApiSettings, LanSettings, PeerSettings};
use tempest_engine::{Engine, EngineConfig};
use tempest_protocol::{Hash, Link};
use tokio::sync::oneshot;

fn config(dir: &Path, api: ApiSettings) -> EngineConfig {
    EngineConfig {
        peers: PeerSettings {
            port: 0,
            bind: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        },
        lan: LanSettings {
            enabled: false,
            ..LanSettings::default()
        },
        api,
        data_dir: Some(dir.to_owned()),
        save_folder: Some(dir.join("downloads")),
        ..EngineConfig::default()
    }
}

/// Serves an engine's API until the returned sender is used or dropped.
async fn serve(config: EngineConfig) -> (Engine, ApiServerTask) {
    let data_dir = config.data_dir.clone().unwrap();
    let engine = Engine::open(config.clone()).await.unwrap();
    let server = ApiServer::bind(&config.api, &data_dir).await.unwrap();
    let url = server.url().to_owned();
    let (stop, stopped) = oneshot::channel::<()>();
    let shutdown_engine = engine.clone();
    let task = tokio::spawn(server.serve(engine.clone(), async move {
        let _ = stopped.await;
        shutdown_engine.shutdown().await;
    }));
    // It is serving once it says where.
    while Discovery::read(&data_dir).unwrap().is_none() {
        tokio::task::yield_now().await;
    }
    (engine, ApiServerTask { stop, task, url })
}

struct ApiServerTask {
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), tempest_engine::api::server::ServeError>>,
    url: String,
}

impl ApiServerTask {
    async fn stop(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap().unwrap();
    }
}

fn add(seed: u8) -> AddRequest {
    AddRequest {
        link: Some(Link::new(Hash::new([seed; 32])).to_string()),
        stopped: true,
        ..AddRequest::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_clients_find_the_api_over_plain_http() {
    let dir = tempfile::tempdir().unwrap();
    let api = ApiSettings {
        port: 0,
        ..ApiSettings::default()
    };
    let (_engine, server) = serve(config(dir.path(), api)).await;
    assert!(
        server.url.starts_with("http://127.0.0.1:"),
        "{}",
        server.url
    );

    let client = ApiClient::for_data_dir(dir.path()).unwrap().expect("found");
    assert_eq!(client.url(), server.url);
    let added = client.add(&add(1)).await.unwrap();
    assert_eq!(client.list().await.unwrap(), std::slice::from_ref(&added));
    let refused = client.add(&add(1)).await.unwrap_err();
    assert!(
        matches!(&refused, ClientError::Refused(e) if e.error == "already_added" && e.status == 409),
        "{refused:?}"
    );

    server.stop().await;
    assert!(
        !dir.path().join(DISCOVERY_FILE).exists(),
        "the discovery file goes when the API stops"
    );
    assert!(ApiClient::for_data_dir(dir.path()).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_access_is_https_with_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let api = ApiSettings {
        port: 0,
        remote: true,
        ..ApiSettings::default()
    };
    let config = config(dir.path(), api.clone());
    let server = ApiServer::bind(&config.api, dir.path()).await.unwrap();
    assert!(matches!(server.access(), Access::Token(_)));
    assert!(server.fingerprint().is_some_and(|f| f.len() == 95));
    drop(server);

    let (_engine, server) = serve(config).await;
    assert!(
        server.url.starts_with("https://127.0.0.1:"),
        "{}",
        server.url
    );
    let token = std::fs::read_to_string(dir.path().join(TOKEN_FILE)).unwrap();
    assert_eq!(token.len(), 64, "the same token as before");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(TOKEN_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "only its owner can read the token");
    }

    // A local client finds the certificate and the token in the data directory.
    let client = ApiClient::for_data_dir(dir.path()).unwrap().expect("found");
    client.add(&add(1)).await.unwrap();
    assert_eq!(client.list().await.unwrap().len(), 1);

    // Without the token, or with another, requests are refused.
    let cert = CertificateDer::from_pem_file(dir.path().join(CERT_FILE)).unwrap();
    for token in [None, Some("wrong".to_owned())] {
        let stranger = ApiClient::new(&server.url, token, Some(cert.clone())).unwrap();
        match stranger.list().await {
            Err(ClientError::Refused(error)) => {
                assert_eq!((error.status, error.error.as_str()), (401, "unauthorized"));
            }
            other => panic!("{other:?}"),
        }
    }
    // Nor is plain HTTP served.
    let plain_url = server.url.replace("https://", "http://");
    let plain = ApiClient::new(&plain_url, Some(token), None).unwrap();
    assert!(plain.list().await.is_err());

    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clients_follow_and_resume_the_event_stream() {
    use tempest_engine::api::client::ApiEvent;

    let dir = tempfile::tempdir().unwrap();
    let api = ApiSettings {
        port: 0,
        ..ApiSettings::default()
    };
    let (_engine, server) = serve(config(dir.path(), api)).await;
    let client = ApiClient::for_data_dir(dir.path()).unwrap().expect("found");

    let mut stream = client.events(None).await.unwrap();
    assert_eq!(
        stream.next().await.unwrap(),
        Some((ApiEvent::Snapshot(vec![]), None))
    );
    let added = client.add(&add(1)).await.unwrap();
    let (event, number) = stream.next().await.unwrap().unwrap();
    assert_eq!(event, ApiEvent::Transfer(Box::new(added.clone())));
    let number = number.expect("transfer events are numbered");
    drop(stream);

    // Resuming after the last event seen sends only what came after it.
    client.remove(added.id, false).await.unwrap();
    let mut resumed = client.events(Some(number)).await.unwrap();
    let (event, next) = resumed.next().await.unwrap().unwrap();
    assert!(
        matches!(&event, ApiEvent::Removed(removed) if removed.id == added.id),
        "{event:?}"
    );
    assert_eq!(next, Some(number + 1));

    // Resuming after an event this engine never sent starts over.
    let mut reset = client.events(Some(number + 1000)).await.unwrap();
    assert_eq!(
        reset.next().await.unwrap(),
        Some((ApiEvent::Reset(vec![]), None))
    );

    server.stop().await;
}
