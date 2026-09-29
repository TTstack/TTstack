//! Backup admission/publication. Storage work runs outside the Runtime mutex.
use super::*;
use ttcore::backup::{self, Action, Backend, Generation, Pending, Receipt, Request, View};
use ttcore::storage::backup::{Context, Witness};

#[derive(Clone)]
pub(crate) struct Setup {
    pub vm: Vm,
    storage: Storage,
    root: String,
    locks: std::path::PathBuf,
}

impl Setup {
    pub fn context(&self) -> std::result::Result<Context, String> {
        let vm = &self.vm;
        let store = storage::create_store(self.storage);
        let clone = format!("{}/clone-{}", self.root, vm.id);
        let directory = if vm.engine == Engine::Firecracker && self.storage == Storage::File {
            Some(clone.clone())
        } else {
            None
        };
        let disk = match (self.storage, vm.engine) {
            (Storage::Zvol, Engine::Firecracker) => format!("{clone}/rootfs"),
            (Storage::Zvol, _) => clone,
            (Storage::File, Engine::Firecracker) => {
                format!("{}/rootfs.ext4", directory.as_ref().unwrap())
            }
            (Storage::File, _) => store.resolve_disk(&clone).map_err(|e| e.to_string())?,
        };
        let mut dependencies = vec![];
        if vm.options.ssh.is_some() {
            dependencies.push(ttcore::ssh::seed_path(&vm.id));
        }
        if let Some(dir) = directory {
            dependencies.push(std::path::Path::new(&dir).join("vmlinux"));
            if vm.options.config_disk_mib(vm.engine) > 0 {
                dependencies
                    .push(std::path::Path::new(&dir).join(ttcore::guest_config::CONFIG_DISK));
            }
        }
        // requested_disk is admission history, not a boot dependency. An implicit
        // Firecracker size becomes explicit on a CPU/RAM-only resource update.
        let mut boot_options = vm.options.clone();
        boot_options.requested_disk = 0;
        Ok(Context {
            vm_id: vm.id.clone(),
            engine: vm.engine,
            backend: if self.storage == Storage::Zvol {
                Backend::Zvol
            } else {
                Backend::Reflink
            },
            disk,
            root: self.root.clone().into(),
            dependencies,
            lock_path: self.locks.join(format!("backup-{}.lock", vm.id)),
            options_digest: serde_json::to_string(&boot_options).map_err(|e| e.to_string())?,
            config_disk: vm.options.config_disk_mib(vm.engine) > 0,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(240),
        })
    }
}

impl Runtime {
    pub fn backup_settings(&self) -> (bool, Option<String>) {
        (self.backup_enabled, self.backup_unsupported.clone())
    }
    pub fn configure_backup(&mut self, enabled: bool) {
        self.backup_enabled = enabled;
        self.backup_unsupported = match self.storage {
            Storage::File => {
                ttcore::storage::backup::probe_reflink(std::path::Path::new(&self.runtime_dir))
                    .err()
            }
            Storage::Zvol => (!probe("zfs", &["version"]))
                .then(|| "backup unsupported: ZFS tools unavailable".into()),
        };
    }

    pub fn backup_view(&self, id: &str) -> std::result::Result<View, String> {
        let vm = load_vm(&self.db, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("not found: VM {id}"))?;
        let unsupported_reason = ttcore::capability::backup_unsupported(
            vm.engine,
            self.storage,
            self.backup_unsupported.clone(),
            Some(&self.capability_report()),
        );
        Ok(View {
            vm,
            enabled: self.backup_enabled,
            supported: unsupported_reason.is_none(),
            unsupported_reason,
        })
    }

