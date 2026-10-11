//! A client for the engine's API, as the CLI and the TUI use it, with [`sse`] decoding its
//! event stream. Built with the `client` feature.

pub mod sse;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tempest_http_client::{CertificateDer, PemObject, Trust};
use tempest_protocol::BundleId;
use tempest_tracker_api::account::Account;
use tempest_tracker_api::listing::Listing;

use self::sse::{SseDecoder, SseEvent};
use crate::discovery::{DISCOVERY_FILE, Discovery, TOKEN_FILE};
use crate::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use crate::requests::{
    AddRequest, Category, CategoryRequest, EngineInfo, MoveRequest, NameRequest, PREFIX,
    PublishRequest, QueuePlace, QueueRequest, RevealInfo, TagsRequest, TrackerSignInRequest,
};
use crate::settings::SettingsInfo;
use crate::{ApiError, ApiEvent, TransferInfo};

/// Why a request to the API failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The API could not be reached, or the exchange broke off.
    #[error("cannot reach the daemon at {url}: {source}")]
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
    #[error("unexpected answer from the daemon: {0}")]
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
        let trust = cert.map_or(Trust::Platform, Trust::Pinned);
        let builder = tempest_http_client::client_builder(trust)
            .map_err(|error| ClientError::Unexpected(error.to_string()))?;
        let http = builder
            .connect_timeout(Duration::from_secs(5))
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
        let discovery_path = data_dir.join(DISCOVERY_FILE);
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

    /// Shows a transfer's files in the file manager of the engine's machine.
    pub async fn reveal(&self, id: BundleId) -> Result<RevealInfo, ClientError> {
        self.post(&format!("/transfers/{id}/reveal")).await
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

    /// Lists a transfer on a tracker (`POST /v1/transfers/{id}/publish`).
    pub async fn publish(
        &self,
        id: BundleId,
        request: &PublishRequest,
    ) -> Result<Listing, ClientError> {
        let path = format!("/transfers/{id}/publish");
        self.send(Method::POST, &path, Some(request)).await
    }

    /// Unpublishes a transfer from a tracker.
    pub async fn unpublish(
        &self,
        id: BundleId,
        tracker: &str,
    ) -> Result<TransferInfo, ClientError> {
        let path = format!(
            "/transfers/{id}/publish?tracker={}",
            encode_segment(tracker)
        );
        self.send(Method::DELETE, &path, None::<&()>).await
    }

    /// Signs the engine's key in to a tracker for whoever asked for the request's code.
    pub async fn tracker_sign_in(
        &self,
        request: &TrackerSignInRequest,
    ) -> Result<Account, ClientError> {
        self.send(Method::POST, "/trackers/sign-in", Some(request))
            .await
    }

    /// Replaces a transfer's own settings, by dotted key.
    pub async fn set_transfer_settings(
        &self,
        id: BundleId,
        settings: &BTreeMap<String, Value>,
    ) -> Result<TransferInfo, ClientError> {
        let path = format!("/transfers/{id}/settings");
        self.send(Method::PUT, &path, Some(settings)).await
    }

    /// The engine's settings.
    pub async fn settings(&self) -> Result<SettingsInfo, ClientError> {
        self.get("/settings").await
    }

    /// Changes settings by dotted key.
    pub async fn change_settings(
        &self,
        changes: &BTreeMap<String, Value>,
    ) -> Result<SettingsInfo, ClientError> {
        self.send(Method::PATCH, "/settings", Some(changes)).await
    }

    /// Undoes the change made through the API to the setting at the dotted `key`, so the
    /// config file's value applies again.
    pub async fn reset_setting(&self, key: &str) -> Result<SettingsInfo, ClientError> {
        self.send(Method::DELETE, &format!("/settings/{key}"), None::<&()>)
            .await
    }

    /// Every category.
    pub async fn categories(&self) -> Result<BTreeMap<String, Category>, ClientError> {
        self.get("/categories").await
    }

    /// Adds the category `name`, or changes its save folder.
    pub async fn set_category_folder(
        &self,
        name: &str,
        save_folder: PathBuf,
    ) -> Result<Category, ClientError> {
        let path = format!("/categories/{}", encode_segment(name));
        self.send(Method::PUT, &path, Some(&Category { save_folder }))
            .await
    }

    /// Removes the category `name`; transfers in it have none.
    pub async fn remove_category(&self, name: &str) -> Result<(), ClientError> {
        let path = format!("/categories/{}", encode_segment(name));
        self.exchange(Method::DELETE, &path, None::<&()>).await?;
        Ok(())
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

/// `text` as one segment of a URL's path: every byte but unreserved ones percent-encoded.
fn encode_segment(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
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
                let decoded = ApiEvent::decode(&event.name, &event.data).map_err(|error| {
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
