//! The engine API's routes that the web UI uses (`specs/api.md`).

use serde::Deserialize;
use serde_json::{Value, json};
use tempest_api::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use tempest_api::requests::{AddRequest, NameRequest};
use tempest_api::ui::UiSettings;
use tempest_api::{PREFIX, TransferInfo};
use tempest_protocol::BundleId;

use crate::request::{RequestError, get, method, send};

type Result<T> = std::result::Result<T, RequestError>;

fn transfer(id: &BundleId, path: &str) -> String {
    format!("{PREFIX}/transfers/{id}{path}")
}

async fn act(id: &BundleId, action: &str) -> Result<TransferInfo> {
    send(method::POST, &transfer(id, action), None::<&()>).await
}

/// Starts a transfer through the queue.
pub async fn start(id: BundleId) -> Result<TransferInfo> {
    act(&id, "/start").await
}

/// Starts a transfer regardless of the queue.
pub async fn force_start(id: BundleId) -> Result<TransferInfo> {
    act(&id, "/force-start").await
}

/// Stops a transfer.
pub async fn stop(id: BundleId) -> Result<TransferInfo> {
    act(&id, "/stop").await
}

/// Checks a transfer's data again.
pub async fn recheck(id: BundleId) -> Result<TransferInfo> {
    act(&id, "/recheck").await
}

/// Removes a transfer, deleting its files if asked.
pub async fn remove(id: BundleId, delete_files: bool) -> Result<()> {
    let path = transfer(&id, &format!("?delete_files={delete_files}"));
    send(method::DELETE, &path, None::<&()>).await
}

/// Renames a transfer, or gives it the bundle's own name again with `None`.
pub async fn rename(id: BundleId, name: Option<String>) -> Result<TransferInfo> {
    send(
        method::PUT,
        &transfer(&id, "/name"),
        Some(&NameRequest { name }),
    )
    .await
}

/// Adds a download from `link`.
pub async fn add_link(link: String, stopped: bool) -> Result<TransferInfo> {
    let request = AddRequest {
        link: Some(link),
        stopped,
        ..AddRequest::default()
    };
    send(method::POST, &format!("{PREFIX}/transfers"), Some(&request)).await
}

/// A transfer's files.
pub async fn files(id: BundleId) -> Result<Vec<FileInfo>> {
    get(&transfer(&id, "/files")).await
}

/// A running transfer's connections.
pub async fn peers(id: BundleId) -> Result<Vec<PeerInfo>> {
    get(&transfer(&id, "/peers")).await
}

/// A transfer's chunk map in at most `cells` cells.
pub async fn chunks(id: BundleId, cells: usize) -> Result<ChunkMapInfo> {
    get(&transfer(&id, &format!("/chunks?cells={cells}"))).await
}

/// The part of `GET /v1/settings` the web UI reads so far: the clients' `ui` group.
#[derive(Debug, Deserialize)]
struct SettingsView {
    settings: Settings,
}

#[derive(Debug, Deserialize)]
struct Settings {
    #[serde(default)]
    ui: UiSettings,
}

/// The clients' shared settings (D-195).
pub async fn ui_settings() -> Result<UiSettings> {
    let view: SettingsView = get(&format!("{PREFIX}/settings")).await?;
    Ok(view.settings.ui)
}

/// Changes one setting, by dotted key.
pub async fn change_setting(key: &str, value: Value) -> Result<()> {
    let _: Value = send(
        method::PATCH,
        &format!("{PREFIX}/settings"),
        Some(&json!({ key: value })),
    )
    .await?;
    Ok(())
}
