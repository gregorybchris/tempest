//! The engine API's routes that the web UI uses (`specs/api.md`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use tempest_api::inspect::{ChunkMapInfo, FileInfo, PeerInfo};
use tempest_api::requests::{
    AddRequest, Category, CategoryRequest, MoveRequest, NameRequest, QueuePlace, QueueRequest,
    TagsRequest,
};
use tempest_api::settings::SettingsInfo;
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

/// Shares a file or folder on the engine's machine. The engine answers once the bundle is
/// built.
pub async fn share(request: AddRequest) -> Result<TransferInfo> {
    send(method::POST, &format!("{PREFIX}/transfers"), Some(&request)).await
}

/// Replaces a transfer's tags.
pub async fn set_tags(id: BundleId, tags: BTreeSet<String>) -> Result<TransferInfo> {
    let body = TagsRequest { tags };
    send(method::PUT, &transfer(&id, "/tags"), Some(&body)).await
}

/// Sets a transfer's category, or clears it with `None`.
pub async fn set_category(id: BundleId, category: Option<String>) -> Result<TransferInfo> {
    let body = CategoryRequest { category };
    send(method::PUT, &transfer(&id, "/category"), Some(&body)).await
}

/// Moves a transfer's files into `folder`, an absolute path on the engine's machine.
pub async fn move_to(id: BundleId, folder: String) -> Result<TransferInfo> {
    let body = MoveRequest {
        save_folder: folder.into(),
    };
    send(method::POST, &transfer(&id, "/move"), Some(&body)).await
}

/// Replaces a transfer's own settings, by dotted key.
pub async fn set_transfer_settings(
    id: BundleId,
    settings: BTreeMap<String, Value>,
) -> Result<TransferInfo> {
    send(method::PUT, &transfer(&id, "/settings"), Some(&settings)).await
}

/// Moves a transfer in the queue.
pub async fn queue(id: BundleId, to: QueuePlace) -> Result<Vec<TransferInfo>> {
    send(
        method::POST,
        &transfer(&id, "/queue"),
        Some(&QueueRequest { to }),
    )
    .await
}

/// The settings in effect, which were changed through the API, and more.
pub async fn settings() -> Result<SettingsInfo> {
    get(&format!("{PREFIX}/settings")).await
}

/// Changes settings, given as an object by dotted key: all of them, or none.
pub async fn change_settings(changes: &Value) -> Result<SettingsInfo> {
    send(method::PATCH, &format!("{PREFIX}/settings"), Some(changes)).await
}

/// Gives setting `key` the config file's value again.
pub async fn reset_setting(key: &str) -> Result<SettingsInfo> {
    let path = format!("{PREFIX}/settings/{}", segment(key));
    send(method::DELETE, &path, None::<&()>).await
}

/// Each category's save folder, by name.
pub async fn categories() -> Result<BTreeMap<String, Category>> {
    get(&format!("{PREFIX}/categories")).await
}

/// Adds category `name`, or changes its folder.
pub async fn set_category_folder(name: &str, folder: String) -> Result<Category> {
    let path = format!("{PREFIX}/categories/{}", segment(name));
    let body = Category {
        save_folder: folder.into(),
    };
    send(method::PUT, &path, Some(&body)).await
}

/// Removes category `name`.
pub async fn remove_category(name: &str) -> Result<()> {
    let path = format!("{PREFIX}/categories/{}", segment(name));
    send(method::DELETE, &path, None::<&()>).await
}

/// `text` as one segment of a path.
fn segment(text: &str) -> String {
    String::from(js_sys::encode_uri_component(text))
}