    /// Excludes lifecycle during read-only qualification before durable admission.
    pub fn backup_claim(
        &mut self,
        id: &str,
        request: Option<&Request>,
    ) -> std::result::Result<Setup, String> {
        let view = self.backup_view(id)?;
        if let Some(request) = request {
            if view.vm.backup.check(request)? {
                return Err("completed backup operation".into());
            }
            if request.action == Action::Create && view.vm.backup.pending.is_none() {
                if !self.backup_enabled {
                    return Err("backup admission disabled".into());
                }
                if let Some(reason) = view.unsupported_reason {
                    return Err(reason);
                }
            }
            ttcore::capability::Feature::Backup
                .require_design(view.vm.engine, self.storage)
                .map_err(|e| format!("backup unsupported: {e}"))?;
            if view.vm.state != VmState::Stopped || view.vm.pending_resources.is_some() {
                return Err(
                    "conflict: backup requires a stopped VM without an unfinished resource update"
                        .into(),
                );
            }
            if (self.engine_factory)(view.vm.engine, self.container_runtime)
                .map_err(|e| e.to_string())?
                .state(&view.vm)
                .map_err(|e| e.to_string())?
                != VmState::Stopped
            {
                return Err("conflict: backup requires an actually stopped VM".into());
            }
        } else if view.vm.backup.pending.is_some() {
            return Err("conflict: backup operation is unfinished".into());
        } else if view.vm.backup.cleanup_retry_at > now() {
            return Err("conflict: backup cleanup is backing off after an error".into());
        }
        if self.backup_running.len() >= 2 || !self.backup_running.insert(id.into()) {
            return Err(
                "conflict: backup worker is running; inspect or retry the same operation".into(),
            );
        }
        Ok(Setup {
            vm: view.vm,
            storage: self.storage,
            root: self.runtime_dir.clone(),
            locks: self.backup_locks.clone(),
        })
    }

    pub fn backup_release(&mut self, id: &str) {
        self.backup_running.remove(id);
    }

    pub fn backup_settled(&self, id: &str, request: &Request) -> Option<u64> {
        if self.backup_running.contains(id) {
            return None;
        }
        let vm = load_vm(&self.db, id).ok()?;
        match vm {
            None => Some(0),
            Some(vm)
                if vm
                    .backup
                    .pending
                    .as_ref()
                    .is_none_or(|p| p.request != *request) =>
            {
                Some(vm.backup.sequence)
            }
            Some(_) => None,
        }
    }

    pub fn backup_admit(
        &mut self,
        id: &str,
        request: &Request,
        witness: &Witness,
    ) -> std::result::Result<Vm, String> {
        let mut vm = self.backup_view(id)?.vm;
        if vm.backup.check(request)? {
            return Err("completed backup operation".into());
        }
        if vm.backup.pending.is_some() {
            return Ok(vm);
        }
        if vm.state != VmState::Stopped || vm.pending_resources.is_some() {
            return Err("conflict: VM changed before backup admission".into());
        }
        let root_mib = u32::try_from(witness.root_bytes.div_ceil(1024 * 1024))
            .map_err(|_| "invalid root capacity")?;
        if vm.disk.checked_sub(vm.options.config_disk_mib(vm.engine)) != Some(root_mib) {
            return Err("conflict: actual disk capacity differs from VM metadata".into());
        }
        match request.action {
            Action::Create if vm.backup.retired.len() >= backup::MAX_RETIRED => {
                return Err("conflict: backup retirement backlog is full; wait for cleanup".into());
            }
            Action::Restore => {
                if vm.backup.retired.iter().any(|g| g.restore_staging) {
                    return Err("conflict: previous restore staging cleanup is unfinished".into());
                }
                let source = vm
                    .backup
                    .current
                    .as_ref()
                    .ok_or("not found: no backup to restore")?;
                if Some(&source.id) != request.generation.as_ref() {
                    return Err("precondition: backup generation changed".into());
                }
                if source.root_bytes != witness.root_bytes
                    || source.dependencies != witness.dependencies
                {
                    return Err(
                        "conflict: root capacity or retained boot dependencies differ from backup"
                            .into(),
                    );
                }
            }
            _ => (),
        }
        self.recount().map_err(|e| e.to_string())?;
        if request.action != Action::Delete && self.resource.disk_free() < root_mib {
            return Err("conflict: insufficient disk budget for backup staging".into());
        }
        vm.backup.pending = Some(Pending {
            request: request.clone(),
            disk_identity: witness.disk_identity.clone(),
            error: None,
            artifact: Generation {
                id: backup::token(),
                backend: if self.storage == Storage::Zvol {
                    Backend::Zvol
                } else {
                    Backend::Reflink
                },
                root_bytes: witness.root_bytes,
                created_at: now(),
                identity: String::new(),
                dependencies: witness.dependencies.clone(),
                restore_staging: request.action == Action::Restore,
            },
        });
        vm.backup.revision = backup::token();
        vm.backup.sequence = vm
            .backup
            .sequence
            .checked_add(1)
            .ok_or("conflict: backup sequence exhausted")?;
        save_vm(&self.db, &vm).map_err(|e| e.to_string())?;
        self.recount().map_err(|e| e.to_string())?;
        Ok(vm)
    }

