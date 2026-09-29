//! VM-scoped backup protocol; controller intent survives transport uncertainty.
use super::*;
use axum::http::HeaderMap;
use ttcore::backup::{Action, Request, RestoreBody, View};

fn reply(result: Result<View, ApiError>) -> axum::response::Response {
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
        Err((status, e)) => (status, Json(ApiResp::<()>::err(e))).into_response(),
    }
}
fn invalid(e: String) -> ApiError {
    (
        StatusCode::from_u16(ttcore::backup::error_status(&e)).unwrap(),
        e,
    )
}

fn target(state: &CtlState, id: &str) -> Result<(Vm, Host), ApiError> {
    let db = state.lock_db();
    let vm = db
        .get_vm(id)
        .map_err(internal)?
        .ok_or_else(|| invalid("not found: VM".into()))?;
    let host = db
        .get_host(&vm.host_id)
        .map_err(internal)?
        .ok_or_else(|| internal("VM host missing"))?;
    if host.state != HostState::Online {
        return Err((StatusCode::SERVICE_UNAVAILABLE, "VM host offline".into()));
    }
    if !host.capabilities.iter().any(|c| c == "disk_backup_v1") {
        return Err(invalid(
            "backup unsupported: agent lacks disk_backup_v1; upgrade agent and controller".into(),
        ));
    }
    Ok((vm, host))
}

async fn decode(response: reqwest::Response) -> Result<View, ApiError> {
    let status = response.status();
    let response: ApiResp<View> = response.json().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("backup outcome unknown: invalid agent response: {e}"),
        )
    })?;
    if !status.is_success() || !response.ok {
        return Err((
            if status.is_client_error() {
                status
            } else {
                StatusCode::BAD_GATEWAY
            },
            response
                .error
                .unwrap_or_else(|| "backup outcome unknown: agent failure".into()),
        ));
    }
    response.data.ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "backup outcome unknown: missing result".into(),
        )
    })
}

fn retain(state: &CtlState, expected: &Vm, mut view: View) -> Result<View, ApiError> {
    if view.vm.id != expected.id
        || view.vm.host_id != expected.host_id
        || view.vm.env_id != expected.env_id
    {
        return Err((
            StatusCode::BAD_GATEWAY,
            "backup outcome unknown: agent VM identity mismatch".into(),
        ));
    }
    let db = state.lock_db();
    let known = db
        .get_vm(&expected.id)
        .map_err(internal)?
        .ok_or_else(|| invalid("not found: VM removed during backup".into()))?;
    view.vm = merge_vm_snapshot(&known, &view.vm);
    db.put_backup_observation(&view.vm, known.reserved_disk())
        .map_err(internal)?;
    state.revision.fetch_add(1, Ordering::SeqCst);
    Ok(view)
}

async fn inspect(state: &CtlState, vm: &Vm, host: &Host) -> Result<View, ApiError> {
    let response = agent_client(state.api_key.as_deref(), 30)
        .map_err(internal)?
        .get(format!("http://{}/api/vms/{}/backup", host.addr, vm.id))
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("cannot inspect backup: {e}"),
            )
        })?;
    retain(state, vm, decode(response).await?)
}

