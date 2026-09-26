//! HTTP API handlers for the central controller.
//!
//! Handles requests from the CLI and coordinates with host agents.

use crate::db::Db;
use crate::scheduler;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use ttcore::api::*;
use ttcore::model::*;

/// Shared controller state.
pub struct CtlShared {
    pub(crate) db: Mutex<Db>,
    operations: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    revision: AtomicU64,
    /// API key used for controller→agent communication.
    pub api_key: Option<String>,
}

impl CtlShared {
    pub fn new(db: Db, api_key: Option<String>) -> Self {
        Self {
            db: Mutex::new(db),
            operations: Mutex::new(HashMap::new()),
            revision: AtomicU64::new(0),
            api_key,
        }
    }

    /// Lock the DB mutex, recovering from poisoning.
    pub fn lock_db(&self) -> MutexGuard<'_, Db> {
        self.db.lock().unwrap_or_else(|e| {
            eprintln!("[ctl] WARN: db mutex was poisoned, recovering");
            e.into_inner()
        })
    }
}

pub type CtlState = Arc<CtlShared>;

/// Per-environment guards keep retries ordered without blocking the whole fleet.
struct Operation {
    state: CtlState,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        let _db = self.state.lock_db();
        self.state.revision.fetch_add(1, Ordering::SeqCst);
    }
}
impl CtlShared {
    fn operation_lock(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self.operations.lock().unwrap_or_else(|e| e.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(id.to_owned(), Arc::downgrade(&lock));
        lock
    }
    fn busy(&self, id: &str) -> bool {
        self.operation_lock(id).try_lock().is_err()
    }
}
fn begin_operation(state: &CtlState, id: &str) -> Result<Operation, ApiError> {
    let guard = state.operation_lock(id).try_lock_owned().map_err(|_| {
        (
            StatusCode::CONFLICT,
            "environment has an operation in progress; inspect it before retrying".into(),
        )
    })?;
    let _db = state.lock_db();
    state.revision.fetch_add(1, Ordering::SeqCst);
    Ok(Operation {
        state: state.clone(),
        _guard: guard,
    })
}

/// Build a client without silently dropping authentication or following redirects.
pub fn agent_client(api_key: Option<&str>, timeout_secs: u64) -> Result<reqwest::Client, String> {
    let mut builder =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_secs));
    if let Some(key) = api_key {
        ttcore::auth::parse_api_key(key)?;
        let mut headers = reqwest::header::HeaderMap::new();
        let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| "invalid API key header".to_owned())?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
        builder = builder.default_headers(headers);
    }
    builder
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

// ── Host Management ─────────────────────────────────────────────────

/// POST /api/hosts — register a new host by its agent address.
pub async fn register_host(
    State(db): State<CtlState>,
    Json(req): Json<RegisterHostReq>,
) -> impl IntoResponse {
    let valid_addr = reqwest::Url::parse(&format!("http://{}", req.addr))
        .ok()
        .is_some_and(|url| {
            url.host_str().is_some()
                && url.port().is_some()
                && url.path() == "/"
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        });
    if !valid_addr {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResp::<Host>::err("agent address must be host:port")),
        );
    }
    let client = match agent_client(db.api_key.as_deref(), 30) {
        Ok(client) => client,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiResp::err(e))),
    };
    let url = format!("http://{}/api/info", req.addr);

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiResp::<Host>::err(format!(
                    "cannot reach agent at {}: {e}",
                    req.addr
                ))),
            );
        }
    };

    let status = resp.status();
    let info: ApiResp<AgentInfo> = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiResp::<Host>::err(format!("invalid agent response: {e}"))),
            );
        }
    };

    let info = match info.data.filter(|_| status.is_success() && info.ok) {
        Some(i) => i,
        None => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiResp::<Host>::err("agent returned no data")),
            );
        }
    };

    if let Err(e) = validate_name(&info.host_id, "host_id") {
        return (StatusCode::BAD_GATEWAY, Json(ApiResp::<Host>::err(e)));
    }
    let db = db.lock_db();

    let existing = match db.list_hosts() {
        Ok(hosts) => hosts,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResp::err(e.to_string())),
            );
        }
    };
    if existing.iter().any(|h| {
        (h.id == info.host_id && h.addr != req.addr) || (h.addr == req.addr && h.id != info.host_id)
    }) {
        return (
            StatusCode::CONFLICT,
            Json(ApiResp::err(
                "host ID or address already belongs to a registered peer; drain and remove it before replacement",
            )),
        );
    }
    if let Some(host) = existing.iter().find(|h| h.id == info.host_id) {
        // Refresh a legitimate registration without dropping outstanding reservations.
        let mut host = host.clone();
        let previous = host.resource;
        host.resource = info.resource;
        host.resource.cpu_used = host.resource.cpu_used.max(previous.cpu_used);
        host.resource.mem_used = host.resource.mem_used.max(previous.mem_used);
        host.resource.disk_used = host.resource.disk_used.max(previous.disk_used);
        host.resource.vm_count = host.resource.vm_count.max(previous.vm_count);
        host.engines = info.engines;
        host.capabilities = info.capabilities;
        host.images = info.images;
        host.image_sizes = info.image_sizes;
        host.state = HostState::Online;
        host.error = (!info.warnings.is_empty()).then(|| info.warnings.join("; "));
        if let Err(e) = db.put_host(&host) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResp::err(e.to_string())),
            );
        }
        return (StatusCode::OK, Json(ApiResp::success(host)));
    }
    let count = match db.host_count() {
        Ok(count) => count,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResp::err(e.to_string())),
            );
        }
    };
    if count >= MAX_HOSTS {
        return (
            StatusCode::CONFLICT,
            Json(ApiResp::err(format!(
                "fleet limit reached ({MAX_HOSTS} hosts)"
            ))),
        );
    }
    let host = Host {
        error: None,
        image_sizes: info.image_sizes,
        capabilities: info.capabilities,
        id: info.host_id,
        addr: req.addr,
        resource: info.resource,
        state: HostState::Online,
        images: info.images,
        engines: info.engines,
        storage: info.storage,
        registered_at: now(),
    };

    if let Err(e) = db.put_host(&host) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResp::<Host>::err(e.to_string())),
        );
    }

    (StatusCode::CREATED, Json(ApiResp::success(host)))
}

/// GET /api/hosts
pub async fn list_hosts(State(db): State<CtlState>) -> impl IntoResponse {
    let db = db.lock_db();
    response(db.list_hosts().map_err(internal), StatusCode::OK)
}

/// GET /api/hosts/:id
pub async fn get_host(State(db): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    let db = db.lock_db();
    match db.get_host(&id) {
        Ok(Some(h)) => (StatusCode::OK, Json(ApiResp::success(h))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResp::<Host>::err(format!("host not found: {id}"))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResp::<Host>::err(e.to_string())),
        ),
    }
}

