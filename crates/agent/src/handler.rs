//! HTTP handlers. Mutations run off the async executor; reads use SQLite snapshots.
use crate::runtime::{self, Runtime};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::Arc;
use ttcore::{api::*, model::Vm};

pub struct AgentShared {
    pub runtime: Arc<tokio::sync::Mutex<Runtime>>,
    pub db_path: String,
    pub info: AgentInfo,
    pub image_dir: String,
}
pub type AppState = Arc<AgentShared>;

type Reply<T> = (StatusCode, Json<ApiResp<T>>);
fn failure<T>(e: impl std::fmt::Display) -> Reply<T> {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiResp::err(e.to_string())),
    )
}

async fn read_snapshot(state: AppState) -> Result<(AgentInfo, Vec<Vm>), String> {
    tokio::task::spawn_blocking(move || {
        let vms = runtime::read_vms(&state.db_path).map_err(|e| e.to_string())?;
        let mut info = state.info.clone();
        info.resource = runtime::resources_for(&info.resource, &vms);
        Ok((info, vms))
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn read_images(
    state: AppState,
) -> Result<(Vec<String>, std::collections::BTreeMap<String, u32>), String> {
    tokio::task::spawn_blocking(move || {
        let store = ttcore::storage::create_store(state.info.storage);
        let images = store
            .list_images(&state.image_dir)
            .map_err(|e| e.to_string())?;
        let sizes = ttcore::storage::image_sizes(store.as_ref(), &state.image_dir, &images);
        Ok((images, sizes))
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn get_info(State(state): State<AppState>) -> impl IntoResponse {
    let (images, sizes) = match read_images(state.clone()).await {
        Ok(images) => images,
        Err(e) => {
            eprintln!("[agent] image catalog unavailable: {e}");
            (vec![], Default::default())
        }
    };
    let snapshot = tokio::task::spawn_blocking(move || {
        let (vms, corrupt) =
            runtime::read_recovery_snapshot(&state.db_path).map_err(|e| e.to_string())?;
        let mut info = state.info.clone();
        info.resource = runtime::resources_for(&info.resource, &vms);
        if corrupt {
            info.resource.cpu_used = info.resource.cpu_total;
            info.resource.mem_used = info.resource.mem_total;
            info.resource.disk_used = info.resource.disk_total;
            info.warnings.push(
                "unreadable VM records retained; new admission disabled; inspect agent logs".into(),
            );
        }
        info.images = images;
        info.image_sizes = sizes;
        info.vms = Some(vms);
        Ok::<_, String>(info)
    })
    .await;
    match snapshot {
        Ok(Ok(info)) => (StatusCode::OK, Json(ApiResp::success(info))),
        Ok(Err(e)) => failure(e),
        Err(e) => failure(e),
    }
}

pub async fn list_images(State(state): State<AppState>) -> impl IntoResponse {
    match read_images(state).await {
        Ok((images, _)) => (StatusCode::OK, Json(ApiResp::success(images))),
        Err(e) => failure(e),
    }
}
pub async fn list_vms(State(state): State<AppState>) -> impl IntoResponse {
    match read_snapshot(state).await {
        Ok((_, vms)) => (StatusCode::OK, Json(ApiResp::success(vms))),
        Err(e) => failure(e),
    }
}
pub async fn get_vm(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let query_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        runtime::read_vm(&state.db_path, &query_id).map_err(|e| e.to_string())
    })
    .await;
    match result {
        Ok(Ok(vm)) => match vm {
            Some(vm) => (StatusCode::OK, Json(ApiResp::success(vm))),
            None => (
                StatusCode::NOT_FOUND,
                Json(ApiResp::err(format!("VM not found: {id}"))),
            ),
        },
        Ok(Err(e)) => failure(e),
        Err(e) => failure(e),
    }
}

async fn mutate<T: Send + 'static>(
    state: AppState,
    f: impl FnOnce(&mut Runtime) -> ruc::Result<T> + Send + 'static,
) -> Result<T, String> {
    // Once accepted, continue even if the caller disconnects or times out.
    tokio::spawn(async move {
        let mut guard = state.runtime.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || f(&mut guard).map_err(|e| e.to_string()))
            .await
            .map_err(|e| e.to_string())?
    })
    .await
    .map_err(|e| e.to_string())?
}
pub async fn create_vm(
    State(state): State<AppState>,
    Json(req): Json<CreateVmReq>,
) -> impl IntoResponse {
    match mutate(state, move |rt| rt.create_vm(&req)).await {
        Ok(vm) => (
            StatusCode::CREATED,
            Json(ApiResp::success(CreateVmResp { vm })),
        ),
        Err(e) => failure(e),
    }
}
pub async fn destroy_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    action(mutate(state, move |rt| rt.destroy_vm(&id)).await)
}
pub async fn stop_vm(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    action(mutate(state, move |rt| rt.stop_vm(&id)).await)
}
pub async fn start_vm(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    action(mutate(state, move |rt| rt.start_vm(&id)).await)
}
fn action(result: Result<(), String>) -> Reply<()> {
    match result {
        Ok(()) => (StatusCode::OK, Json(ApiRespEmpty::ok())),
        Err(e) => failure(e),
    }
}