pub async fn get_backup(
    State(state): State<CtlState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    reply(
        async {
            let (vm, host) = target(&state, &id)?;
            inspect(&state, &vm, &host).await
        }
        .await,
    )
}
pub async fn create_backup(
    State(state): State<CtlState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    mutation(state, id, headers, Action::Create, None).await
}
pub async fn delete_backup(
    State(state): State<CtlState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> axum::response::Response {
    mutation(state, id, headers, Action::Delete, None).await
}
pub async fn restore_backup(
    State(state): State<CtlState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RestoreBody>,
) -> axum::response::Response {
    mutation(state, id, headers, Action::Restore, Some(body.generation)).await
}

async fn mutation(
    state: CtlState,
    id: String,
    headers: HeaderMap,
    action: Action,
    generation: Option<String>,
) -> axum::response::Response {
    reply(
        async {
            let request = Request::from_headers(
                headers.get("idempotency-key").and_then(|v| v.to_str().ok()),
                headers.get("if-match").and_then(|v| v.to_str().ok()),
                action,
                generation,
            )
            .map_err(invalid)?;
            let (vm, host) = target(&state, &id)?;
            let _operation = begin_operation(&state, &vm.env_id)?;
            let view = inspect(&state, &vm, &host).await?;
            if view.vm.backup.check(&request).map_err(invalid)? {
                return match view
                    .vm
                    .backup
                    .last_result
                    .as_ref()
                    .and_then(|r| r.error.clone())
                {
                    Some(e) => Err(invalid(e)),
                    None => Ok(view),
                };
            }
            if let Some(forwarded) = &view.vm.backup.forwarding
                && *forwarded != request
            {
                return Err(invalid(
                    "conflict: another backup outcome is unresolved".into(),
                ));
            }
            let previous = view.vm;
            let fresh = previous.backup.forwarding.is_none() && previous.backup.pending.is_none();
            if previous.state != VmState::Stopped || previous.pending_resources.is_some() {
                return Err(invalid(
                    "conflict: stop VM and resolve resource updates before backup".into(),
                ));
            }
            if action == Action::Create && fresh {
                if !view.enabled {
                    return Err(invalid("backup admission disabled".into()));
                }
                if !view.supported {
                    return Err(invalid(
                        view.unsupported_reason
                            .unwrap_or_else(|| "backup unsupported: runtime".into()),
                    ));
                }
            }
            {
                let db = state.lock_db();
                let env = db
                    .get_env(&vm.env_id)
                    .map_err(internal)?
                    .ok_or_else(|| invalid("not found: environment".into()))?;
                if matches!(env.state, EnvState::Creating | EnvState::Deleting) {
                    return Err(invalid(
                        "conflict: environment lifecycle operation unfinished".into(),
                    ));
                }
                let mut saved = db
                    .get_vm(&id)
                    .map_err(internal)?
                    .ok_or_else(|| invalid("not found: VM".into()))?;
                if fresh {
                    let reserve = if action == Action::Delete {
                        0
                    } else {
                        saved
                            .disk
                            .saturating_sub(saved.options.config_disk_mib(saved.engine))
                    };
                    let mut host = db
                        .get_host(&host.id)
                        .map_err(internal)?
                        .ok_or_else(|| internal("host missing"))?;
                    if host.resource.disk_free() < reserve {
                        return Err(invalid("conflict: insufficient backup disk budget".into()));
                    }
                    saved.backup.forwarding_reserved_mib = reserve;
                    host.resource.disk_used = host
                        .resource
                        .disk_used
                        .checked_add(reserve)
                        .ok_or_else(|| invalid("conflict: backup reservation overflow".into()))?;
                    db.put_host(&host).map_err(internal)?;
                }
                saved.backup.forwarding = Some(request.clone());
                db.put_vm(&saved).map_err(internal)?;
                state.revision.fetch_add(1, Ordering::SeqCst);
            }
            let method = match action {
                Action::Create => reqwest::Method::PUT,
                Action::Restore => reqwest::Method::POST,
                Action::Delete => reqwest::Method::DELETE,
            };
            let suffix = if action == Action::Restore {
                "/restore"
            } else {
                ""
            };
            let client = agent_client(state.api_key.as_deref(), 360).map_err(internal)?;
            let mut outbound = client
                .request(
                    method,
                    format!("http://{}/api/vms/{id}/backup{suffix}", host.addr),
                )
                .header("Idempotency-Key", &request.operation_id)
                .header("If-Match", format!("\"{}\"", request.expected_revision));
            if let Some(generation) = &request.generation {
                outbound = outbound.json(&RestoreBody {
                    generation: generation.clone(),
                });
            }
            let mut settled = None;
            let result = match outbound.send().await {
                Ok(response) => {
                    let matches = response
                        .headers()
                        .get("x-tt-backup-settled")
                        .and_then(|v| v.to_str().ok())
                        == Some(request.operation_id.as_str());
                    if matches {
                        settled = response
                            .headers()
                            .get("x-tt-backup-settled-sequence")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.parse::<u64>().ok());
                    }
                    decode(response).await
                }
                Err(e) => Err((
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "backup outcome unknown; inspect and retry operation {}: {e}",
                        request.operation_id
                    ),
                )),
            };
            match result {
                Ok(view) => retain(&state, &vm, view),
                Err(error) => {
                    if let Some(sequence) = settled {
                        let db = state.lock_db();
                        if let Some(mut saved) = db.get_vm(&id).map_err(internal)?
                            && saved.backup.forwarding.as_ref() == Some(&request)
                        {
                            // A settled writer may leave retired artifacts. Keep
                            // reservation until an equally new inventory is read.
                            saved.backup.forwarding_settled_sequence = Some(sequence);
                            db.put_vm(&saved).map_err(internal)?;
                            state.revision.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    // Reconciliation resolves terminal receipts without losing unknown intent.
                    let _ = inspect(&state, &vm, &host).await;
                    Err(error)
                }
            }
        }
        .await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use ttcore::backup::{Backend, Generation, Receipt};

    #[derive(Clone)]
    struct FakeAgent {
        view: Arc<Mutex<View>>,
        writes: Arc<AtomicUsize>,
        reject: Arc<std::sync::atomic::AtomicBool>,
    }
    async fn fake_get(State(agent): State<FakeAgent>) -> Json<ApiResp<View>> {
        Json(ApiResp::success(agent.view.lock().unwrap().clone()))
    }
    async fn fake_restore(
        State(agent): State<FakeAgent>,
        headers: HeaderMap,
        Json(body): Json<RestoreBody>,
    ) -> axum::response::Response {
        let request = Request::from_headers(
            headers.get("idempotency-key").and_then(|v| v.to_str().ok()),
            headers.get("if-match").and_then(|v| v.to_str().ok()),
            Action::Restore,
            Some(body.generation),
        )
        .unwrap();
        if agent.reject.load(AtomicOrdering::SeqCst) {
            return (
                StatusCode::CONFLICT,
                [
                    ("x-tt-backup-settled", request.operation_id),
                    (
                        "x-tt-backup-settled-sequence",
                        agent.view.lock().unwrap().vm.backup.sequence.to_string(),
                    ),
                ],
                Json(ApiResp::<View>::err(
                    "conflict: injected admission rejection",
                )),
            )
                .into_response();
        }
        let mut view = agent.view.lock().unwrap();
        assert!(!view.vm.backup.check(&request).unwrap());
        assert_eq!(
            request.generation.as_deref(),
            view.vm.backup.current.as_ref().map(|g| g.id.as_str())
        );
        agent.writes.fetch_add(1, AtomicOrdering::SeqCst);
        view.vm.backup.sequence += 1;
        view.vm.backup.revision = ttcore::backup::token();
        view.vm.backup.last_result = Some(Receipt {
            request,
            completed_at: 1,
            error: None,
        });
        (
            StatusCode::BAD_GATEWAY,
            Json(ApiResp::<View>::err(
                "injected response loss after disk write",
            )),
        )
            .into_response()
    }
    fn guest() -> Vm {
        let mut vm: Vm = serde_json::from_value(serde_json::json!({
            "id":"vm", "env_id":"env", "host_id":"host", "image":"fixture", "engine":"qemu",
            "cpu":1, "mem":128, "disk":16, "ip":"10.10.0.2", "port_map":{}, "state":"stopped", "created_at":1
        })).unwrap();
        vm.backup.current = Some(Generation {
            id: ttcore::backup::token(),
            backend: Backend::Reflink,
            root_bytes: 16 * 1024 * 1024,
            created_at: 1,
            identity: "file".into(),
            dependencies: "boot".into(),
            restore_staging: false,
        });
        vm
    }

    #[tokio::test]
    async fn forwarded_restore_lost_reply_replay_and_headers_are_safe() {
        let vm = guest();
        let fake = FakeAgent {
            view: Arc::new(Mutex::new(View {
                vm: vm.clone(),
                enabled: false,
                supported: true,
                unsupported_reason: None,
            })),
            writes: Arc::new(AtomicUsize::new(0)),
            reject: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route("/api/vms/{id}/backup", axum::routing::get(fake_get))
            .route(
                "/api/vms/{id}/backup/restore",
                axum::routing::post(fake_restore),
            )
            .with_state(fake.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let db = Db::open(":memory:").unwrap();
        let host: Host = serde_json::from_value(serde_json::json!({
            "id":"host", "addr":addr.to_string(), "state":"online", "engines":["qemu"],
            "storage":"file", "registered_at":1, "capabilities":["disk_backup_v1"],
            "resource":{"cpu_total":8,"cpu_used":0,"mem_total":8192,"mem_used":0,"disk_total":1024,"disk_used":32,"vm_count":1}
        })).unwrap();
        db.put_host(&host).unwrap();
        let env: Env = serde_json::from_value(serde_json::json!({
            "id":"env","owner":"test","vm_ids":["vm"],"created_at":1,"expires_at":0,"state":"stopped"
        })).unwrap();
        db.put_environment(&env, std::slice::from_ref(&vm)).unwrap();
        let state = Arc::new(CtlShared::new(db, None));
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", ttcore::backup::token().parse().unwrap());
        let missing = mutation(
            state.clone(),
            vm.id.clone(),
            headers.clone(),
            Action::Restore,
            Some(vm.backup.current.as_ref().unwrap().id.clone()),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::PRECONDITION_REQUIRED);
        headers.insert(
            "if-match",
            format!("\"{}\"", vm.backup.revision).parse().unwrap(),
        );
        let generation = Some(vm.backup.current.as_ref().unwrap().id.clone());
        let lost = mutation(
            state.clone(),
            vm.id.clone(),
            headers.clone(),
            Action::Restore,
            generation.clone(),
        )
        .await;
        assert_eq!(lost.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(fake.writes.load(AtomicOrdering::SeqCst), 1);
        assert!(
            !state
                .lock_db()
                .get_vm(&vm.id)
                .unwrap()
                .unwrap()
                .backup
                .busy()
        );
        let replay = mutation(
            state.clone(),
            vm.id.clone(),
            headers.clone(),
            Action::Restore,
            generation,
        )
        .await;
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(fake.writes.load(AtomicOrdering::SeqCst), 1);
        let changed = mutation(
            state.clone(),
            vm.id.clone(),
            headers,
            Action::Restore,
            Some(ttcore::backup::token()),
        )
        .await;
        assert_eq!(changed.status(), StatusCode::CONFLICT);
        assert_eq!(fake.writes.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            state
                .lock_db()
                .get_host("host")
                .unwrap()
                .unwrap()
                .resource
                .disk_used,
            32
        );
        fake.reject.store(true, AtomicOrdering::SeqCst);
        let mut headers = HeaderMap::new();
        headers.insert("idempotency-key", ttcore::backup::token().parse().unwrap());
        headers.insert(
            "if-match",
            format!("\"{}\"", fake.view.lock().unwrap().vm.backup.revision)
                .parse()
                .unwrap(),
        );
        let rejected = mutation(
            state.clone(),
            vm.id.clone(),
            headers,
            Action::Restore,
            vm.backup.current.as_ref().map(|g| g.id.clone()),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        assert!(
            !state
                .lock_db()
                .get_vm(&vm.id)
                .unwrap()
                .unwrap()
                .backup
                .busy()
        );
        assert_eq!(
            state
                .lock_db()
                .get_host("host")
                .unwrap()
                .unwrap()
                .resource
                .disk_used,
            32
        );
        assert_eq!(fake.writes.load(AtomicOrdering::SeqCst), 1);
        server.abort();
    }

    #[test]
    fn old_observations_cannot_erase_intent_or_restore_completion() {
        let old = guest();
        let mut known = old.clone();
        let request = Request {
            operation_id: ttcore::backup::token(),
            expected_revision: old.backup.revision.clone(),
            action: Action::Restore,
            generation: old.backup.current.as_ref().map(|g| g.id.clone()),
        };
        known.backup.forwarding = Some(request.clone());
        known.backup.forwarding_reserved_mib = 16;
        assert!(merge_vm_snapshot(&known, &old).backup.busy());
        known.backup.forwarding_settled_sequence = Some(2);
        assert_eq!(merge_vm_snapshot(&known, &old).reserved_disk(), 48);
        let mut settled = old.clone();
        settled.backup.sequence = 2;
        let mut retired = settled.backup.current.clone().unwrap();
        retired.id = ttcore::backup::token();
        settled.backup.retired.push(retired);
        let observed = merge_vm_snapshot(&known, &settled);
        assert!(!observed.backup.busy());
        assert_eq!(
            observed.reserved_disk(),
            48,
            "settled work can retain an artifact reservation"
        );
        known.backup.forwarding_settled_sequence = None;
        known.backup.sequence = 1;
        known.backup.revision = ttcore::backup::token();
        known.backup.forwarding = None;
        known.backup.last_result = Some(Receipt {
            request: request.clone(),
            completed_at: 1,
            error: None,
        });
        assert_eq!(merge_vm_snapshot(&known, &old).backup.sequence, 1);
        let mut pending = known.clone();
        pending.backup.last_result = None;
        pending.backup.pending = Some(ttcore::backup::Pending {
            request,
            artifact: known.backup.current.clone().unwrap(),
            disk_identity: "old".into(),
            error: None,
        });
        assert!(merge_vm_snapshot(&known, &pending).backup.pending.is_none());
    }
}