/// DELETE /api/hosts/:id
pub async fn remove_host(State(db): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    let db = db.lock_db();

    let vms = match db.vms_by_host(&id) {
        Ok(vms) => vms,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiRespEmpty::err(e.to_string())),
            );
        }
    };
    if !vms.is_empty() {
        return (
            StatusCode::CONFLICT,
            Json(ApiRespEmpty::err(format!(
                "host {id} still has {} VMs; destroy them first",
                vms.len()
            ))),
        );
    }

    if let Err(e) = db.remove_host(&id) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiRespEmpty::err(e.to_string())),
        );
    }

    (StatusCode::OK, Json(ApiRespEmpty::ok()))
}

/// Explicit recovery escape hatch; does not send a destructive command to a host.
pub async fn detach_host(
    State(state): State<CtlState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = (|| {
        let db = state.lock_db();
        let host = db
            .get_host(&id)
            .map_err(internal)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "host not found".into()))?;
        if host.state != HostState::Offline {
            return Err((
                StatusCode::CONFLICT,
                "only an offline host can be detached; drain online guests first".into(),
            ));
        }
        if db
            .vms_by_host(&id)
            .map_err(internal)?
            .iter()
            .any(|v| state.busy(&v.env_id))
        {
            return Err((
                StatusCode::CONFLICT,
                "host has an environment operation in progress".into(),
            ));
        }
        let orphans = db.detach_host(&id).map_err(internal)?;
        state.revision.fetch_add(1, Ordering::SeqCst);
        Ok(orphans)
    })();
    response(result, StatusCode::OK)
}

// ── Environment Management ──────────────────────────────────────────

type ApiError = (StatusCode, String);
fn internal(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn response<T>(result: Result<T, ApiError>, success: StatusCode) -> (StatusCode, Json<ApiResp<T>>) {
    match result {
        Ok(data) => (success, Json(ApiResp::success(data))),
        Err((status, error)) => (status, Json(ApiResp::err(error))),
    }
}

pub async fn create_env(
    State(state): State<CtlState>,
    Json(req): Json<CreateEnvReq>,
) -> impl IntoResponse {
    let result = tokio::spawn(async move { create_environment(&state, req).await }).await;
    response(
        result.unwrap_or_else(|e| Err(internal(e))),
        StatusCode::ACCEPTED,
    )
}

async fn create_environment(state: &CtlState, req: CreateEnvReq) -> Result<EnvDetail, ApiError> {
    validate_name(&req.id, "environment").map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if req.vms.is_empty() || req.vms.len() > MAX_VMS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("an environment must contain 1..={MAX_VMS} VMs"),
        ));
    }
    for spec in &req.vms {
        ttcore::guest_config::validate(spec.engine, &spec.guest_config, spec.isolated_network)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        validate_image(&spec.image, spec.engine).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        let keys: Vec<_> = req.ssh_keys.iter().chain(&spec.ssh_keys).cloned().collect();
        validate_vm_options(
            spec.engine,
            spec.disk,
            spec.deny_outgoing,
            &keys,
            &spec.ports,
        )
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        if spec.cpu == Some(0) || spec.mem == Some(0) || spec.disk == Some(0) {
            return Err((
                StatusCode::BAD_REQUEST,
                "cpu, memory and disk must be > 0 when specified".into(),
            ));
        }
    }
    let created_at = now();
    let expires_at = match req.lifetime.unwrap_or(DEFAULT_LIFETIME) {
        0 => 0,
        seconds => created_at
            .checked_add(seconds)
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "lifetime is too large".into()))?,
    };
    let operation = begin_operation(state, &req.id)?;
    let read_client = agent_client(state.api_key.as_deref(), 5).map_err(internal)?;
    refresh_all_hosts(state, &read_client).await;
    // Placement and persistence share one short DB critical section.
    let db = state.lock_db();
    {
        if db.get_env(&req.id).map_err(internal)?.is_some() {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "environment '{}' already exists; inspect it with 'tt env show {}'",
                    req.id, req.id
                ),
            ));
        }
        if db
            .vm_count()
            .map_err(internal)?
            .saturating_add(req.vms.len())
            > MAX_VMS
        {
            return Err((StatusCode::CONFLICT, "fleet VM limit reached".into()));
        }
    }
    let mut hosts = db.list_hosts().map_err(internal)?;
    // Include plans accepted since the last agent snapshot.
    for host in &mut hosts {
        let mut tracked = Resource {
            cpu_total: host.resource.cpu_total,
            mem_total: host.resource.mem_total,
            disk_total: host.resource.disk_total,
            ..Default::default()
        };
        for vm in db.vms_by_host(&host.id).map_err(internal)? {
            tracked.account(&vm);
        }
        host.resource.cpu_used = host.resource.cpu_used.max(tracked.cpu_used);
        host.resource.mem_used = host.resource.mem_used.max(tracked.mem_used);
        host.resource.disk_used = host.resource.disk_used.max(tracked.disk_used);
    }
    let host_images = hosts
        .iter()
        .map(|h| (h.id.clone(), h.images.iter().cloned().collect()))
        .collect();
    let placements = scheduler::schedule_env(&hosts, &req.vms, &host_images)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    let mut planned = Vec::new();
    let mut requests = Vec::new();
    for (spec, placement) in placements {
        let mut ssh_keys = req.ssh_keys.clone();
        ssh_keys.extend(spec.ssh_keys);
        ssh_keys.sort();
        ssh_keys.dedup();
        let vm_id = uuid::Uuid::new_v4().to_string();
        let cpu = spec.cpu.unwrap_or(VM_CPU_DEFAULT);
        let mem = spec.mem.unwrap_or(VM_MEM_DEFAULT);
        let requested_disk = spec.disk.unwrap_or(spec.engine.default_disk());
        let disk = placement.disk;
        planned.push(Vm {
            pending_resources: None,
            id: vm_id.clone(),
            env_id: req.id.clone(),
            host_id: placement.host_id,
            image: spec.image.clone(),
            engine: spec.engine,
            cpu,
            mem,
            disk,
            ip: String::new(),
            port_map: Default::default(),
            state: VmState::Creating,
            created_at,
            options: VmOptions {
                ports: spec.ports.clone(),
                ssh_keys: ssh_keys.clone(),
                deny_outgoing: spec.deny_outgoing,
                requested_disk,
                isolated_network: spec.isolated_network,
                guest_config_digest: ttcore::guest_config::digest(&spec.guest_config),
            },
            error: None,
        });
        requests.push((
            placement.host_addr,
            CreateVmReq {
                vm_id,
                env_id: req.id.clone(),
                image: spec.image,
                engine: spec.engine,
                cpu,
                mem,
                disk: requested_disk,
                ports: spec.ports,
                deny_outgoing: spec.deny_outgoing,
                ssh_keys,
                isolated_network: spec.isolated_network,
                guest_config: spec.guest_config,
            },
        ));
    }
    let env = Env {
        id: req.id.clone(),
        owner: req.owner,
        vm_ids: planned.iter().map(|v| v.id.clone()).collect(),
        created_at,
        expires_at,
        state: EnvState::Creating,
        error: None,
    };
    db.put_environment(&env, &planned).map_err(internal)?;
    for host in &mut hosts {
        for vm in planned.iter().filter(|v| v.host_id == host.id) {
            host.resource.account(vm);
        }
        db.put_host(host).map_err(internal)?;
    }
    state.revision.fetch_add(1, Ordering::SeqCst);
    drop(db);
    let detail = environment_detail(state, &req.id)?;
    let state = state.clone();
    let env_id = req.id;
    tokio::spawn(async move {
        let _operation = operation;
        if let Err((_, error)) = finish_creation(state, env_id, requests).await {
            eprintln!("[ctl] creation: {error}");
        }
    });
    Ok(detail)
}

