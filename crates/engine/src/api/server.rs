//! Serving the API on TCP (D-080, D-125, D-126).
//!
//! Without remote access the API is plain HTTP on a loopback address. With it, the API is
//! HTTPS, with the configured certificate or a self-signed one kept in the data directory, and
//! every request needs the access token kept there too.
//!
//! While it serves, the API's URL is written to a file in the data directory, so local clients
//! such as the CLI find it whatever port it is on.

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use super::{Access, router};
use crate::Engine;
use crate::config::ApiSettings;

pub use tempest_api::discovery::{DISCOVERY_FILE, Discovery, TOKEN_FILE};

/// The self-signed certificate made for remote access when none is configured.
pub const CERT_FILE: &str = "api-cert.pem";
/// The self-signed certificate's private key.
pub const KEY_FILE: &str = "api-key.pem";

/// How long a TLS handshake may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why the API could not start.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The port could not be bound.
    #[error("cannot bind TCP port {port}: {source}")]
    Bind {
        /// The port.
        port: u16,
        /// What went wrong.
        source: io::Error,
    },
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
    /// The certificate or key is unusable.
    #[error("TLS: {0}")]
    Tls(String),
    /// Serving failed.
    #[error("serving the API: {0}")]
    Serve(io::Error),
}

/// The API's listening socket, ready to serve. Clients can find it from the moment it is
/// bound: connections wait until it serves.
pub struct ApiServer {
    listener: Listener,
    access: Access,
    url: String,
    fingerprint: Option<String>,
    ui: Option<axum::Router>,
    _recorded: Recorded,
}

/// The discovery file, removed when dropped if it still records this server.
struct Recorded {
    path: PathBuf,
    discovery: Discovery,
}

impl Recorded {
    fn write(data_dir: &Path, discovery: Discovery) -> Result<Self, ServeError> {
        let path = data_dir.join(DISCOVERY_FILE);
        let bytes = serde_json::to_vec_pretty(&discovery).expect("discovery serializes");
        write_atomically(&path, &bytes)?;
        Ok(Self { path, discovery })
    }
}

impl Drop for Recorded {
    fn drop(&mut self) {
        let data_dir = self.path.parent().expect("in the data directory");
        if Discovery::read(data_dir).is_ok_and(|found| found.as_ref() == Some(&self.discovery)) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

enum Listener {
    Plain(TcpListener),
    Tls(TlsListener),
}

impl ApiServer {
    /// Binds the API's port as `settings` say, preparing HTTPS and the access token for remote
    /// access, both kept in `data_dir`.
    pub async fn bind(settings: &ApiSettings, data_dir: &Path) -> Result<Self, ServeError> {
        let port = settings.port;
        let tcp = TcpListener::bind(SocketAddr::new(settings.bind, port))
            .await
            .map_err(|source| ServeError::Bind { port, source })?;
        let local = tcp
            .local_addr()
            .map_err(|source| ServeError::Bind { port, source })?;
        let host = match local.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            ip => ip,
        };
        let address = SocketAddr::new(host, local.port());
        let record = |url: &str, cert: Option<PathBuf>| {
            let discovery = Discovery {
                url: url.to_owned(),
                cert,
                pid: std::process::id(),
            };
            Recorded::write(data_dir, discovery)
        };
        if !settings.remote {
            let url = format!("http://{address}");
            return Ok(Self {
                listener: Listener::Plain(tcp),
                access: Access::Local,
                _recorded: record(&url, None)?,
                url,
                fingerprint: None,
                ui: None,
            });
        }
        let token = load_or_create_token(&data_dir.join(TOKEN_FILE))?;
        let (cert_path, key_path) = match (&settings.tls_cert, &settings.tls_key) {
            (Some(cert), Some(key)) => (cert.clone(), key.clone()),
            _ => {
                let paths = (data_dir.join(CERT_FILE), data_dir.join(KEY_FILE));
                if !paths.0.exists() || !paths.1.exists() {
                    create_self_signed(&paths.0, &paths.1, settings.bind)?;
                }
                paths
            }
        };
        let (chain, key) = load_cert(&cert_path, &key_path)?;
        let fingerprint = fingerprint(&chain[0]);
        let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_no_client_auth().with_single_cert(chain, key))
        .map_err(|error| ServeError::Tls(error.to_string()))?;
        let url = format!("https://{address}");
        Ok(Self {
            listener: Listener::Tls(TlsListener::new(
                tcp,
                local,
                TlsAcceptor::from(Arc::new(tls)),
            )),
            access: Access::Token(token.into()),
            _recorded: record(&url, Some(cert_path))?,
            url,
            fingerprint: Some(fingerprint),
            ui: None,
        })
    }

