//! A client for the engine's API.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Method, RequestBuilder};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tempest_protocol::BundleId;

use super::server::{Discovery, TOKEN_FILE};
use super::sse::{SseDecoder, SseEvent};
use super::{
    AddRequest, ApiError, Category, CategoryRequest, DecisionInfo, EngineInfo, MissedInfo,
    MoveRequest, NameRequest, PREFIX, QueuePlace, QueueRequest, RemovedInfo, SnapshotInfo,
    TagsRequest,
};
use crate::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use crate::{SettingsView, TransferInfo};

/// Why a request to the API failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The API could not be reached, or the exchange broke off.
    #[error("cannot reach the engine at {url}: {source}")]
    Unreachable {
        /// The API's URL.
        url: String,
        /// What went wrong.
        source: reqwest::Error,
    },
    /// The API refused the request.
    #[error("{0}")]
    Refused(ApiError),
    /// The API's answer is not what the client expected.
    #[error("unexpected answer from the engine: {0}")]
    Unexpected(String),
    /// The client's settings could not be read.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
}

/// A client for one engine's API. Clones share connections.
#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl ApiClient {
    /// A client for the API at `url`, sending `token` if given. Over HTTPS, the API must
    /// present exactly `cert` if given, or else a certificate the platform trusts.
    pub fn new(
        url: &str,
        token: Option<String>,
        cert: Option<CertificateDer<'static>>,
    ) -> Result<Self, ClientError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|error| ClientError::Unexpected(error.to_string()))?;
        let tls = match cert {
            Some(cert) => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(Pinned { cert, provider })),
            None => {
                use rustls_platform_verifier::BuilderVerifierExt;
                builder
                    .with_platform_verifier()
                    .map_err(|error| ClientError::Unexpected(error.to_string()))?
            }
        }
        .with_no_client_auth();
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(Duration::from_secs(5))
            .user_agent(concat!("tempest/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|source| ClientError::Unreachable {
                url: url.to_owned(),
                source,
            })?;
        Ok(Self {
            http,
            base: url.trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// A client for the engine using `data_dir`, from what it recorded there, or `None` if no
    /// engine has recorded where its API is.
    pub fn for_data_dir(data_dir: &Path) -> Result<Option<Self>, ClientError> {
        let io_error = |path: PathBuf| move |source| ClientError::Io { path, source };
        let discovery_path = data_dir.join(super::server::DISCOVERY_FILE);
        let Some(discovery) = Discovery::read(data_dir).map_err(io_error(discovery_path))? else {
            return Ok(None);
        };
        let Some(cert_path) = discovery.cert else {
            return Self::new(&discovery.url, None, None).map(Some);
        };
        let cert = CertificateDer::from_pem_file(&cert_path).map_err(|error| ClientError::Io {
            path: cert_path.clone(),
            source: std::io::Error::other(error.to_string()),
        })?;
        let token_path = data_dir.join(TOKEN_FILE);
        let token = std::fs::read_to_string(&token_path).map_err(io_error(token_path))?;
        Self::new(&discovery.url, Some(token.trim().to_owned()), Some(cert)).map(Some)
    }

    /// The API's base URL.
    pub fn url(&self) -> &str {
        &self.base
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T, ClientError> {
        let response = self.exchange(method, path, body).await?;
        serde_json::from_slice(&response).map_err(|e| ClientError::Unexpected(e.to_string()))
    }

    async fn exchange(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<Vec<u8>, ClientError> {
        let mut request: RequestBuilder = self
            .http
            .request(method, format!("{}{PREFIX}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(body).expect("requests serialize"));
        }
        let unreachable = |source| ClientError::Unreachable {
            url: self.base.clone(),
            source,
        };
        let response = request.send().await.map_err(unreachable)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(unreachable)?;
        if status.is_success() {
            return Ok(bytes.to_vec());
        }
        Err(refusal(status, &bytes))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.send(Method::GET, path, None::<&()>).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.send(Method::POST, path, None::<&()>).await
    }

    /// What the engine is.
    pub async fn engine(&self) -> Result<EngineInfo, ClientError> {
        self.get("/engine").await
    }

    /// Every transfer, in queue order.
    pub async fn list(&self) -> Result<Vec<TransferInfo>, ClientError> {
        self.get("/transfers").await
    }

    /// One transfer.
    pub async fn transfer(&self, id: BundleId) -> Result<TransferInfo, ClientError> {
        self.get(&format!("/transfers/{id}")).await
    }

    /// A transfer's files and how much of each is stored.
    pub async fn files(&self, id: BundleId) -> Result<Vec<FileInfo>, ClientError> {
        self.get(&format!("/transfers/{id}/files")).await
    }

    /// A running transfer's connections.
    pub async fn peers(&self, id: BundleId) -> Result<Vec<PeerInfo>, ClientError> {
        self.get(&format!("/transfers/{id}/peers")).await
    }

    /// A transfer's chunks in at most `cells` cells.
    pub async fn chunk_map(&self, id: BundleId, cells: u32) -> Result<ChunkMapInfo, ClientError> {
        self.get(&format!("/transfers/{id}/chunks?cells={cells}"))
            .await
    }

    /// Adds a download or a share.
    pub async fn add(&self, request: &AddRequest) -> Result<TransferInfo, ClientError> {
        self.send(Method::POST, "/transfers", Some(request)).await
    }

    /// Starts a transfer through the queue.
    pub async fn start(&self, id: BundleId) -> Result<TransferInfo, ClientError> {
        self.post(&format!("/transfers/{id}/start")).await
    }

    /// Starts a transfer at once, bypassing the queue.
    pub async fn force_start(&self, id: BundleId) -> Result<TransferInfo, ClientError> {
        self.post(&format!("/transfers/{id}/force-start")).await
    }

    /// Stops a transfer.
    pub async fn stop(&self, id: BundleId) -> Result<TransferInfo, ClientError> {
        self.post(&format!("/transfers/{id}/stop")).await
    }

    /// Checks a transfer's data on disk again from scratch.
    pub async fn recheck(&self, id: BundleId) -> Result<TransferInfo, ClientError> {
        self.post(&format!("/transfers/{id}/recheck")).await
    }

    /// Moves a transfer's files into `save_folder`.
    pub async fn move_storage(
        &self,
        id: BundleId,
        save_folder: PathBuf,
    ) -> Result<TransferInfo, ClientError> {
        let path = format!("/transfers/{id}/move");
        self.send(Method::POST, &path, Some(&MoveRequest { save_folder }))
            .await
    }

    /// Removes a transfer, deleting its files too if `delete_files` is set.
    pub async fn remove(&self, id: BundleId, delete_files: bool) -> Result<(), ClientError> {
        let path = format!("/transfers/{id}?delete_files={delete_files}");
        self.exchange(Method::DELETE, &path, None::<&()>).await?;
        Ok(())
    }

    /// Shows a transfer under `name`, or its bundle's own name with `None`.
    pub async fn rename(
        &self,
        id: BundleId,
        name: Option<String>,
    ) -> Result<TransferInfo, ClientError> {
        let body = NameRequest { name };
        let path = format!("/transfers/{id}/name");
        self.send(Method::PUT, &path, Some(&body)).await
    }

    /// Replaces a transfer's tags.
    pub async fn set_tags(
        &self,
        id: BundleId,
        tags: BTreeSet<String>,
    ) -> Result<TransferInfo, ClientError> {
        let path = format!("/transfers/{id}/tags");
        self.send(Method::PUT, &path, Some(&TagsRequest { tags }))
            .await
    }

    /// Puts a transfer in a category, or none.
    pub async fn set_category(
        &self,
        id: BundleId,
        category: Option<String>,
    ) -> Result<TransferInfo, ClientError> {
        let path = format!("/transfers/{id}/category");
        self.send(Method::PUT, &path, Some(&CategoryRequest { category }))
            .await
    }

    /// Moves a transfer in the queue, returning every transfer in the new order.
    pub async fn move_in_queue(
        &self,
        id: BundleId,
        to: QueuePlace,
    ) -> Result<Vec<TransferInfo>, ClientError> {
        let path = format!("/transfers/{id}/queue");
        self.send(Method::POST, &path, Some(&QueueRequest { to }))
            .await
    }

    /// The engine's settings.
    pub async fn settings(&self) -> Result<SettingsView, ClientError> {
        self.get("/settings").await
    }

    /// Changes settings by dotted key.
    pub async fn change_settings(
        &self,
        changes: &BTreeMap<String, Value>,
    ) -> Result<SettingsView, ClientError> {
        self.send(Method::PATCH, "/settings", Some(changes)).await
    }

    /// Every category.
    pub async fn categories(&self) -> Result<BTreeMap<String, Category>, ClientError> {
        self.get("/categories").await
    }

    /// Opens the event stream, resuming after the event numbered `after` if given.
    pub async fn events(&self, after: Option<u64>) -> Result<EventStream, ClientError> {
        let mut request = self
            .http
            .get(format!("{}{PREFIX}/events", self.base))
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(after) = after {
            request = request.header("last-event-id", after.to_string());
        }
        let response = request
            .send()
            .await
            .map_err(|source| ClientError::Unreachable {
                url: self.base.clone(),
                source,
            })?;
        let status = response.status();
        if !status.is_success() {
            let bytes = response.bytes().await.unwrap_or_default();
            return Err(refusal(status, &bytes));
        }
        Ok(EventStream {
            response,
            decoder: SseDecoder::new(),
            pending: Default::default(),
            url: self.base.clone(),
        })
    }
}

/// The error an unsuccessful answer with `status` and `body` stands for.
fn refusal(status: reqwest::StatusCode, body: &[u8]) -> ClientError {
    match serde_json::from_slice::<ApiError>(body) {
        Ok(error) => ClientError::Refused(ApiError {
            status: status.as_u16(),
            ..error
        }),
        Err(_) => ClientError::Unexpected(format!("status {status}")),
    }
}

/// An event from the engine's stream (`specs/api.md`).
#[derive(Debug, Clone, PartialEq)]
pub enum ApiEvent {
    /// Every transfer, first on a new stream.
    Snapshot(Vec<TransferInfo>),
    /// Every transfer, because events the client asked for are gone: replace the whole view.
    Reset(Vec<TransferInfo>),
    /// A transfer that was added or changed.
    Transfer(Box<TransferInfo>),
    /// A transfer that was removed.
    Removed(RemovedInfo),
    /// A session's decision event.
    Decision(DecisionInfo),
    /// Decision events dropped because they came too fast.
    DecisionsMissed(MissedInfo),
}

impl ApiEvent {
    /// The event that `event` carries, or `None` for a name this client doesn't know, which a
    /// later engine may send.
    pub fn decode(event: &SseEvent) -> Result<Option<Self>, serde_json::Error> {
        let data = event.data.as_str();
        Ok(Some(match event.name.as_str() {
            "snapshot" => Self::Snapshot(serde_json::from_str::<SnapshotInfo>(data)?.transfers),
            "reset" => Self::Reset(serde_json::from_str::<SnapshotInfo>(data)?.transfers),
            "transfer" => Self::Transfer(serde_json::from_str(data)?),
            "removed" => Self::Removed(serde_json::from_str(data)?),
            "decision" => Self::Decision(serde_json::from_str(data)?),
            "decisions_missed" => Self::DecisionsMissed(serde_json::from_str(data)?),
            _ => return Ok(None),
        }))
    }
}

/// The engine's event stream, as [`ApiClient::events`] opens it.
#[derive(Debug)]
pub struct EventStream {
    response: reqwest::Response,
    decoder: SseDecoder,
    pending: VecDeque<SseEvent>,
    url: String,
}

impl EventStream {
    /// The next event and its number, if it has one, or `None` once the stream has ended.
    pub async fn next(&mut self) -> Result<Option<(ApiEvent, Option<u64>)>, ClientError> {
        loop {
            while let Some(event) = self.pending.pop_front() {
                let decoded = ApiEvent::decode(&event).map_err(|error| {
                    ClientError::Unexpected(format!("bad {} event: {error}", event.name))
                })?;
                if let Some(decoded) = decoded {
                    let number = event.id.and_then(|id| id.parse().ok());
                    return Ok(Some((decoded, number)));
                }
            }
            let chunk = self
                .response
                .chunk()
                .await
                .map_err(|source| ClientError::Unreachable {
                    url: self.url.clone(),
                    source,
                })?;
            let Some(chunk) = chunk else {
                return Ok(None);
            };
            let events = self
                .decoder
                .feed(&chunk)
                .map_err(|error| ClientError::Unexpected(error.to_string()))?;
            self.pending.extend(events);
        }
    }
}

/// Trusts exactly one certificate, whatever names it has, as a client that read it from the
/// engine's data directory may.
#[derive(Debug)]
struct Pinned {
    cert: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match end_entity.as_ref() == self.cert.as_ref() {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::General(
                "the engine presented a certificate other than the one in its data directory"
                    .to_owned(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, cert, dss, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, cert, dss, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