async fn finish_creation(
    state: CtlState,
    env_id: String,
    requests: Vec<(String, CreateVmReq)>,
) -> Result<(), ApiError> {
    let read_client = agent_client(state.api_key.as_deref(), 5).map_err(internal)?;
    let client = agent_client(state.api_key.as_deref(), 360).map_err(internal)?;
    for (addr, request) in requests {
        let expired = state
            .lock_db()
            .get_env(&env_id)
            .map_err(internal)?
            .is_none_or(|e| e.expires_at != 0 && e.expires_at <= now());
        if expired {
            break;
        }
        match create_on_agent(&client, &addr, &request).await {
            Ok(vm) => state.lock_db().put_vm(&vm).map_err(internal)?,
            Err(error) => {
                // A timeout is ambiguous. Keep the ID and allocation until reconciliation.
                let mut vm = state
                    .lock_db()
                    .get_vm(&request.vm_id)
                    .map_err(internal)?
                    .ok_or_else(|| internal("missing planned VM"))?;
                vm.error = Some(error.clone());
                state.lock_db().put_vm(&vm).map_err(internal)?;
                eprintln!("[ctl] VM {}: {error}", request.vm_id);
            }
        }
    }
    refresh_hosts(&state, &read_client, Some(&env_id)).await;
    update_env_state(&state, &env_id).map_err(internal)?;
    Ok(())
}

async fn create_on_agent(
    client: &reqwest::Client,
    addr: &str,
    request: &CreateVmReq,
) -> Result<Vm, String> {
    let resp = client
        .post(format!("http://{addr}/api/vms"))
        .json(request)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: ApiResp<CreateVmResp> = resp
        .json()
        .await
        .map_err(|e| format!("agent HTTP {status}: invalid response: {e}"))?;
    if !status.is_success() || !body.ok {
        return Err(body.error.unwrap_or_else(|| format!("agent HTTP {status}")));
    }
    let vm = body.data.ok_or("agent returned no VM")?.vm;
    if vm.id != request.vm_id || vm.env_id != request.env_id {
        return Err("agent returned an unexpected VM identity".into());
    }
    Ok(vm)
}

pub async fn list_envs(State(state): State<CtlState>) -> impl IntoResponse {
    response(
        state.lock_db().list_envs().map_err(internal),
        StatusCode::OK,
    )
}
fn environment_detail(state: &CtlState, id: &str) -> Result<EnvDetail, ApiError> {
    let db = state.lock_db();
    let env = db.get_env(id).map_err(internal)?.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("environment not found: {id}"),
        )
    })?;
    let vms = db.vms_by_env(id).map_err(internal)?;
    let warnings = vms
        .iter()
        .filter_map(|vm| vm.error.as_ref().map(|e| format!("{}: {e}", vm.id)))
        .chain(env.error.clone())
        .collect();
    Ok(EnvDetail { env, vms, warnings })
}
pub async fn get_env(State(state): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    response(environment_detail(&state, &id), StatusCode::OK)
}

pub async fn delete_env(
    State(state): State<CtlState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let result = tokio::spawn(async move { delete_environment(&state, &id).await }).await;
    response(result.unwrap_or_else(|e| Err(internal(e))), StatusCode::OK)
}

/// Retain deletion intent until every agent confirms cleanup. Also used by expiry/recovery.
pub async fn delete_environment(state: &CtlState, id: &str) -> Result<(), ApiError> {
    delete_environment_inner(state, id, false).await
}

pub async fn expire_environment(state: &CtlState, id: &str) -> Result<(), ApiError> {
    delete_environment_inner(state, id, true).await
}

async fn delete_environment_inner(
    state: &CtlState,
    id: &str,
    expiry_only: bool,
) -> Result<(), ApiError> {
    let _operation = begin_operation(state, id)?;
    let (mut env, vms, hosts) = {
        let db = state.lock_db();
        let Some(mut env) = db.get_env(id).map_err(internal)? else {
            return Ok(());
        };
        // Recheck under the mutation lock: a reused name must not inherit old expiry.
        if expiry_only
            && env.state != EnvState::Deleting
            && (env.expires_at == 0 || env.expires_at > now())
        {
            return Ok(());
        }
        env.state = EnvState::Deleting;
        db.put_env(&env).map_err(internal)?;
        (
            env,
            db.vms_by_env(id).map_err(internal)?,
            db.recovery_hosts().map_err(internal)?,
        )
    };
    let client = agent_client(state.api_key.as_deref(), 360).map_err(internal)?;
    let mut errors = Vec::new();
    for vm in &vms {
        let result = match hosts.iter().find(|h| h.id == vm.host_id) {
            Some(host) if host.state == HostState::Online => {
                agent_action(&client, host, &vm.id, None).await
            }
            Some(_) => Err(format!(
                "host {} is offline; cleanup retained for retry",
                vm.host_id
            )),
            None => Err(format!("host {} is missing", vm.host_id)),
        };
        match result {
            Ok(()) => state.lock_db().remove_vm(&vm.id).map_err(internal)?,
            Err(error) => errors.push(format!("{}: {error}", vm.id)),
        }
    }
    let db = state.lock_db();
    if errors.is_empty() {
        db.remove_env(id).map_err(internal)?;
        Ok(())
    } else {
        env.error = Some(errors.join("; "));
        env.vm_ids = db
            .vms_by_env(id)
            .map_err(internal)?
            .into_iter()
            .map(|v| v.id)
            .collect();
        db.put_env(&env).map_err(internal)?;
        Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "cleanup incomplete; environment retained for retry: {}",
                errors.join("; ")
            ),
        ))
    }
}

async fn agent_action(
    client: &reqwest::Client,
    host: &Host,
    id: &str,
    action: Option<&str>,
) -> Result<(), String> {
    let url = format!("http://{}/api/vms/{id}", host.addr);
    let resp = match action {
        Some(action) => client.post(format!("{url}/{action}")),
        None => client.delete(url),
    }
    .send()
    .await
    .map_err(|e| e.to_string())?;
    let status = resp.status();
    let body: ApiRespEmpty = resp
        .json()
        .await
        .map_err(|e| format!("agent HTTP {status}: invalid response: {e}"))?;
    if status.is_success() && body.ok {
        Ok(())
    } else {
        Err(body.error.unwrap_or_else(|| format!("agent HTTP {status}")))
    }
}