    /// The API's base URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The SHA-256 fingerprint of the HTTPS certificate, for remote clients to check.
    pub fn fingerprint(&self) -> Option<&str> {
        self.fingerprint.as_deref()
    }

    /// Who may use the API.
    pub fn access(&self) -> &Access {
        &self.access
    }

    /// Serves `ui`'s routes beside the API's, without the API's access checks. Every path it
    /// doesn't route stays the API's.
    pub fn with_ui(mut self, ui: axum::Router) -> Self {
        self.ui = Some(ui);
        self
    }

    /// Serves `engine` until `shutdown` completes and every response has ended. Event streams
    /// end when the engine stops, so `shutdown` should stop it.
    pub async fn serve(
        self,
        engine: Engine,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError> {
        let mut app = router(engine, self.access.clone());
        if let Some(ui) = self.ui {
            app = app.merge(ui);
        }
        let result = match self.listener {
            Listener::Plain(listener) => {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
            }
            Listener::Tls(listener) => {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
            }
        };
        result.map_err(ServeError::Serve)
    }
}

/// Accepts TCP connections and completes their TLS handshakes in tasks of their own, so a slow
/// handshake holds up nobody else.
struct TlsListener {
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
    acceptor: tokio::task::JoinHandle<()>,
}

impl TlsListener {
    fn new(tcp: TcpListener, local: SocketAddr, tls: TlsAcceptor) -> Self {
        let (sender, ready) = mpsc::channel(64);
        let acceptor = tokio::spawn(async move {
            loop {
                let Ok((stream, peer)) = tcp.accept().await else {
                    // Such as too many open files: give it a moment.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let (tls, sender) = (tls.clone(), sender.clone());
                tokio::spawn(async move {
                    let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, tls.accept(stream));
                    if let Ok(Ok(stream)) = handshake.await {
                        let _ = sender.send((stream, peer)).await;
                    }
                });
            }
        });
        Self {
            ready,
            local,
            acceptor,
        }
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.acceptor.abort();
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The acceptor runs as long as the listener does.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Reads the access token, or makes one: 32 random bytes in hex.
fn load_or_create_token(path: &Path) -> Result<String, ServeError> {
    let io_error = |source| ServeError::Io {
        path: path.to_owned(),
        source,
    };
    match std::fs::read_to_string(path) {
        Ok(token) if !token.trim().is_empty() => return Ok(token.trim().to_owned()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|e| io_error(io::Error::other(e.to_string())))?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    write_private(path, token.as_bytes())?;
    Ok(token)
}

/// Makes a self-signed certificate for this machine's names and `bind`.
fn create_self_signed(cert: &Path, key: &Path, bind: IpAddr) -> Result<(), ServeError> {
    let mut names = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    if !bind.is_unspecified() && !bind.is_loopback() {
        names.push(bind.to_string());
    }
    let made = rcgen::generate_simple_self_signed(names)
        .map_err(|error| ServeError::Tls(error.to_string()))?;
    write_private(key, made.signing_key.serialize_pem().as_bytes())?;
    write_atomically(cert, made.cert.pem().as_bytes())
}

fn load_cert(
    cert: &Path,
    key: &Path,
) -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>), ServeError> {
    let invalid = |path: &Path, error: rustls::pki_types::pem::Error| {
        ServeError::Tls(format!("{}: {error}", path.display()))
    };
    let chain = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|error| invalid(cert, error))?;
    if chain.is_empty() {
        return Err(ServeError::Tls(format!(
            "{}: no certificate",
            cert.display()
        )));
    }
    let key_der = PrivateKeyDer::from_pem_file(key).map_err(|error| invalid(key, error))?;
    Ok((chain, key_der))
}

/// A certificate's SHA-256 fingerprint, as colon-separated hex.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    let digest = Sha256::digest(cert.as_ref());
    let hex: Vec<_> = digest.iter().map(|byte| format!("{byte:02X}")).collect();
    hex.join(":")
}

/// Writes a file only its owner can read.
fn write_private(path: &Path, contents: &[u8]) -> Result<(), ServeError> {
    use std::io::Write;

    let io_error = |source| ServeError::Io {
        path: path.to_owned(),
        source,
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(io_error)?;
    file.write_all(contents).map_err(io_error)
}

/// Writes a file whole or not at all, so a reader never sees half of it.
fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), ServeError> {
    let io_error = |source| ServeError::Io {
        path: path.to_owned(),
        source,
    };
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, contents).map_err(io_error)?;
    std::fs::rename(&temporary, path).map_err(io_error)
}
