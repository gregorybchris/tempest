//! The engine's HTTP API (D-080), specified in `specs/api.md`.
//!
//! - [`router`]: the routes under `/v1`, over an [`Engine`].
//! - [`server`]: listening on TCP, with HTTPS and the access token for remote access (D-125),
//!   and the file that tells local clients where the API is.
//! - [`client`]: a client for the API, as the CLI uses it.

mod access;
pub mod client;
mod error;
mod events;
pub mod server;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempest_protocol::{Address, BundleId};

pub use self::access::Access;
pub use self::error::ApiError;
use self::error::{Json, json};
use crate::{AddOptions, Engine, QueueMove, ShareOptions};

/// The prefix of every route: the API's version.
pub const PREFIX: &str = "/v1";

/// `GET /v1/engine`: what the engine is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    /// The engine's version.
    pub version: String,
    /// This peer's ID.
    pub peer_id: String,
    /// The addresses this peer advertises.
    pub addresses: Vec<String>,
}

/// `POST /v1/transfers`: a download from a link, or a share of a local file or folder.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AddRequest {
    /// A `tempest:` link to download. Exactly one of `link` and `path` is given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// An absolute path to share.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// A share's bundle name. Defaults to the file or folder's name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A share's description.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// Where a download's bundle folder is made. Defaults to the category's folder, then the
    /// engine's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_folder: Option<PathBuf>,
    /// The transfer's category.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// The transfer's tags.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<String>,
    /// Add a download stopped.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
}

/// `PUT /v1/transfers/{id}/name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NameRequest {
    /// The name to show, or `null` for the bundle's own.
    pub name: Option<String>,
}

/// `PUT /v1/transfers/{id}/tags`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagsRequest {
    /// Every tag.
    pub tags: BTreeSet<String>,
}

/// `PUT /v1/transfers/{id}/category`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryRequest {
    /// The category, or `null` for none.
    pub category: Option<String>,
}

/// `POST /v1/transfers/{id}/queue`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueueRequest {
    /// `top`, `up`, `down`, `bottom`, or a place counting from 0.
    pub to: QueuePlace,
}

/// Where to move a transfer in the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum QueuePlace {
    /// A place, counting from 0.
    Place(u32),
    /// `top`, `up`, `down` or `bottom`.
    Step(QueueStep),
}

/// A move relative to the queue or the transfer's place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueStep {
    /// First.
    Top,
    /// One place earlier.
    Up,
    /// One place later.
    Down,
    /// Last.
    Bottom,
}

impl From<QueuePlace> for QueueMove {
    fn from(place: QueuePlace) -> Self {
        match place {
            QueuePlace::Place(place) => Self::To(place),
            QueuePlace::Step(QueueStep::Top) => Self::Top,
            QueuePlace::Step(QueueStep::Up) => Self::Up,
            QueuePlace::Step(QueueStep::Down) => Self::Down,
            QueuePlace::Step(QueueStep::Bottom) => Self::Bottom,
        }
    }
}

/// A category, as `GET /v1/categories` lists it and `PUT /v1/categories/{name}` sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Category {
    /// Where downloads added in the category go.
    pub save_folder: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    delete_files: bool,
}

/// The API's routes over `engine`, refusing what `access` doesn't allow.
pub fn router(engine: Engine, access: Access) -> Router {
    let transfer = |path: &str| format!("{PREFIX}/transfers/{{id}}{path}");
    Router::new()
        .route(&format!("{PREFIX}/engine"), get(engine_info))
        .route(
            &format!("{PREFIX}/transfers"),
            get(list_transfers).post(add_transfer),
        )
        .route(&transfer(""), get(get_transfer).delete(remove_transfer))
        .route(&transfer("/start"), post(start))
        .route(&transfer("/force-start"), post(force_start))
        .route(&transfer("/stop"), post(stop))
        .route(&transfer("/name"), put(rename))
        .route(&transfer("/tags"), put(set_tags))
        .route(&transfer("/category"), put(set_category))
        .route(&transfer("/settings"), put(set_transfer_settings))
        .route(&transfer("/queue"), post(move_in_queue))
        .route(
            &format!("{PREFIX}/settings"),
            get(settings).patch(change_settings),
        )
        .route(
            &format!("{PREFIX}/settings/{{key}}"),
            axum::routing::delete(reset_setting),
        )
        .route(&format!("{PREFIX}/categories"), get(categories))
        .route(
            &format!("{PREFIX}/categories/{{name}}"),
            put(set_category_folder).delete(remove_category),
        )
        .route(&format!("{PREFIX}/events"), get(events::events))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(engine)
        .layer(axum::middleware::from_fn_with_state(access, access::guard))
}