pub async fn stop_env(State(state): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    let result = tokio::spawn(async move { change_environment(&state, &id, "stop").await }).await;
    response(result.unwrap_or_else(|e| Err(internal(e))), StatusCode::OK)
}
pub async fn start_env(State(state): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    let result = tokio::spawn(async move { change_environment(&state, &id, "start").await }).await;
    response(result.unwrap_or_else(|e| Err(internal(e))), StatusCode::OK)
}

/// The operation is per VM: a multi-VM environment has no atomic resize promise.
pub async fn resize_vm(
    State(state): State<CtlState>,
    Path(id): Path<String>,
    Json(target): Json<VmResources>,
) -> impl IntoResponse {
    let result =
        tokio::spawn(async move { resize_virtual_machine(&state, &id, target).await }).await;
    response(result.unwrap_or_else(|e| Err(internal(e))), StatusCode::OK)
}

async fn resize_virtual_machine(
    state: &CtlState,
    id: &str,
    target: VmResources,
) -> Result<Vm, ApiError> {
    if target.cpu == 0 || target.mem == 0 || target.disk == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "cpu, memory and root disk must be > 0".into(),
        ));
    }
    let original = state
        .lock_db()
        .get_vm(id)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "VM not found".into()))?;
    let _operation = begin_operation(state, &original.env_id)?;
    let detail = environment_detail(state, &original.env_id)?;
    if matches!(detail.env.state, EnvState::Creating | EnvState::Deleting) {
        return Err((
            StatusCode::CONFLICT,
            "environment has an operation in progress".into(),
        ));
    }
    let client = agent_client(state.api_key.as_deref(), 360).map_err(internal)?;
    refresh_hosts(
        state,
        &agent_client(state.api_key.as_deref(), 5).map_err(internal)?,
        Some(&original.env_id),
    )
    .await;
    let mut vm = state
        .lock_db()
        .get_vm(id)
        .map_err(internal)?
        .ok_or_else(|| internal("VM disappeared"))?;
    let host = state
        .lock_db()
        .get_host(&vm.host_id)
        .map_err(internal)?
        .ok_or_else(|| internal("VM host missing"))?;
    if host.state != HostState::Online {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "VM host is offline".into()));
    }
    if vm.engine != Engine::Firecracker
        || !host
            .capabilities
            .iter()
            .any(|c| c == "firecracker_resources")
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "agent lacks firecracker_resources; upgrade agent and controller".into(),
        ));
    }
    if vm.state != VmState::Stopped || vm.pending_resources.is_some_and(|r| r != target) {
        return Err((
            StatusCode::CONFLICT,
            "stop the VM first; an unfinished update must use its recorded target".into(),
        ));
    }
    let overhead = if vm.options.guest_config_digest.is_some() {
        ttcore::guest_config::CONFIG_DISK_MIB
    } else {
        0
    };
    let disk = target
        .disk
        .checked_add(overhead)
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "disk overflow".into()))?;
    if target.disk < vm.options.requested_disk || disk < vm.disk {
        return Err((
            StatusCode::BAD_REQUEST,
            "disk shrinking is not supported".into(),
        ));
    }
    if !host.resource.can_fit(
        target.cpu,
        vm.engine.memory_reservation(target.mem),
        disk.saturating_sub(vm.reserved_disk()),
    ) {
        return Err((
            StatusCode::CONFLICT,
            "insufficient resources on the workspace host".into(),
        ));
    }
    vm.pending_resources = Some(target);
    {
        let db = state.lock_db();
        db.put_vm(&vm).map_err(internal)?;
        state.revision.fetch_add(1, Ordering::SeqCst);
    }
    let result = async {
        let resp = client
            .post(format!("http://{}/api/vms/{id}/resources", host.addr))
            .json(&target)
            .send()
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("resource update outcome unknown; inspect VM before retrying: {e}"),
                )
            })?;
        let status = resp.status();
        let body: ApiResp<Vm> = resp.json().await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("invalid agent response: {e}"),
            )
        })?;
        if !status.is_success() || !body.ok {
            return Err((
                if status.is_client_error() {
                    status
                } else {
                    StatusCode::BAD_GATEWAY
                },
                body.error
                    .unwrap_or_else(|| "resource update failed".into()),
            ));
        }
        let confirmed = body.data.ok_or_else(|| internal("agent returned no VM"))?;
        if confirmed.id != vm.id
            || confirmed.env_id != vm.env_id
            || confirmed.host_id != vm.host_id
            || confirmed.state != VmState::Stopped
            || confirmed.pending_resources.is_some()
            || confirmed.cpu != target.cpu
            || confirmed.mem != target.mem
            || confirmed.options.requested_disk != target.disk
            || confirmed.disk != disk
        {
            return Err((
                StatusCode::BAD_GATEWAY,
                "agent returned inconsistent resource update".into(),
            ));
        }
        state.lock_db().put_vm(&confirmed).map_err(internal)?;
        Ok(confirmed)
    }
    .await;
    refresh_hosts(
        state,
        &agent_client(state.api_key.as_deref(), 5).map_err(internal)?,
        Some(&original.env_id),
    )
    .await;
    result
}
async fn change_environment(state: &CtlState, id: &str, action: &str) -> Result<(), ApiError> {
    let _operation = begin_operation(state, id)?;
    let detail = environment_detail(state, id)?;
    if matches!(detail.env.state, EnvState::Creating | EnvState::Deleting) {
        return Err((
            StatusCode::CONFLICT,
            "environment has an operation in progress".into(),
        ));
    }
    let hosts = state.lock_db().recovery_hosts().map_err(internal)?;
    let client = agent_client(state.api_key.as_deref(), 360).map_err(internal)?;
    if action == "start" {
        if detail.vms.iter().any(|vm| vm.pending_resources.is_some()) {
            return Err((
                StatusCode::CONFLICT,
                "resource update unfinished; retry the recorded resources before starting".into(),
            ));
        }
        let mut starting = detail.vms.clone();
        for vm in &mut starting {
            if vm.state == VmState::Stopped {
                vm.state = VmState::Creating;
            }
        }
        // Reserve stopped guests before concurrent placement can spend their capacity.
        // A failed/unknown start retains this reservation until an agent snapshot arrives.
        let db = state.lock_db();
        db.put_environment(&detail.env, &starting)
            .map_err(internal)?;
        state.revision.fetch_add(1, Ordering::SeqCst);
    }
    let mut errors = Vec::new();
    for vm in &detail.vms {
        let result = match hosts.iter().find(|h| h.id == vm.host_id) {
            Some(host) => agent_action(&client, host, &vm.id, Some(action)).await,
            None => Err(format!("host {} is missing", vm.host_id)),
        };
        match result {
            Ok(()) => {
                // An unrelated environment can invalidate the fleet refresh below.
                // Keep this agent's confirmed result even when that snapshot is stale.
                let mut confirmed = vm.clone();
                confirmed.state = if action == "start" {
                    VmState::Running
                } else {
                    VmState::Stopped
                };
                confirmed.error = None;
                state.lock_db().put_vm(&confirmed).map_err(internal)?;
            }
            Err(error) => errors.push(format!("{}: {error}", vm.id)),
        }
    }
    refresh_hosts(
        state,
        &agent_client(state.api_key.as_deref(), 5).map_err(internal)?,
        Some(id),
    )
    .await;
    update_env_state(state, id).map_err(internal)?;
    let mut env = state
        .lock_db()
        .get_env(id)
        .map_err(internal)?
        .ok_or_else(|| internal("environment disappeared"))?;
    let expected = if action == "start" {
        EnvState::Active
    } else {
        EnvState::Stopped
    };
    if !errors.is_empty() || env.state != expected {
        env.state = EnvState::Failed;
        env.error = Some(format!("{action} incomplete: {}", errors.join("; ")));
        state.lock_db().put_env(&env).map_err(internal)?;
        return Err((StatusCode::BAD_GATEWAY, env.error.unwrap()));
    }
    Ok(())
}

