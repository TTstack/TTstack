//! HTTP API handlers for the central controller.
//!
//! Handles requests from the CLI and coordinates with host agents.

use crate::db::Db;
use crate::scheduler;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use std::sync::{Arc, Mutex, MutexGuard};
use ttcore::api::*;
use ttcore::model::*;

/// Shared controller state.
pub struct CtlShared {
    pub(crate) db: Mutex<Db>,
    pub operations: Arc<tokio::sync::Mutex<()>>,
    /// API key used for controller→agent communication.
    pub api_key: Option<String>,
}

impl CtlShared {
    pub fn new(db: Db, api_key: Option<String>) -> Self {
        Self {
            db: Mutex::new(db),
            operations: Arc::new(tokio::sync::Mutex::new(())),
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

/// Build an HTTP client for agent communication, with optional Bearer auth.
pub fn agent_client(api_key: Option<&str>, timeout_secs: u64) -> reqwest::Client {
    let mut builder =
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(timeout_secs));
    if let Some(key) = api_key {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        builder = builder.default_headers(headers);
    }
    builder
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

// ── Host Management ─────────────────────────────────────────────────

/// POST /api/hosts — register a new host by its agent address.
pub async fn register_host(
    State(db): State<CtlState>,
    Json(req): Json<RegisterHostReq>,
) -> impl IntoResponse {
    let Ok(_operation) = db.operations.try_lock() else {
        return (
            StatusCode::CONFLICT,
            Json(ApiResp::<Host>::err(
                "another fleet operation is in progress; retry shortly",
            )),
        );
    };
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
    let client = agent_client(db.api_key.as_deref(), 30);
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

    let info: ApiResp<AgentInfo> = match resp.json().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiResp::<Host>::err(format!("invalid agent response: {e}"))),
            );
        }
    };

    let info = match info.data {
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

    if db.host_count().unwrap_or(0) >= MAX_HOSTS {
        return (
            StatusCode::CONFLICT,
            Json(ApiResp::<Host>::err(format!(
                "fleet limit reached ({MAX_HOSTS} hosts)"
            ))),
        );
    }

    let host = Host {
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
    match db.list_hosts() {
        Ok(hosts) => Json(ApiResp::success(hosts)),
        Err(e) => Json(ApiResp::<Vec<Host>>::err(e.to_string())),
    }
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
    let Ok(_operation) = db.operations.try_lock() else {
        return (
            StatusCode::CONFLICT,
            Json(ApiRespEmpty::err(
                "another fleet operation is in progress; retry shortly",
            )),
        );
    };
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
    // Serialize mutations at this scale: fresh snapshots and no competing reservations.
    let _operation = state.operations.clone().lock_owned().await;
    {
        let db = state.lock_db();
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
    let read_client = agent_client(state.api_key.as_deref(), 5);
    refresh_all_hosts(state, &read_client).await;
    let hosts = state.lock_db().list_hosts().map_err(internal)?;
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
        let disk = spec.disk.unwrap_or(spec.engine.default_disk());
        planned.push(Vm {
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
                requested_disk: disk,
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
                disk,
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
    state
        .lock_db()
        .put_environment(&env, &planned)
        .map_err(internal)?;
    let detail = environment_detail(state, &req.id)?;
    let state = state.clone();
    let env_id = req.id;
    tokio::spawn(async move {
        let _operation = _operation;
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
    let read_client = agent_client(state.api_key.as_deref(), 5);
    let client = agent_client(state.api_key.as_deref(), 360);
    for (addr, request) in requests {
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
    refresh_all_hosts(&state, &read_client).await;
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
    let body: ApiResp<CreateVmResp> = resp.json().await.map_err(|e| e.to_string())?;
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
    let _operation = state.operations.clone().lock_owned().await;
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
            db.list_hosts().map_err(internal)?,
        )
    };
    let client = agent_client(state.api_key.as_deref(), 360);
    let mut errors = Vec::new();
    for vm in &vms {
        let result = match hosts.iter().find(|h| h.id == vm.host_id) {
            Some(host) => agent_action(&client, host, &vm.id, None).await,
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
    let body: ApiRespEmpty = resp.json().await.map_err(|e| e.to_string())?;
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
async fn change_environment(state: &CtlState, id: &str, action: &str) -> Result<(), ApiError> {
    let _operation = state.operations.clone().lock_owned().await;
    let detail = environment_detail(state, id)?;
    if matches!(detail.env.state, EnvState::Creating | EnvState::Deleting) {
        return Err((
            StatusCode::CONFLICT,
            "environment has an operation in progress".into(),
        ));
    }
    let hosts = state.lock_db().list_hosts().map_err(internal)?;
    let client = agent_client(state.api_key.as_deref(), 360);
    let mut errors = Vec::new();
    for vm in &detail.vms {
        let result = match hosts.iter().find(|h| h.id == vm.host_id) {
            Some(host) => agent_action(&client, host, &vm.id, Some(action)).await,
            None => Err(format!("host {} is missing", vm.host_id)),
        };
        if let Err(error) = result {
            errors.push(format!("{}: {error}", vm.id));
        }
    }
    refresh_all_hosts(state, &agent_client(state.api_key.as_deref(), 5)).await;
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
    let hosts = db.list_hosts()?;
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
    env.state = if vms.is_empty() {
        EnvState::Failed
    } else if vms.iter().any(|v| v.state == VmState::Creating) {
        EnvState::Creating
    } else if vms.iter().any(|v| v.error.is_some()) {
        EnvState::Failed
    } else if vms.iter().all(|v| v.state == VmState::Running) {
        EnvState::Active
    } else if vms.iter().all(|v| v.state == VmState::Stopped) {
        EnvState::Stopped
    } else {
        EnvState::Failed
    };
    env.vm_ids = vms.iter().map(|v| v.id.clone()).collect();
    if matches!(env.state, EnvState::Active | EnvState::Stopped) {
        env.error = None;
    }
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
    match db.fleet_status() {
        Ok(s) => Json(ApiResp::success(s)),
        Err(e) => Json(ApiResp::<FleetStatus>::err(e.to_string())),
    }
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
    let hosts = match state.lock_db().list_hosts() {
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
                    let info = info.data.filter(|_| info.ok).ok_or("invalid agent info")?;
                    if info.host_id != host.id {
                        return Err("agent identity changed; re-register host".into());
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
                    if let Err(e) = apply_host_snapshot(state, &id, result) {
                        eprintln!("[ctl] save snapshot: {e}");
                    }
                }
                Err(e) => eprintln!("[ctl] snapshot task: {e}"),
            }
        }
    }
    let ids = match state.lock_db().list_envs() {
        Ok(envs) => envs.into_iter().map(|e| e.id).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for id in ids {
        if let Err(e) = update_env_state(state, &id) {
            eprintln!("[ctl] update {id}: {e}");
        }
    }
}

fn apply_host_snapshot(
    state: &CtlState,
    id: &str,
    snapshot: Result<(AgentInfo, Vec<Vm>), String>,
) -> ruc::Result<()> {
    let db = state.lock_db();
    let Some(mut host) = db.get_host(id)? else {
        return Ok(());
    };
    match snapshot {
        Ok((info, actual)) => {
            host.resource = info.resource;
            host.engines = info.engines;
            host.capabilities = info.capabilities;
            host.storage = info.storage;
            host.images = info.images;
            host.state = HostState::Online;
            for mut known in db.vms_by_host(id)? {
                if let Some(vm) = actual
                    .iter()
                    .find(|vm| vm.id == known.id && vm.env_id == known.env_id && vm.host_id == id)
                {
                    db.put_vm(vm)?;
                } else {
                    // Never discard a resource merely because one snapshot is missing it.
                    known.state = VmState::Failed;
                    if known.error.is_none() {
                        known.error = Some(
                            "VM absent from agent snapshot; inspect or delete to reconcile".into(),
                        );
                    }
                    db.put_vm(&known)?;
                }
            }
        }
        Err(error) => {
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
        vms: Mutex<Vec<Vm>>,
        fail_delete: AtomicBool,
        fail_action: AtomicBool,
        lose_create_response: AtomicBool,
        creates: AtomicUsize,
    }
    fn record(id: &str, env: &str) -> Vm {
        Vm {
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
            capabilities: vec![],
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
    async fn fixture() -> (CtlState, Arc<MockAgent>, tokio::task::JoinHandle<()>) {
        let mock = Arc::new(MockAgent {
            vms: Mutex::new(vec![]),
            fail_delete: AtomicBool::new(false),
            fail_action: AtomicBool::new(false),
            lose_create_response: AtomicBool::new(false),
            creates: AtomicUsize::new(0),
        });
        let app = Router::new()
            .route("/api/info", get(info))
            .route("/api/vms", get(vms).post(create))
            .route("/api/vms/{id}", axum::routing::delete(delete))
            .route("/api/vms/{id}/stop", post(stop))
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
    async fn lost_create_response_is_reconciled_without_duplicate_resources() {
        let (state, mock, server) = fixture().await;
        mock.lose_create_response.store(true, Ordering::SeqCst);
        let accepted = create_environment(&state, request()).await.unwrap();
        assert_eq!(accepted.env.state, EnvState::Creating);
        assert_eq!(accepted.env.expires_at, 0);
        assert_eq!(accepted.vms.len(), 1);
        {
            let _finished = state.operations.lock().await;
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
        refresh_all_hosts(&state, &agent_client(None, 2)).await;
        let detail = environment_detail(&state, "env").unwrap();
        assert_eq!(detail.vms.len(), 2);
        assert_eq!(detail.env.state, EnvState::Failed);
        assert!(detail.vms.iter().all(|vm| vm.error.is_some()));
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
