//! HTTP handlers. Mutations run off the async executor; reads use SQLite snapshots.
use crate::runtime::{self, Runtime};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use std::sync::{Arc, RwLock};
use ttcore::{api::*, model::Vm};

pub struct AgentShared {
    pub backup_settings: (bool, Option<String>),
    pub runtime: Arc<tokio::sync::Mutex<Runtime>>,
    pub db_path: String,
    pub info: AgentInfo,
    pub image_dir: String,
    pub images: RwLock<ImageCatalog>,
}
pub type AppState = Arc<AgentShared>;

#[derive(Clone)]
pub struct ImageCatalog {
    pub images: Vec<String>,
    pub sizes: std::collections::BTreeMap<String, u32>,
    pub warning: Option<String>,
}

impl Default for ImageCatalog {
    fn default() -> Self {
        Self {
            images: vec![],
            sizes: Default::default(),
            warning: Some("image catalog refresh pending".into()),
        }
    }
}

type Reply<T> = (StatusCode, Json<ApiResp<T>>);

/// Yield the host mutation lock between VM observations so queued lifecycle work progresses.
pub async fn reconcile_once(state: AppState) -> Result<(), String> {
    let path = state.db_path.clone();
    let vms = tokio::task::spawn_blocking(move || {
        runtime::read_recovery_snapshot(&path)
            .map(|(vms, _)| vms)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    for vm in vms {
        let id = vm.id;
        let diagnostic_id = id.clone();
        if let Err(e) = mutate(state.clone(), move |rt| rt.reconcile_vm(&id)).await {
            eprintln!("[agent] reconcile {diagnostic_id}: {e}");
        }
        if let Some(pending) = vm.backup.pending {
            if let Err(e) =
                run_backup(state.clone(), diagnostic_id.clone(), pending.request, false).await
            {
                eprintln!("[agent] backup recovery {diagnostic_id}: {e}");
            }
        } else if !vm.backup.retired.is_empty() {
            cleanup_backup(state.clone(), diagnostic_id).await;
        }
    }
    Ok(())
}
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

/// One background worker refreshes the catalog; HTTP reads never wait for disk tools.
pub async fn refresh_images(state: AppState) {
    let reader = state.clone();
    let mut task = tokio::task::spawn_blocking(move || {
        let state = reader;
        let store = ttcore::storage::create_store(state.info.storage);
        let images = store
            .list_images(&state.image_dir)
            .map_err(|e| e.to_string())?;
        let sizes = ttcore::storage::image_sizes(store.as_ref(), &state.image_dir, &images);
        Ok::<_, String>(ImageCatalog {
            images,
            sizes,
            warning: None,
        })
    });
    let result = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
        Ok(result) => result,
        Err(_) => {
            state
                .images
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .warning = Some("image catalog refresh is slow; serving cached images".into());
            // Keep the single worker until completion: timing out a blocking task
            // does not cancel it and must not start overlapping inspections.
            task.await
        }
    };
    let catalog = result
        .map_err(|e| e.to_string())
        .and_then(|r| r)
        .unwrap_or_else(|e| {
            eprintln!("[agent] image catalog unavailable: {e}");
            ImageCatalog {
                images: vec![],
                sizes: Default::default(),
                warning: Some(format!("image catalog unavailable: {e}")),
            }
        });
    *state.images.write().unwrap_or_else(|e| e.into_inner()) = catalog;
}

pub async fn get_info(State(state): State<AppState>) -> impl IntoResponse {
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
        let catalog = state.images.read().unwrap_or_else(|e| e.into_inner());
        info.images = catalog.images.clone();
        info.image_sizes = catalog.sizes.clone();
        info.warnings.extend(catalog.warning.clone());
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
    let catalog = state.images.read().unwrap_or_else(|e| e.into_inner());
    (
        StatusCode::OK,
        Json(ApiResp::success(catalog.images.clone())),
    )
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
pub async fn resize_vm(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(target): Json<ttcore::model::VmResources>,
) -> impl IntoResponse {
    match mutate(state, move |rt| rt.resize_vm(&id, target)).await {
        Ok(vm) => (StatusCode::OK, Json(ApiResp::success(vm))),
        Err(e) => {
            let status = if e.contains("insufficient resources")
                || e.contains("require a confirmed stopped VM")
                || e.contains("retry the recorded target first")
            {
                StatusCode::CONFLICT
            } else if e.contains("shrinking")
                || e.contains("must be > 0")
                || e.contains("require QEMU or Firecracker")
                || e.contains("overflow")
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(ApiResp::err(e)))
        }
    }
}
fn action(result: Result<(), String>) -> Reply<()> {
    match result {
        Ok(()) => (StatusCode::OK, Json(ApiRespEmpty::ok())),
        Err(e) => failure(e),
    }
}

fn backup_reply(result: Result<ttcore::backup::View, String>) -> axum::response::Response {
    match result {
        Ok(view) => {
            let etag = format!("\"{}\"", view.vm.backup.revision);
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag)],
                Json(ApiResp::success(view)),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::from_u16(ttcore::backup::error_status(&e)).unwrap(),
            Json(ApiResp::<()>::err(e)),
        )
            .into_response(),
    }
}

