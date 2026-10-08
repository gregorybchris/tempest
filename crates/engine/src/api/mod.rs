//! The engine's HTTP API (D-080), specified in `specs/api.md`.
//!
//! - [`router`]: the routes under `/v1`, over an [`Engine`].
//! - [`server`]: listening on TCP, with HTTPS and the access token for remote access (D-125),
//!   and the file that tells local clients where the API is.
//! - [`client`]: a client for the API, as the CLI and the TUI use it, with [`sse`] decoding its
//!   event stream.

mod access;
pub mod client;
mod error;
mod events;
pub mod server;
pub mod sse;

use std::collections::BTreeMap;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempest_net::{PortMappingStatus, RelayStatus};
use tempest_protocol::{Address, BundleId};

pub use self::access::Access;
pub use self::error::ApiError;
use self::error::{Json, json};
use crate::{AddOptions, Engine, QueueMove, ShareOptions};

pub use tempest_api::events::{DecisionInfo, MissedInfo, RemovedInfo, SnapshotInfo};
pub use tempest_api::requests::*;

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

#[derive(Debug, Deserialize)]
struct ChunksQuery {
    #[serde(default = "default_cells")]
    cells: u32,
}

/// How many cells a chunk map has when the request doesn't say.
const DEFAULT_CELLS: u32 = 1024;

fn default_cells() -> u32 {
    DEFAULT_CELLS
}

#[derive(Debug, Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    delete_files: bool,
}

#[derive(Debug, Deserialize)]
struct UnpublishQuery {
    tracker: String,
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
        .route(&transfer("/files"), get(files))
        .route(&transfer("/peers"), get(peers))
        .route(&transfer("/chunks"), get(chunks))
        .route(&transfer("/start"), post(start))
        .route(&transfer("/force-start"), post(force_start))
        .route(&transfer("/stop"), post(stop))
        .route(&transfer("/recheck"), post(recheck))
        .route(&transfer("/move"), post(move_storage))
        .route(&transfer("/name"), put(rename))
        .route(&transfer("/tags"), put(set_tags))
        .route(&transfer("/category"), put(set_category))
        .route(&transfer("/settings"), put(set_transfer_settings))
        .route(&transfer("/queue"), post(move_in_queue))
        .route(&transfer("/publish"), post(publish).delete(unpublish))
        .route(&format!("{PREFIX}/trackers/sign-in"), post(tracker_sign_in))
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
        .layer(axum::middleware::from_fn_with_state(
            access.clone(),
            access::guard,
        ))
        .merge(session_routes(access))
}

/// The routes for signing in (D-222), which a remote engine answers without a session.
fn session_routes(access: Access) -> Router {
    Router::new()
        .route(
            &format!("{PREFIX}/session"),
            get(session_info).post(sign_in).delete(sign_out),
        )
        .layer(axum::middleware::from_fn_with_state(
            access.clone(),
            access::session_guard,
        ))
        .with_state(access)
}

async fn session_info(State(access): State<Access>, headers: HeaderMap) -> Result {
    ok(&match &access {
        Access::Local => SessionInfo {
            required: false,
            signed_in: true,
        },
        Access::Token(token) => SessionInfo {
            required: true,
            signed_in: access::is_authorized(&headers, token, access::unix_now()),
        },
    })
}

async fn sign_in(State(access): State<Access>, Json(request): Json<SignInRequest>) -> Result {
    let Access::Token(token) = &access else {
        return ok(&SessionInfo {
            required: false,
            signed_in: true,
        });
    };
    if !access::token_matches(&request.token, token) {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED.as_u16(),
            "unauthorized",
            "that isn't the engine's access token",
        ));
    }
    let info = SessionInfo {
        required: true,
        signed_in: true,
    };
    let mut response = json(StatusCode::OK, &info);
    let cookie = access::session_cookie(token, access::unix_now());
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    Ok(response)
}

async fn sign_out() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let cookie = access::clearing_cookie();
    response.headers_mut().insert(header::SET_COOKIE, cookie);
    response
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
    let relay = engine.relay();
    let (state, relay_errors) = match &relay {
        RelayStatus::Off => ("off", Vec::new()),
        RelayStatus::Connecting => ("connecting", Vec::new()),
        RelayStatus::Registered { .. } => ("registered", Vec::new()),
        RelayStatus::Unavailable { failures } => (
            "unavailable",
            failures.iter().map(ToString::to_string).collect(),
        ),
    };
    let mapping = engine.port_mapping();
    let (mapping_state, port_mapping_errors) = match &mapping {
        PortMappingStatus::Off => ("off", Vec::new()),
        PortMappingStatus::Mapping => ("mapping", Vec::new()),
        PortMappingStatus::Mapped { .. } => ("mapped", Vec::new()),
        PortMappingStatus::Unavailable { failures } => (
            "unavailable",
            failures.iter().map(ToString::to_string).collect(),
        ),
    };
    ok(&EngineInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        peer_id: engine.peer_id().to_string(),
        addresses: engine.addresses().iter().map(display).collect(),
        relay: state.to_owned(),
        relay_address: relay.relay_address().as_ref().map(display),
        relay_errors,
        port_mapping: mapping_state.to_owned(),
        mapped_address: mapping.address().as_ref().map(display),
        port_mapping_errors,
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
        trackers,
        created,
    } = request;
    let info = match (link, path) {
        (Some(link), None) => {
            if name.is_some() || !comment.is_empty() || created.is_some() {
                return Err(ApiError::invalid(
                    "name, comment and created describe a new bundle; rename a download instead",
                ));
            }
            let options = AddOptions {
                save_folder,
                category,
                tags,
                stopped,
                trackers,
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
                trackers,
                created,
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

async fn files(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.files(bundle_id(&id)?).await?)
}

async fn peers(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.peers(bundle_id(&id)?).await?)
}

async fn chunks(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    query: std::result::Result<Query<ChunksQuery>, axum::extract::rejection::QueryRejection>,
) -> Result {
    let Query(query) = query.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    ok(&engine.chunk_map(bundle_id(&id)?, query.cells).await?)
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

async fn recheck(State(engine): State<Engine>, Path(id): Path<String>) -> Result {
    ok(&engine.recheck(bundle_id(&id)?).await?)
}

async fn move_storage(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<MoveRequest>,
) -> Result {
    ok(&engine
        .move_storage(bundle_id(&id)?, &request.save_folder)
        .await?)
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

async fn publish(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    Json(request): Json<PublishRequest>,
) -> Result {
    let invite = request.invite.as_deref();
    ok(&engine
        .publish(bundle_id(&id)?, &request.tracker, &request.listing, invite)
        .await?)
}

async fn unpublish(
    State(engine): State<Engine>,
    Path(id): Path<String>,
    query: std::result::Result<Query<UnpublishQuery>, axum::extract::rejection::QueryRejection>,
) -> Result {
    let Query(query) = query.map_err(|rejection| ApiError::invalid(rejection.body_text()))?;
    ok(&engine.unpublish(bundle_id(&id)?, &query.tracker).await?)
}

async fn tracker_sign_in(
    State(engine): State<Engine>,
    Json(request): Json<TrackerSignInRequest>,
) -> Result {
    let invite = request.invite.as_deref();
    ok(&engine
        .sign_in(&request.tracker, &request.code, invite)
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
    ApiError::new(
        StatusCode::NOT_FOUND.as_u16(),
        "not_found",
        "no such endpoint",
    )
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED.as_u16(),
        "method_not_allowed",
        "the endpoint doesn't take this method",
    )
}