    pub fn backup_finish(
        &mut self,
        id: &str,
        request: &Request,
        removed: &[String],
        result: std::result::Result<Option<Generation>, String>,
    ) -> std::result::Result<View, String> {
        let mut vm = self.backup_view(id)?.vm;
        let pending = vm
            .backup
            .pending
            .clone()
            .ok_or("conflict: backup intent disappeared")?;
        if pending.request != *request {
            return Err("conflict: backup operation changed".into());
        }
        match result {
            Ok(artifact) => {
                match request.action {
                    Action::Create => {
                        let artifact = artifact.ok_or("backup completed without an artifact")?;
                        if let Some(old) = vm.backup.current.replace(artifact) {
                            vm.backup.retired.push(old);
                        }
                    }
                    Action::Restore => {
                        vm.backup.retired.retain(|g| !removed.contains(&g.id));
                        if let Some(stage) = artifact {
                            vm.backup.retired.push(stage);
                        }
                        if let Some(ssh) = &mut vm.ssh {
                            ssh.initialized = false;
                            ssh.ready = false;
                            ssh.checked_at = 0;
                            ssh.observation_error =
                                Some("SSH identity must be observed after disk restore".into());
                        }
                    }
                    Action::Delete => {
                        vm.backup.current = None;
                        vm.backup.retired.clear();
                    }
                }
                vm.backup.pending = None;
                if vm.backup.retired.is_empty() {
                    vm.backup.cleanup_error = None;
                    vm.backup.cleanup_retry_at = 0;
                    vm.backup.cleanup_failures = 0;
                }
                vm.backup.last_result = Some(Receipt {
                    request: request.clone(),
                    completed_at: now(),
                    error: None,
                });
            }
            Err(error) if request.action == Action::Create => {
                // Create never modifies the active disk or the published backup.
                vm.backup.retired.push(pending.artifact);
                vm.backup.pending = None;
                vm.backup.last_result = Some(Receipt {
                    request: request.clone(),
                    completed_at: now(),
                    error: Some(error),
                });
            }
            Err(error) => vm.backup.pending.as_mut().unwrap().error = Some(error),
        }
        save_vm(&self.db, &vm).map_err(|e| e.to_string())?;
        self.recount().map_err(|e| e.to_string())?;
        self.backup_view(id)
    }

    pub fn backup_cleanup_finish(
        &mut self,
        id: &str,
        removed: &[String],
        error: Option<String>,
    ) -> std::result::Result<(), String> {
        let mut vm = self.backup_view(id)?.vm;
        if vm
            .backup
            .current
            .as_ref()
            .is_some_and(|g| removed.contains(&g.id))
        {
            return Err("conflict: refusing cleanup of current backup".into());
        }
        vm.backup.retired.retain(|g| !removed.contains(&g.id));
        vm.backup.cleanup_error = error;
        if vm.backup.cleanup_error.is_some() {
            vm.backup.cleanup_failures = vm.backup.cleanup_failures.saturating_add(1);
            vm.backup.cleanup_retry_at =
                now().saturating_add(15u64 << vm.backup.cleanup_failures.min(6));
        } else {
            vm.backup.cleanup_failures = 0;
            vm.backup.cleanup_retry_at = 0;
        }
        save_vm(&self.db, &vm).map_err(|e| e.to_string())?;
        self.recount().map_err(|e| e.to_string())
    }

    pub(super) fn backup_guard(&self, vm: &Vm) -> Result<()> {
        if vm.backup.busy() || self.backup_running.contains(&vm.id) {
            return Err(eg!(
                "conflict: backup operation unfinished; inspect it before lifecycle changes"
            ));
        }
        Ok(())
    }