fn update_env_state(state: &CtlState, id: &str) -> ruc::Result<()> {
    let db = state.lock_db();
    let Some(mut env) = db.get_env(id)? else {
        return Ok(());
    };
    if env.state == EnvState::Deleting {
        return Ok(());
    }
    let vms = db.vms_by_env(id)?;
    let hosts = db.recovery_hosts()?;
    let unavailable = vms.iter().any(|vm| {
        !hosts
            .iter()
            .any(|h| h.id == vm.host_id && h.state == HostState::Online)
    });
    if unavailable {
        env.state = EnvState::Failed;
        env.error =
            Some("one or more agents are offline; VM states are last observed snapshots".into());
        return db.put_env(&env);
    }
    env.state = if vms.is_empty() || vms.iter().any(|v| v.error.is_some()) {
        EnvState::Failed
    } else if vms.iter().any(|v| v.state == VmState::Creating) {
        EnvState::Creating
    } else if vms.iter().all(|v| v.state == VmState::Running) {
        EnvState::Active
    } else if vms.iter().all(|v| v.state == VmState::Stopped) {
        EnvState::Stopped
    } else {
        EnvState::Failed
    };
    env.vm_ids = vms.iter().map(|v| v.id.clone()).collect();
    env.error = if env.state == EnvState::Failed && vms.is_empty() {
        env.error
            .or_else(|| Some("environment has no tracked VMs".into()))
    } else if env.state == EnvState::Failed {
        Some(
            vms.iter()
                .map(|v| {
                    format!(
                        "{}: {}",
                        v.id,
                        v.error.clone().unwrap_or_else(|| v.state.to_string())
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )
    } else {
        None
    };
    db.put_env(&env)
}

// ── Images ──────────────────────────────────────────────────────────

/// GET /api/images
pub async fn list_images(State(state): State<CtlState>) -> impl IntoResponse {
    let result = state
        .lock_db()
        .list_hosts()
        .map(|hosts| {
            hosts
                .into_iter()
                .filter(|h| h.state == HostState::Online)
                .flat_map(|h| {
                    h.images.into_iter().map(move |name| ImageInfo {
                        name,
                        host_id: h.id.clone(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .map_err(internal);
    response(result, StatusCode::OK)
}

// ── Status ──────────────────────────────────────────────────────────

/// GET /api/status
pub async fn fleet_status(State(db): State<CtlState>) -> impl IntoResponse {
    let db = db.lock_db();
    response(db.fleet_status().map_err(internal), StatusCode::OK)
}

// ── VM Lookup ───────────────────────────────────────────────────────

/// GET /api/vms/:id — get a single VM by ID (across all hosts).
pub async fn get_vm(State(db): State<CtlState>, Path(id): Path<String>) -> impl IntoResponse {
    let db = db.lock_db();
    match db.get_vm(&id) {
        Ok(Some(vm)) => (StatusCode::OK, Json(ApiResp::success(vm))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResp::<Vm>::err(format!("VM not found: {id}"))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResp::<Vm>::err(e.to_string())),
        ),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Fetch host snapshots concurrently with a small, fixed concurrency limit.
pub async fn refresh_all_hosts(state: &CtlState, client: &reqwest::Client) {
    refresh_hosts(state, client, None).await;
}
async fn refresh_hosts(state: &CtlState, client: &reqwest::Client, owned_env: Option<&str>) {
    let revision = state.revision.load(Ordering::SeqCst);
    let hosts = match state.lock_db().recovery_hosts() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[ctl] list hosts: {e}");
            return;
        }
    };
    for batch in hosts.chunks(8) {
        let mut tasks = tokio::task::JoinSet::new();
        for host in batch {
            let host = host.clone();
            let client = client.clone();
            tasks.spawn(async move {
                let result = async {
                    let info = client
                        .get(format!("http://{}/api/info", host.addr))
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;
                    let info = info
                        .error_for_status()
                        .map_err(|e| e.to_string())?
                        .json::<ApiResp<AgentInfo>>()
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut info = info.data.filter(|_| info.ok).ok_or("invalid agent info")?;
                    if info.host_id != host.id {
                        return Err("agent identity changed; re-register host".into());
                    }
                    if let Some(vms) = info.vms.take() {
                        return Ok((info, vms));
                    }
                    let vms = client
                        .get(format!("http://{}/api/vms", host.addr))
                        .send()
                        .await
                        .map_err(|e| e.to_string())?;
                    let vms = vms
                        .error_for_status()
                        .map_err(|e| e.to_string())?
                        .json::<ApiResp<Vec<Vm>>>()
                        .await
                        .map_err(|e| e.to_string())?;
                    let vms = vms.data.filter(|_| vms.ok).ok_or("invalid VM snapshot")?;
                    Ok::<_, String>((info, vms))
                }
                .await;
                (host.id, result)
            });
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((id, result)) => {
                    if let Err(e) = apply_snapshot(state, &id, result, Some(revision), owned_env) {
                        eprintln!("[ctl] save snapshot: {e}");
                    }
                }
                Err(e) => eprintln!("[ctl] snapshot task: {e}"),
            }
        }
    }
    let ids = match state.lock_db().recovery_envs() {
        Ok(envs) => envs.into_iter().map(|e| e.id).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for id in ids {
        if owned_env != Some(id.as_str()) && state.busy(&id) {
            continue;
        }
        if let Err(e) = update_env_state(state, &id) {
            eprintln!("[ctl] update {id}: {e}");
        }
    }
}

#[cfg(test)]
fn apply_host_snapshot(
    state: &CtlState,
    id: &str,
    snapshot: Result<(AgentInfo, Vec<Vm>), String>,
) -> ruc::Result<()> {
    apply_snapshot(state, id, snapshot, None, None)
}
fn apply_snapshot(
    state: &CtlState,
    id: &str,
    snapshot: Result<(AgentInfo, Vec<Vm>), String>,
    revision: Option<u64>,
    owned_env: Option<&str>,
) -> ruc::Result<()> {
    let db = state.lock_db();
    if revision.is_some_and(|r| r != state.revision.load(Ordering::SeqCst)) {
        return Ok(());
    }
    let Some(mut host) = db.get_host(id)? else {
        return Ok(());
    };
    match snapshot {
        Ok((info, actual)) => {
            host.resource = Resource {
                cpu_total: info.resource.cpu_total,
                mem_total: info.resource.mem_total,
                disk_total: info.resource.disk_total,
                ..Default::default()
            };
            host.engines = info.engines;
            host.capabilities = info.capabilities;
            host.storage = info.storage;
            host.images = info.images;
            host.image_sizes = info.image_sizes;
            host.state = HostState::Online;
            host.error = (!info.warnings.is_empty()).then(|| info.warnings.join("; "));
            let known_vms = db.vms_by_host(id)?;
            for vm in &actual {
                if !known_vms.iter().any(|v| v.id == vm.id) {
                    eprintln!(
                        "[ctl] host {id}: untracked VM {} (environment {}); inspect agent inventory",
                        vm.id, vm.env_id
                    );
                    host.resource.account(vm);
                }
            }
            for mut known in known_vms {
                if owned_env != Some(known.env_id.as_str()) && state.busy(&known.env_id) {
                    host.resource.account(&known);
                    continue;
                }
                if let Some(vm) = actual
                    .iter()
                    .find(|vm| vm.id == known.id && vm.env_id == known.env_id && vm.host_id == id)
                {
                    host.resource.account(vm);
                    db.put_vm(vm)?;
                } else {
                    // Never discard a resource merely because one snapshot is missing it.
                    known.state = VmState::Failed;
                    if known.error.is_none() {
                        known.error = Some(
                            "VM absent from agent snapshot; inspect or delete to reconcile".into(),
                        );
                    }
                    host.resource.account(&known);
                    db.put_vm(&known)?;
                }
            }
            host.resource.cpu_used = host.resource.cpu_used.max(info.resource.cpu_used);
            host.resource.mem_used = host.resource.mem_used.max(info.resource.mem_used);
            host.resource.disk_used = host.resource.disk_used.max(info.resource.disk_used);
        }
        Err(error) => {
            host.error = Some(error.clone());
            host.state = HostState::Offline;
            eprintln!("[ctl] host {id}: {error}");
        }
    }
    db.put_host(&host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct MockAgent {
        resources_supported: AtomicBool,
        vms: Mutex<Vec<Vm>>,
        stall_info: AtomicBool,
        info_entered: AtomicBool,
        stall_start: AtomicBool,
        start_entered: AtomicBool,
        fail_delete: AtomicBool,
        stall_delete: AtomicBool,
        delete_entered: AtomicBool,
        fail_action: AtomicBool,
        lose_create_response: AtomicBool,
        creates: AtomicUsize,
    }
    fn record(id: &str, env: &str) -> Vm {
        Vm {
            pending_resources: None,
            id: id.into(),
            env_id: env.into(),
            host_id: "host".into(),
            image: "alpine:3.21".into(),
            engine: Engine::Docker,
            cpu: 1,
            mem: 128,
            disk: 0,
            ip: String::new(),
            port_map: Default::default(),
            options: Default::default(),
            error: None,
            state: VmState::Running,
            created_at: 0,
        }
    }
    async fn info(State(mock): State<Arc<MockAgent>>) -> Json<ApiResp<AgentInfo>> {
        if mock.stall_info.load(Ordering::SeqCst) {
            mock.info_entered.store(true, Ordering::SeqCst);
            while mock.stall_info.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        }
        let mut resource = Resource {
            cpu_total: 16,
            mem_total: 16384,
            disk_total: 100000,
            ..Default::default()
        };
        for vm in mock.vms.lock().unwrap().iter() {
            resource.account(vm);
        }
        Json(ApiResp::success(AgentInfo {
            vms: None,
            warnings: vec![],
            image_sizes: Default::default(),
            capabilities: if mock.resources_supported.load(Ordering::SeqCst) {
                vec!["firecracker_resources".into()]
            } else {
                vec![]
            },
            host_id: "host".into(),
            resource,
            engines: vec![Engine::Docker],
            storage: Storage::File,
            images: vec![],
        }))
    }
    async fn vms(State(mock): State<Arc<MockAgent>>) -> Json<ApiResp<Vec<Vm>>> {
        Json(ApiResp::success(mock.vms.lock().unwrap().clone()))
    }
    async fn create(
        State(mock): State<Arc<MockAgent>>,
        Json(req): Json<CreateVmReq>,
    ) -> impl IntoResponse {
        mock.creates.fetch_add(1, Ordering::SeqCst);
        let mut vm = record(&req.vm_id, &req.env_id);
        vm.options = VmOptions {
            isolated_network: req.isolated_network,
            guest_config_digest: ttcore::guest_config::digest(&req.guest_config),
            ports: req.ports,
            ssh_keys: req.ssh_keys,
            deny_outgoing: req.deny_outgoing,
            requested_disk: req.disk,
        };
        mock.vms.lock().unwrap().push(vm.clone());
        if mock.lose_create_response.load(Ordering::SeqCst) {
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ApiResp::<CreateVmResp>::err("response lost after creation")),
            )
        } else {
            (
                StatusCode::CREATED,
                Json(ApiResp::success(CreateVmResp { vm })),
            )
        }
    }
    async fn delete(
        State(mock): State<Arc<MockAgent>>,
        Path(id): Path<String>,
    ) -> impl IntoResponse {
        if id == "bad" && mock.stall_delete.load(Ordering::SeqCst) {
            mock.delete_entered.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        if id == "bad" && mock.fail_delete.load(Ordering::SeqCst) {
            // Even HTTP 200 is not success when the API envelope reports an error.
            return Json(ApiRespEmpty::err("injected cleanup failure"));
        }
        mock.vms.lock().unwrap().retain(|v| v.id != id);
        Json(ApiRespEmpty::ok())
    }
    async fn stop(State(mock): State<Arc<MockAgent>>, Path(id): Path<String>) -> impl IntoResponse {
        if mock.fail_action.load(Ordering::SeqCst) {
            return Json(ApiRespEmpty::err("injected stop failure"));
        }
        if let Some(vm) = mock.vms.lock().unwrap().iter_mut().find(|v| v.id == id) {
            vm.state = VmState::Stopped;
        }
        Json(ApiRespEmpty::ok())
    }
    async fn start(
        State(mock): State<Arc<MockAgent>>,
        Path(id): Path<String>,
    ) -> impl IntoResponse {
        mock.start_entered.store(true, Ordering::SeqCst);
        while mock.stall_start.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        if let Some(vm) = mock.vms.lock().unwrap().iter_mut().find(|v| v.id == id) {
            vm.state = VmState::Running;
        }
        Json(ApiRespEmpty::ok())
    }
    async fn resize(
        State(mock): State<Arc<MockAgent>>,
        Path(id): Path<String>,
        Json(target): Json<VmResources>,
    ) -> impl IntoResponse {
        let mut rows = mock.vms.lock().unwrap();
        let vm = rows.iter_mut().find(|v| v.id == id).unwrap();
        vm.cpu = target.cpu;
        vm.mem = target.mem;
        vm.disk = target.disk;
        vm.options.requested_disk = target.disk;
        if mock.fail_action.load(Ordering::SeqCst) {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ApiResp::<Vm>::err("response lost after resource update")),
            );
        }
        (StatusCode::OK, Json(ApiResp::success(vm.clone())))
    }
    #[tokio::test]
    async fn resize_requires_capability_and_stopped_vm_and_recovers_lost_reply() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        let target = VmResources {
            cpu: 2,
            mem: 512,
            disk: 1024,
        };
        assert_eq!(
            resize_virtual_machine(&state, "good", target)
                .await
                .unwrap_err()
                .0,
            StatusCode::BAD_REQUEST
        );
        mock.resources_supported.store(true, Ordering::SeqCst);
        for vm in mock.vms.lock().unwrap().iter_mut() {
            vm.engine = Engine::Firecracker;
        }
        assert_eq!(
            resize_virtual_machine(&state, "good", target)
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        for vm in mock.vms.lock().unwrap().iter_mut() {
            vm.state = VmState::Stopped;
        }
        assert_eq!(
            resize_virtual_machine(&state, "good", VmResources { cpu: 17, ..target })
                .await
                .unwrap_err()
                .0,
            StatusCode::CONFLICT
        );
        mock.fail_action.store(true, Ordering::SeqCst);
        assert!(
            resize_virtual_machine(&state, "good", target)
                .await
                .is_err()
        );
        let saved = state.lock_db().get_vm("good").unwrap().unwrap();
        assert_eq!((saved.cpu, saved.mem, saved.disk), (2, 512, 1024));
        assert!(saved.pending_resources.is_none());
        mock.fail_action.store(false, Ordering::SeqCst);
        assert!(resize_virtual_machine(&state, "good", target).await.is_ok());
        assert_eq!(
            resize_virtual_machine(
                &state,
                "good",
                VmResources {
                    disk: 512,
                    ..target
                }
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::BAD_REQUEST
        );
        server.abort();
    }
    async fn fixture() -> (CtlState, Arc<MockAgent>, tokio::task::JoinHandle<()>) {
        let mock = Arc::new(MockAgent {
            resources_supported: AtomicBool::new(false),
            vms: Mutex::new(vec![]),
            stall_info: AtomicBool::new(false),
            info_entered: AtomicBool::new(false),
            stall_start: AtomicBool::new(false),
            start_entered: AtomicBool::new(false),
            fail_delete: AtomicBool::new(false),
            stall_delete: AtomicBool::new(false),
            delete_entered: AtomicBool::new(false),
            fail_action: AtomicBool::new(false),
            lose_create_response: AtomicBool::new(false),
            creates: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/api/info", get(info))
            .route("/api/vms", get(vms).post(create))
            .route("/api/vms/{id}", axum::routing::delete(delete))
            .route("/api/vms/{id}/stop", post(stop))
            .route("/api/vms/{id}/start", post(start))
            .route("/api/vms/{id}/resources", post(resize))
            .with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let state = Arc::new(CtlShared::new(Db::open(":memory:").unwrap(), None));
        state
            .lock_db()
            .put_host(&Host {
                error: None,
                image_sizes: Default::default(),
                capabilities: vec![],
                id: "host".into(),
                addr,
                resource: Resource::default(),
                state: HostState::Online,
                engines: vec![Engine::Docker],
                images: vec![],
                storage: Storage::File,
                registered_at: 0,
            })
            .unwrap();
        (state, mock, task)
    }
    fn seed(state: &CtlState, mock: &MockAgent) {
        let records = vec![record("good", "env"), record("bad", "env")];
        let env = Env {
            id: "env".into(),
            owner: "test".into(),
            vm_ids: vec!["good".into(), "bad".into()],
            created_at: 0,
            expires_at: 1,
            state: EnvState::Active,
            error: None,
        };
        state.lock_db().put_environment(&env, &records).unwrap();
        *mock.vms.lock().unwrap() = records;
    }
    fn request() -> CreateEnvReq {
        CreateEnvReq {
            id: "new-env".into(),
            owner: "test".into(),
            lifetime: Some(0),
            ssh_keys: vec![],
            vms: vec![VmSpec {
                isolated_network: false,
                guest_config: Default::default(),
                image: "alpine:3.21".into(),
                engine: Engine::Docker,
                cpu: Some(1),
                mem: Some(128),
                disk: None,
                ports: vec![],
                deny_outgoing: false,
                ssh_keys: vec![],
            }],
        }
    }
    #[tokio::test]
    async fn stalled_delete_does_not_block_another_environment_or_heartbeat() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        mock.stall_delete.store(true, Ordering::SeqCst);
        let stalled = {
            let state = state.clone();
            tokio::spawn(async move { delete_environment(&state, "env").await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !mock.delete_entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mut env = state.lock_db().get_env("env").unwrap().unwrap();
        env.id = "healthy".into();
        env.vm_ids = vec!["healthy-vm".into()];
        env.state = EnvState::Active;
        let vm = record("healthy-vm", "healthy");
        state
            .lock_db()
            .put_environment(&env, std::slice::from_ref(&vm))
            .unwrap();
        mock.vms.lock().unwrap().push(vm);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            delete_environment(&state, "healthy"),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            refresh_all_hosts(&state, &agent_client(None, 1).unwrap()),
        )
        .await
        .unwrap();
        assert!(state.lock_db().get_env("healthy").unwrap().is_none());
        assert!(state.lock_db().get_env("env").unwrap().is_some());
        stalled.abort();
        server.abort();
    }

    #[tokio::test]
    async fn pending_restart_reserves_capacity_against_concurrent_creation() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        {
            let db = state.lock_db();
            let mut env = db.get_env("env").unwrap().unwrap();
            env.state = EnvState::Stopped;
            let mut records = mock.vms.lock().unwrap();
            for vm in records.iter_mut() {
                vm.state = VmState::Stopped;
                vm.cpu = 8;
            }
            db.put_environment(&env, &records).unwrap();
        }
        mock.stall_start.store(true, Ordering::SeqCst);
        let started = {
            let state = state.clone();
            tokio::spawn(async move { change_environment(&state, "env", "start").await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !mock.start_entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            create_environment(&state, request()).await.unwrap_err().0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(mock.creates.load(Ordering::SeqCst), 0);
        mock.stall_start.store(false, Ordering::SeqCst);
        started.await.unwrap().unwrap();
        assert_eq!(
            state.lock_db().get_env("env").unwrap().unwrap().state,
            EnvState::Active
        );
        server.abort();
    }

    #[tokio::test]
    async fn confirmed_stop_survives_a_concurrently_invalidated_refresh() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        mock.stall_info.store(true, Ordering::SeqCst);
        let stopped = {
            let state = state.clone();
            tokio::spawn(async move { change_environment(&state, "env", "stop").await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !mock.info_entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(begin_operation(&state, "another-env").unwrap());
        mock.stall_info.store(false, Ordering::SeqCst);
        stopped.await.unwrap().unwrap();
        assert_eq!(
            state.lock_db().get_env("env").unwrap().unwrap().state,
            EnvState::Stopped
        );
        server.abort();
    }

    #[tokio::test]
    async fn old_snapshots_cannot_overwrite_new_mutation_state() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        let before = state.revision.load(Ordering::SeqCst);
        let operation = begin_operation(&state, "env").unwrap();
        drop(operation);
        apply_snapshot(
            &state,
            "host",
            Err("stale failure".into()),
            Some(before),
            None,
        )
        .unwrap();
        assert_eq!(
            state.lock_db().get_host("host").unwrap().unwrap().state,
            HostState::Online
        );
        server.abort();
    }

    #[tokio::test]
    async fn offline_detach_returns_orphans_and_releases_only_tracking() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        apply_host_snapshot(&state, "host", Err("offline".into())).unwrap();
        let orphans = state.lock_db().detach_host("host").unwrap();
        assert_eq!(orphans.len(), 2);
        assert_eq!(mock.vms.lock().unwrap().len(), 2);
        assert_eq!(state.lock_db().vm_count().unwrap(), 0);
        assert!(
            state
                .lock_db()
                .get_env("env")
                .unwrap()
                .unwrap()
                .error
                .unwrap()
                .contains("detached")
        );
        server.abort();
    }

    #[tokio::test]
    async fn partial_delete_preserves_failed_vm_and_expiry_retries_it() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        mock.fail_delete.store(true, Ordering::SeqCst);
        assert!(delete_environment(&state, "env").await.is_err());
        let detail = environment_detail(&state, "env").unwrap();
        assert_eq!(detail.env.state, EnvState::Deleting);
        assert_eq!(detail.vms.len(), 1);
        assert_eq!(detail.vms[0].id, "bad");
        assert!(detail.env.error.unwrap().contains("injected"));
        mock.fail_delete.store(false, Ordering::SeqCst);
        crate::expire_envs(&state).await;
        assert!(state.lock_db().get_env("env").unwrap().is_none());
        assert!(mock.vms.lock().unwrap().is_empty());
        assert!(delete_environment(&state, "env").await.is_ok());
        server.abort();
    }
    #[tokio::test]
    async fn lifetime_defaults_long_terms_and_permanent_envs_survive_expiry_sweep() {
        for lifetime in [None, Some(30 * 86400), Some(3650 * 86400), Some(0)] {
            let (state, mock, server) = fixture().await;
            let mut req = request();
            req.lifetime = lifetime;
            let accepted = create_environment(&state, req).await.unwrap();
            let expected = match lifetime.unwrap_or(DEFAULT_LIFETIME) {
                0 => 0,
                seconds => accepted.env.created_at + seconds,
            };
            assert_eq!(accepted.env.expires_at, expected);
            {
                let _finished = state.operation_lock("new-env").lock_owned().await;
            }
            crate::expire_envs(&state).await;
            let detail = environment_detail(&state, "new-env").unwrap();
            assert_eq!(detail.env.expires_at, expected);
            assert_eq!(detail.env.state, EnvState::Active);
            assert_eq!(detail.vms.len(), 1);
            assert_eq!(mock.vms.lock().unwrap().len(), 1);
            server.abort();
        }
    }
    #[tokio::test]
    async fn lost_create_response_is_reconciled_without_duplicate_resources() {
        let (state, mock, server) = fixture().await;
        mock.lose_create_response.store(true, Ordering::SeqCst);
        let accepted = create_environment(&state, request()).await.unwrap();
        assert_eq!(accepted.env.state, EnvState::Creating);
        assert_eq!(accepted.env.expires_at, 0);
        assert_eq!(accepted.vms.len(), 1);
        {
            let _finished = state.operation_lock("new-env").lock_owned().await;
        }
        let detail = environment_detail(&state, "new-env").unwrap();
        assert_eq!(detail.env.state, EnvState::Active);
        assert_eq!(mock.creates.load(Ordering::SeqCst), 1);
        assert!(create_environment(&state, request()).await.is_err());
        assert_eq!(mock.creates.load(Ordering::SeqCst), 1);
        server.abort();
    }
    #[tokio::test]
    async fn failed_stop_does_not_report_stopped() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        mock.fail_action.store(true, Ordering::SeqCst);
        assert!(change_environment(&state, "env", "stop").await.is_err());
        let detail = environment_detail(&state, "env").unwrap();
        assert_eq!(detail.env.state, EnvState::Failed);
        assert!(detail.vms.iter().all(|vm| vm.state == VmState::Running));
        server.abort();
    }
    #[tokio::test]
    async fn missing_vm_after_controller_restart_remains_visible_for_cleanup() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        mock.vms.lock().unwrap().clear();
        refresh_all_hosts(&state, &agent_client(None, 2).unwrap()).await;
        let detail = environment_detail(&state, "env").unwrap();
        assert_eq!(detail.vms.len(), 2);
        assert_eq!(detail.env.state, EnvState::Failed);
        assert!(detail.vms.iter().all(|vm| vm.error.is_some()));
        let host = state.lock_db().get_host("host").unwrap().unwrap();
        assert_eq!(host.resource.cpu_used, 2);
        assert_eq!(host.resource.mem_used, 256);
        assert!(detail.env.error.is_some());
        server.abort();
    }
    #[tokio::test]
    async fn reject_empty_environment_and_unsupported_options_before_side_effects() {
        let (state, mock, server) = fixture().await;
        let mut req = request();
        req.vms.clear();
        assert_eq!(
            create_environment(&state, req).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        let mut req = request();
        req.vms[0].deny_outgoing = true;
        assert_eq!(
            create_environment(&state, req).await.unwrap_err().0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(state.lock_db().env_count().unwrap(), 0);
        assert_eq!(mock.creates.load(Ordering::SeqCst), 0);
        server.abort();
    }
    #[tokio::test]
    async fn stale_expiry_does_not_delete_a_recreated_permanent_environment() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        let mut env = state.lock_db().get_env("env").unwrap().unwrap();
        env.expires_at = 0;
        state.lock_db().put_env(&env).unwrap();
        expire_environment(&state, "env").await.unwrap();
        assert_eq!(state.lock_db().vm_count().unwrap(), 2);
        assert_eq!(mock.vms.lock().unwrap().len(), 2);
        server.abort();
    }
    #[tokio::test]
    async fn offline_agent_cannot_produce_a_false_successful_stop() {
        let (state, mock, server) = fixture().await;
        seed(&state, &mock);
        apply_host_snapshot(&state, "host", Err("unreachable".into())).unwrap();
        update_env_state(&state, "env").unwrap();
        let detail = environment_detail(&state, "env").unwrap();
        assert_eq!(detail.env.state, EnvState::Failed);
        assert!(detail.env.error.unwrap().contains("offline"));
        assert_eq!(detail.vms.len(), 2);
        server.abort();
    }
}