type Result<T = Response> = std::result::Result<T, ApiError>;

fn ok(body: &impl Serialize) -> Result {
    Ok(json(StatusCode::OK, body))
}

fn bundle_id(id: &str) -> Result<BundleId> {
    id.parse().map_err(|_| {
        ApiError::invalid("a transfer ID is its bundle ID: 64 lowercase hexadecimal characters")
    })
}

async fn engine_info(State(engine): State<Engine>) -> Result {
    let display = |address: &Address| address.to_string();
    ok(&EngineInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        peer_id: engine.peer_id().to_string(),
        addresses: engine.addresses().iter().map(display).collect(),
    })
}

async fn list_transfers(State(engine): State<Engine>) -> Result {
    ok(&engine.list().await)
}

async fn add_transfer(State(engine): State<Engine>, Json(request): Json<AddRequest>) -> Result {
    let AddRequest {
        link,
        path,
        name,
        comment,
        save_folder,
        category,
        tags,
        stopped,
    } = request;
    let info = match (link, path) {
        (Some(link), None) => {
            if name.is_some() || !comment.is_empty() {
                return Err(ApiError::invalid(
                    "name and comment describe a new bundle; rename a download instead",
                ));
            }
            let options = AddOptions {
                save_folder,
                category,
                tags,
                stopped,
            };
            engine.add_link(&link, options).await?
        }
        (None, Some(path)) => {
            if save_folder.is_some() || stopped {
                return Err(ApiError::invalid(
                    "save_folder and stopped apply to downloads; a share stays where it is",
                ));
            }
            if !path.is_absolute() {
                return Err(ApiError::invalid("path must be absolute"));
            }
            let options = ShareOptions {
                name,
                comment,
                category,
                tags,
            };
            engine.add_path(&path, options).await?
        }
        _ => return Err(ApiError::invalid("give exactly one of link and path")),
    };
    Ok(json(StatusCode::CREATED, &info))
}

async fn get_transfer(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    let id = bundle_id(&id)?;
    match engine.get(id).await {
        Some(info) => ok(&info),
        None => Err(crate::EngineError::NotFound(id).into()),
    }
}

async fn remove_transfer(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    query: std::result::Result<Query<RemoveQuery>, axum::extract::rejection::QueryRejection>,
) -> Result {
    let Query(query) = query.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    engine.remove(bundle_id(&id)?, query.delete_files).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn start(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.start(bundle_id(&id)?).await?)
}

async fn force_start(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.force_start(bundle_id(&id)?).await?)
}

async fn stop(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.stop(bundle_id(&id)?).await?)
}

async fn rename(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<NameRequest>,
) -> Result {
    ok(&engine.rename(bundle_id(&id)?, request.name).await?)
}

async fn set_tags(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<TagsRequest>,
) -> Result {
    ok(&engine.set_tags(bundle_id(&id)?, request.tags).await?)
}

async fn set_category(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<CategoryRequest>,
) -> Result {
    ok(&engine
        .set_category(bundle_id(&id)?, request.category)
        .await?)
}

async fn set_transfer_settings(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(settings): Json<BTreeMap<String, Value>>,
) -> Result {
    ok(&engine
        .set_transfer_settings(bundle_id(&id)?, settings)
        .await?)
}

async fn move_in_queue(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<QueueRequest>,
) -> Result {
    ok(&engine
        .move_in_queue(bundle_id(&id)?, request.to.into())
        .await?)
}

async fn settings(State(engine): State<Engine>) -> Result {
    ok(&engine.settings().await?)
}

async fn change_settings(
    State(engine): State<Engine>,
    Json(changes): Json<BTreeMap<String, Value>>,
) -> Result {
    ok(&engine.change_settings(changes).await?)
}

async fn reset_setting(State(engine): State<Engine>, Path(key): Path<String>) -> Result {
    ok(&engine.reset_setting(&key).await?)
}

async fn categories(State(engine): State<Engine>) -> Result {
    let categories: BTreeMap<_, _> = engine
        .categories()
        .await
        .into_iter()
        .map(|(name, save_folder)| (name, Category { save_folder }))
        .collect();
    ok(&categories)
}

async fn set_category_folder(
    State(engine): State<Engine>,
    Path(name): Path<String>,
    Json(category): Json<Category>,
) -> Result {
    engine
        .set_category_folder(&name, &category.save_folder)
        .await?;
    ok(&category)
}

async fn remove_category(State(engine): State<Engine>, Path(name): Path<String>) -> Result {
    engine.remove_category(&name).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such endpoint")
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method_not_allowed",
        "the endpoint doesn't take this method",
    )
}