    pub(super) fn delete_backups(&mut self, vm: &Vm) -> Result<()> {
        if !vm.backup.has_artifacts() {
            return Ok(());
        }
        let setup = Setup {
            vm: vm.clone(),
            storage: self.storage,
            root: self.runtime_dir.clone(),
            locks: self.backup_locks.clone(),
        };
        let context = setup.context().map_err(|e| eg!(e))?;
        let lock = context.lock().map_err(|e| eg!(e))?;
        for generation in vm.backup.current.iter().chain(&vm.backup.retired) {
            context.remove(&lock, generation).map_err(|e| eg!(e))?;
        }
        if let Some(pending) = &vm.backup.pending {
            context
                .remove(&lock, &pending.artifact)
                .map_err(|e| eg!(e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Stopped;
    impl engine::VmEngine for Stopped {
        fn create(&self, _: &Vm, _: &str, _: &str, _: &[String]) -> Result<()> {
            Ok(())
        }
        fn start(&self, _: &Vm) -> Result<()> {
            Ok(())
        }
        fn stop(&self, _: &Vm) -> Result<()> {
            Ok(())
        }
        fn destroy(&self, _: &Vm) -> Result<()> {
            Ok(())
        }
        fn state(&self, _: &Vm) -> Result<VmState> {
            Ok(VmState::Stopped)
        }
        fn name(&self) -> &'static str {
            "test"
        }
    }
    fn runtime(path: &std::path::Path) -> Runtime {
        let db = Connection::open(path).unwrap();
        init_db(&db).unwrap();
        Runtime {
            probed_capabilities: ttcore::capability::legacy::ALL
                .iter()
                .map(|s| (*s).into())
                .collect(),
            backup_enabled: false,
            backup_unsupported: None,
            backup_running: HashSet::new(),
            backup_locks: path.parent().unwrap().into(),
            ssh_ingress: None,
            host_id: "host".into(),
            db,
            engines: ttcore::capability::ENGINES.to_vec(),
            store: storage::create_store(Storage::File),
            storage: Storage::File,
            image_dir: String::new(),
            runtime_dir: path.parent().unwrap().to_string_lossy().into(),
            network_ready: false,
            container_runtime: None,
            port_range: 21000..=21999,
            resource: Resource {
                cpu_total: 8,
                mem_total: 8192,
                disk_total: 1024,
                ..Default::default()
            },
            engine_factory: |_, _| Ok(Box::new(Stopped)),
        }
    }
    fn guest() -> Vm {
        serde_json::from_value(serde_json::json!({
            "id":"test-vm", "env_id":"test-env", "host_id":"host", "image":"fixture",
            "engine":"firecracker", "cpu":1, "mem":128, "disk":16,
            "ip":"10.10.0.2", "port_map":{}, "state":"stopped", "created_at":1
        }))
        .unwrap()
    }
    fn witness() -> Witness {
        Witness {
            root_bytes: 16 * 1024 * 1024,
            disk_identity: "device:inode".into(),
            dependencies: "boot-fixture".into(),
        }
    }
    fn request(vm: &Vm, action: Action) -> Request {
        Request {
            operation_id: backup::token(),
            expected_revision: vm.backup.revision.clone(),
            action,
            generation: if action == Action::Restore {
                Some(vm.backup.current.as_ref().unwrap().id.clone())
            } else {
                None
            },
        }
    }
    fn publish(rt: &mut Runtime, vm: &Vm) -> Vm {
        let req = request(vm, Action::Create);
        let admitted = rt.backup_admit(&vm.id, &req, &witness()).unwrap();
        let mut artifact = admitted.backup.pending.as_ref().unwrap().artifact.clone();
        artifact.identity = "owned-artifact".into();
        rt.backup_finish(&vm.id, &req, &[], Ok(Some(artifact)))
            .unwrap()
            .vm
    }

    #[test]
    fn disabled_admission_and_qualification_claim_cannot_change_or_start_a_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(&dir.path().join("agent.db"));
        let vm = guest();
        save_vm(&rt.db, &vm).unwrap();
        let req = request(&vm, Action::Create);
        assert_eq!(
            rt.backup_claim(&vm.id, Some(&req)).err().unwrap(),
            "backup admission disabled"
        );
        assert_eq!(
            load_vm(&rt.db, &vm.id).unwrap().unwrap().backup.revision,
            vm.backup.revision
        );
        assert!(!dir.path().join(".tt-backups").exists());
        rt.backup_enabled = true;
        rt.backup_claim(&vm.id, Some(&req)).unwrap();
        assert!(rt.start_vm(&vm.id).is_err());
        assert!(rt.stop_vm(&vm.id).is_err());
        assert!(rt.destroy_vm(&vm.id).is_err());
        rt.backup_release(&vm.id);
        assert_eq!(
            load_vm(&rt.db, &vm.id).unwrap().unwrap().state,
            VmState::Stopped
        );
    }

    #[test]
    fn normalizing_an_implicit_disk_request_does_not_invalidate_boot_dependencies() {
        let mut setup = Setup {
            vm: guest(),
            storage: Storage::File,
            root: "/unused".into(),
            locks: "/unused".into(),
        };
        let original = setup.context().unwrap().options_digest;
        setup.vm.cpu = 2;
        setup.vm.mem = 256;
        setup.vm.options.requested_disk = 16;
        assert_eq!(setup.context().unwrap().options_digest, original);
        setup.vm.options.deny_outgoing = true;
        assert_ne!(setup.context().unwrap().options_digest, original);
    }

    #[test]
    fn container_and_jail_backups_are_rejected_before_claiming_storage() {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(&dir.path().join("agent.db"));
        rt.backup_enabled = true;
        for engine in [Engine::Docker, Engine::Jail] {
            let mut vm = guest();
            vm.engine = engine;
            // Even a qualified backend cannot turn a container root into a VM disk.
            save_vm(&rt.db, &vm).unwrap();
            let view = rt.backup_view(&vm.id).unwrap();
            assert!(!view.supported);
            assert!(
                view.unsupported_reason
                    .unwrap()
                    .starts_with("backup unsupported:")
            );
            for action in [Action::Create, Action::Restore, Action::Delete] {
                let req = Request {
                    action,
                    generation: (action == Action::Restore).then(backup::token),
                    ..request(&vm, Action::Create)
                };
                assert!(
                    rt.backup_claim(&vm.id, Some(&req))
                        .err()
                        .unwrap()
                        .starts_with("backup unsupported:")
                );
                assert!(rt.backup_running.is_empty());
                assert_eq!(
                    load_vm(&rt.db, &vm.id).unwrap().unwrap().backup.revision,
                    vm.backup.revision
                );
            }
        }
    }

    #[test]
    fn bhyve_zvol_uses_the_shared_snapshot_context_and_lifecycle_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(&dir.path().join("agent.db"));
        rt.backup_enabled = true;
        rt.storage = Storage::Zvol;
        rt.runtime_dir = "pool/runtime".into();
        let mut vm = guest();
        vm.engine = Engine::Bhyve;
        save_vm(&rt.db, &vm).unwrap();
        assert!(rt.backup_view(&vm.id).unwrap().supported);
        let setup = rt
            .backup_claim(&vm.id, Some(&request(&vm, Action::Create)))
            .unwrap();
        let context = setup.context().unwrap();
        assert_eq!(context.backend, Backend::Zvol);
        assert_eq!(context.disk, "pool/runtime/clone-test-vm");
        assert!(context.dependencies.is_empty());
        assert!(rt.start_vm(&vm.id).is_err());
        assert!(rt.stop_vm(&vm.id).is_err());
        assert!(rt.destroy_vm(&vm.id).is_err());
        rt.backup_release(&vm.id);
        assert!(rt.backup_running.is_empty());
    }

    #[test]
    fn failed_refresh_preserves_current_and_failed_cleanup_keeps_reservations() {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(&dir.path().join("agent.db"));
        let vm = guest();
        save_vm(&rt.db, &vm).unwrap();
        let first = publish(&mut rt, &vm);
        assert_eq!(first.reserved_disk(), 32);
        let req = request(&first, Action::Create);
        rt.backup_admit(&vm.id, &req, &witness()).unwrap();
        assert_eq!(rt.resource.disk_used, 48);
        let failed = rt
            .backup_finish(&vm.id, &req, &[], Err("injected snapshot failure".into()))
            .unwrap()
            .vm;
        assert_eq!(failed.backup.current, first.backup.current);
        assert_eq!(failed.backup.retired.len(), 1);
        assert_eq!(failed.backup.check(&req), Ok(true));
        rt.backup_cleanup_finish(&vm.id, &[], Some("held by operator".into()))
            .unwrap();
        let held = rt.backup_view(&vm.id).unwrap().vm;
        assert_eq!(held.reserved_disk(), 48);
        assert!(held.backup.cleanup_retry_at > now());
        let newer = publish(&mut rt, &held);
        assert_ne!(newer.backup.current, first.backup.current);
        assert_eq!(newer.backup.retired.len(), 2);
        assert_eq!(newer.reserved_disk(), 64);
        let refused = request(&newer, Action::Create);
        assert!(
            rt.backup_admit(&vm.id, &refused, &witness())
                .unwrap_err()
                .contains("backlog")
        );
        assert!(
            rt.resize_vm(
                &vm.id,
                VmResources {
                    cpu: 1,
                    mem: 128,
                    disk: 32
                }
            )
            .is_err()
        );
    }

    #[test]
    fn restore_intent_survives_reopen_and_completed_retry_preserves_new_observations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let mut rt = runtime(&path);
        let mut vm = guest();
        vm.ssh = Some(ttcore::ssh::SshInfo {
            observation_error: None,
            user: "root".into(),
            sudo: false,
            host: String::new(),
            port: 22,
            host_key: "initial-public-identity".into(),
            ready: true,
            initialized: true,
            checked_at: 10,
        });
        save_vm(&rt.db, &vm).unwrap();
        let published = publish(&mut rt, &vm);
        let req = request(&published, Action::Restore);
        rt.backup_admit(&vm.id, &req, &witness()).unwrap();
        assert!(rt.start_vm(&vm.id).is_err());
        drop(rt);
        let mut rt = runtime(&path);
        assert!(rt.start_vm(&vm.id).is_err());
        assert_eq!(
            rt.backup_view(&vm.id)
                .unwrap()
                .vm
                .backup
                .pending
                .as_ref()
                .unwrap()
                .request,
            req
        );
        let mut restored = rt.backup_finish(&vm.id, &req, &[], Ok(None)).unwrap().vm;
        assert_eq!(restored.backup.current, published.backup.current);
        let ssh = restored.ssh.as_mut().unwrap();
        assert!(!ssh.ready && !ssh.initialized);
        assert_eq!(ssh.checked_at, 0);
        assert_eq!(ssh.host_key, "initial-public-identity");
        ssh.ready = true;
        ssh.initialized = true;
        ssh.checked_at = 20;
        save_vm(&rt.db, &restored).unwrap();
        assert_eq!(
            rt.backup_view(&vm.id).unwrap().vm.backup.check(&req),
            Ok(true)
        );
        assert_eq!(
            rt.backup_claim(&vm.id, Some(&req)).err().unwrap(),
            "completed backup operation"
        );
        assert_eq!(
            rt.backup_view(&vm.id).unwrap().vm.ssh.unwrap().checked_at,
            20
        );
    }