pub async fn get_backup(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let result = tokio::task::spawn_blocking(move || {
        let vm = runtime::read_vm(&state.db_path, &id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("not found: VM {id}"))?;
        let unsupported_reason = if matches!(
            vm.engine,
            ttcore::model::Engine::Docker | ttcore::model::Engine::Jail
        ) {
            Some("backup unsupported: container and Jail roots are not VM disks".into())
        } else {
            state.backup_settings.1.clone()
        };
        Ok(ttcore::backup::View {
            vm,
            enabled: state.backup_settings.0,
            supported: unsupported_reason.is_none(),
            unsupported_reason,
        })
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    backup_reply(result)
}

pub async fn create_backup(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    backup_request(state, id, headers, ttcore::backup::Action::Create, None).await
}
pub async fn delete_backup(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    backup_request(state, id, headers, ttcore::backup::Action::Delete, None).await
}
pub async fn restore_backup(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ttcore::backup::RestoreBody>,
) -> axum::response::Response {
    backup_request(
        state,
        id,
        headers,
        ttcore::backup::Action::Restore,
        Some(body.generation),
    )
    .await
}

async fn backup_request(
    state: AppState,
    id: String,
    headers: axum::http::HeaderMap,
    action: ttcore::backup::Action,
    generation: Option<String>,
) -> axum::response::Response {
    let request = ttcore::backup::Request::from_headers(
        headers.get("idempotency-key").and_then(|h| h.to_str().ok()),
        headers.get("if-match").and_then(|h| h.to_str().ok()),
        action,
        generation,
    );
    let identity = request.as_ref().ok().cloned();
    let result = match request {
        Err(e) => Err(e),
        Ok(request) => tokio::spawn(run_backup(state.clone(), id.clone(), request, true))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r),
    };
    let failed = result.is_err();
    let mut response = backup_reply(result);
    if failed
        && let Some(request) = identity
        && let Some(sequence) = state.runtime.lock().await.backup_settled(&id, &request)
    {
        response
            .headers_mut()
            .insert("x-tt-backup-settled", request.operation_id.parse().unwrap());
        response.headers_mut().insert(
            "x-tt-backup-settled-sequence",
            sequence.to_string().parse().unwrap(),
        );
    }
    response
}

async fn run_backup(
    state: AppState,
    id: String,
    request: ttcore::backup::Request,
    wait: bool,
) -> Result<ttcore::backup::View, String> {
    use ttcore::backup::Action;
    let started = std::time::Instant::now();
    let setup = loop {
        let mut rt = state.runtime.lock().await;
        let view = rt.backup_view(&id)?;
        if view.vm.backup.check(&request)? {
            return match view
                .vm
                .backup
                .last_result
                .as_ref()
                .and_then(|r| r.error.clone())
            {
                Some(e) => Err(e),
                None => Ok(view),
            };
        }
        match rt.backup_claim(&id, Some(&request)) {
            Ok(setup) => break setup,
            Err(e)
                if wait
                    && e.contains("backup worker is running")
                    && started.elapsed().as_secs() < 300 =>
            {
                drop(rt);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(e) => return Err(e),
        }
    };
    let result = async {
        let probe = request.action == Action::Create && setup.vm.backup.pending.is_none();
        let (context, lock, witness) = tokio::task::spawn_blocking(move || {
            let context = setup.context()?;
            let lock = context.lock()?;
            let witness = context.inspect(&lock, probe)?;
            Ok::<_, String>((context, lock, witness))
        })
        .await
        .map_err(|e| e.to_string())??;
        let vm = state
            .runtime
            .lock()
            .await
            .backup_admit(&id, &request, &witness)?;
        let pending = vm.backup.pending.clone().ok_or("backup intent absent")?;
        let storage_result = tokio::task::spawn_blocking(move || match pending.request.action {
            Action::Create => context.create(&lock, &pending).map(|g| (Some(g), vec![])),
            Action::Restore => {
                let source = vm
                    .backup
                    .current
                    .as_ref()
                    .ok_or("not found: backup absent")?;
                let removed = context.prepare_restore(&lock, source, &vm.backup.retired)?;
                #[cfg(target_os = "linux")]
                if vm.engine == ttcore::model::Engine::Firecracker {
                    ttcore::engine::firecracker::FirecrackerEngine::prepare_disk_restore(&vm)
                        .map_err(|e| e.to_string())?;
                }
                context
                    .restore(&lock, &pending, source)
                    .map(|g| (g, removed))
            }
            Action::Delete => {
                for old in vm.backup.current.iter().chain(&vm.backup.retired) {
                    context.remove(&lock, old)?;
                }
                Ok((None, vec![]))
            }
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
        let failure = storage_result.as_ref().err().cloned();
        let removed = storage_result
            .as_ref()
            .map(|(_, removed)| removed.clone())
            .unwrap_or_default();
        let storage_result = storage_result.map(|(artifact, _)| artifact);
        let view =
            state
                .runtime
                .lock()
                .await
                .backup_finish(&id, &request, &removed, storage_result)?;
        match failure {
            Some(e) => Err(e),
            None => Ok(view),
        }
    }
    .await;
    state.runtime.lock().await.backup_release(&id);
    result
}

async fn cleanup_backup(state: AppState, id: String) {
    let setup = match state.runtime.lock().await.backup_claim(&id, None) {
        Ok(setup) => setup,
        Err(_) => return,
    };
    let result = tokio::task::spawn_blocking(move || {
        let mut removed = vec![];
        let context = setup.context()?;
        let lock = context.lock()?;
        if let Some(current) = &setup.vm.backup.current {
            context.verify(&lock, current)?;
        }
        let mut errors = vec![];
        for old in &setup.vm.backup.retired {
            match context.remove(&lock, old) {
                Ok(()) => removed.push(old.id.clone()),
                Err(e) => errors.push(e),
            }
        }
        Ok::<_, String>((removed, (!errors.is_empty()).then(|| errors.join("; "))))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    let (removed, error) = result.unwrap_or_else(|e| (vec![], Some(e)));
    let mut rt = state.runtime.lock().await;
    if let Err(e) = rt.backup_cleanup_finish(&id, &removed, error) {
        eprintln!("[agent] backup cleanup {id}: {e}");
    }
    rt.backup_release(&id);
}