    #[tokio::test]
    async fn backup_inspection_does_not_wait_for_the_runtime_mutation_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let rt = runtime(&path);
        let engines = [
            Engine::Qemu,
            Engine::Firecracker,
            Engine::Bhyve,
            Engine::Docker,
            Engine::Jail,
        ];
        for engine in engines {
            let mut vm = guest();
            vm.id = engine.to_string();
            vm.engine = engine;
            save_vm(&rt.db, &vm).unwrap();
        }
        rt.db
            .execute(
                "INSERT INTO vms VALUES ('unrelated-corrupt','not-json')",
                [],
            )
            .unwrap();
        let info = rt.agent_info().unwrap();
        let state = std::sync::Arc::new(crate::handler::AgentShared {
            backup_settings: rt.backup_settings(),
            runtime: std::sync::Arc::new(tokio::sync::Mutex::new(rt)),
            db_path: path.to_string_lossy().into(),
            info,
            image_dir: String::new(),
            images: Default::default(),
        });
        let _guard = state.runtime.lock().await;
        for engine in engines {
            let response = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                crate::handler::get_backup(
                    axum::extract::State(state.clone()),
                    axum::extract::Path(engine.to_string()),
                ),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                body["data"]["supported"],
                matches!(engine, Engine::Qemu | Engine::Firecracker)
            );
        }
    }
}
