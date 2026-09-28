//! Durable VM lifecycle management for one host.
use ruc::*;
use rusqlite::{Connection, OptionalExtension};
use std::collections::{BTreeMap, HashSet};
use ttcore::api::{AgentInfo, CreateVmReq};
use ttcore::engine;
use ttcore::engine::docker::ContainerRuntime;
use ttcore::model::*;
use ttcore::net;
use ttcore::storage::{self, ImageStore};

pub struct Runtime {
    pub ssh_ingress: Option<crate::ssh_ingress::SshIngress>,
    pub host_id: String,
    db: Connection,
    engines: Vec<Engine>,
    store: Box<dyn ImageStore>,
    storage: Storage,
    image_dir: String,
    runtime_dir: String,
    network_ready: bool,
    container_runtime: Option<ContainerRuntime>,
    port_range: std::ops::RangeInclusive<u16>,
    pub resource: Resource,
    engine_factory: fn(Engine, Option<ContainerRuntime>) -> Result<Box<dyn engine::VmEngine>>,
}

impl Runtime {
    pub fn new(
        host_id: String,
        storage: Storage,
        image_dir: String,
        runtime_dir: String,
        db_path: &str,
        resource: Resource,
    ) -> Result<Self> {
        if storage == Storage::File {
            std::fs::create_dir_all(&image_dir).c(d!("create image_dir"))?;
            std::fs::create_dir_all(&runtime_dir).c(d!("create runtime_dir"))?;
        }
        std::fs::create_dir_all(RUN_DIR).c(d!("create run dir"))?;
        let db = Connection::open(db_path).c(d!("open agent DB"))?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .c(d!("agent DB timeout"))?;
        init_db(&db)?;
        let mut rt = Self {
            ssh_ingress: None,
            host_id,
            db,
            engines: detect_engines(None),
            store: storage::create_store(storage),
            storage,
            image_dir,
            runtime_dir,
            resource,
            engine_factory: engine::create_engine,
            network_ready: false,
            container_runtime: None,
            port_range: 20000..=65535,
        };
        // Reconcile retries network recovery; a transient firewall error must not
        // prevent the management listener from starting.
        for mut vm in load_recoverable_vms(&rt.db)?.0 {
            if vm.engine != Engine::Docker
                && matches!(vm.state, VmState::Running | VmState::Paused)
                && vm.error.is_none()
            {
                vm.error = Some("network recovery pending".into());
                save_vm(&rt.db, &vm)?;
            }
        }
        rt.recount()?;
        Ok(rt)
    }

    pub fn configure_container_runtime(
        &mut self,
        requested: Option<ContainerRuntime>,
    ) -> Result<()> {
        self.container_runtime = bind_container_runtime(&self.db, requested, || {
            [ContainerRuntime::Docker, ContainerRuntime::Podman]
                .into_iter()
                .find(|runtime| runtime.available())
        })?;
        self.engines.retain(|engine| *engine != Engine::Docker);
        if self
            .container_runtime
            .is_some_and(ContainerRuntime::available)
        {
            self.engines.push(Engine::Docker);
        }
        Ok(())
    }

    pub fn configure_network(
        &mut self,
        ingress: Option<crate::ssh_ingress::SshIngress>,
        start: u16,
        end: u16,
    ) -> Result<()> {
        if start == 0 || start > end {
            return Err(eg!(
                "host port range must satisfy 1 <= port-start <= port-end <= 65535"
            ));
        }
        if let Some(settings) = &ingress {
            settings.validate()?;
        }
        let requested =
            serde_json::json!({"ingress": ingress, "port_start": start, "port_end": end});
        let stored: Option<String> = self
            .db
            .query_row(
                "SELECT value FROM _meta WHERE key='network_config'",
                [],
                |row| row.get(0),
            )
            .optional()
            .c(d!("read network binding"))?;
        let occupied: bool = self
            .db
            .query_row("SELECT EXISTS(SELECT 1 FROM vms)", [], |row| row.get(0))
            .c(d!("check network inventory"))?;
        if let Some(saved) = stored {
            let saved: serde_json::Value =
                serde_json::from_str(&saved).c(d!("decode network binding"))?;
            if occupied && saved != requested {
                return Err(eg!(
                    "cannot change SSH ingress or host port range while VM records remain; restore the saved configuration and delete those VMs first"
                ));
            }
        } else if occupied {
            return Err(eg!(
                "agent database has VM records but no network binding; inspect the database before starting"
            ));
        }
        self.db
            .execute(
                "INSERT OR REPLACE INTO _meta (key,value) VALUES ('network_config',?1)",
                [requested.to_string()],
            )
            .c(d!("persist network binding"))?;
        self.ssh_ingress = ingress;
        self.port_range = start..=end;
        Ok(())
    }

    pub fn create_vm(&mut self, req: &CreateVmReq) -> Result<Vm> {
        validate_name(&req.vm_id, "vm_id").map_err(|e| eg!(e))?;
        validate_name(&req.env_id, "env_id").map_err(|e| eg!(e))?;
        validate_image(&req.image, req.engine).map_err(|e| eg!(e))?;
        ttcore::guest_config::validate(req.engine, &req.guest_config, req.isolated_network)
            .map_err(|e| eg!(e))?;
        validate_vm_options(
            req.engine,
            (req.disk > 0).then_some(req.disk),
            req.deny_outgoing,
            &req.ssh_keys,
            &req.ports,
        )
        .map_err(|e| eg!(e))?;
        if !self.engines.contains(&req.engine) {
            return Err(eg!("engine {} is unavailable on this host", req.engine));
        }
        if self.storage == Storage::Zvol && req.engine == Engine::Jail {
            return Err(eg!("Jail requires file storage"));
        }
        if req.engine == Engine::Qemu && req.disk == 0 {
            return Err(eg!("QEMU disk size must be > 0"));
        }
        if req.cpu == 0 || req.mem == 0 {
            return Err(eg!("cpu and memory must be > 0"));
        }
        let ssh = ttcore::ssh::resolve_options(req.engine, req.ssh.as_ref(), &req.ssh_keys)
            .map_err(|e| eg!(e))?;
        if req.guest_config.contains_key(ttcore::ssh::SEED_FILE) {
            return Err(eg!("reserved guest configuration filename"));
        }
        let mut options = VmOptions {
            ssh,
            ports: req.ports.clone(),
            ssh_keys: req.ssh_keys.clone(),
            deny_outgoing: req.deny_outgoing,
            requested_disk: req.disk,
            isolated_network: req.isolated_network,
            guest_config_digest: ttcore::guest_config::digest(&req.guest_config),
        };
        options.ports.sort_unstable();
        options.ports.dedup();
        options.ssh_keys.sort();
        options.ssh_keys.dedup();
        if let Some(mut vm) = load_vm(&self.db, &req.vm_id)? {
            vm.options.ports.sort_unstable();
            vm.options.ports.dedup();
            vm.options.ssh_keys.sort();
            vm.options.ssh_keys.dedup();
            if vm.env_id != req.env_id
                || vm.image != req.image
                || vm.engine != req.engine
                || vm.cpu != req.cpu
                || vm.mem != req.mem
                || vm.options != options
            {
                return Err(eg!(
                    "VM ID already exists with different creation parameters"
                ));
            }
            if matches!(vm.state, VmState::Failed | VmState::Deleting) {
                return Err(eg!(
                    "VM {} is {}; inspect and delete it before recreating",
                    vm.id,
                    vm.state
                ));
            }
            return Ok(vm);
        }
        self.recount()?;
        let base_image = format!("{}/{}", self.image_dir, req.image);
        if req.engine != Engine::Docker && !self.store.image_exists(&base_image)? {
            return Err(eg!("base image does not exist"));
        }
        let disk = match req.engine {
            Engine::Docker | Engine::Bhyve | Engine::Jail => 0,
            Engine::Firecracker => {
                let bytes = self.store.firecracker_size(&base_image)?;
                let base_mib =
                    u32::try_from(bytes.div_ceil(1024 * 1024)).c(d!("rootfs too large"))?;
                if req.disk != 0 && req.disk < base_mib {
                    return Err(eg!(format!(
                        "requested disk is smaller than the base image ({base_mib} MiB)"
                    )));
                }
                req.disk
                    .max(base_mib)
                    .checked_add(options.config_disk_mib(req.engine))
                    .ok_or_else(|| eg!("rootfs and configuration disk are too large"))?
            }
            Engine::Qemu => {
                let base_mib = self.store.qemu_size(&base_image)?.div_ceil(1024 * 1024);
                if u64::from(req.disk) < base_mib {
                    return Err(eg!(format!(
                        "requested disk is smaller than the base image ({base_mib} MiB)"
                    )));
                }
                req.disk
            }
        };
        if !self
            .resource
            .can_fit(req.cpu, req.engine.memory_reservation(req.mem), disk)
        {
            return Err(eg!("insufficient resources on host {}", self.host_id));
        }
        if self.resource.vm_count as usize >= MAX_VMS {
            return Err(eg!("VM limit reached"));
        }
        let vms = self.list_vms()?;
        let used_ips: HashSet<_> = vms.iter().map(|v| v.ip.as_str()).collect();
        let ip = if req.engine == Engine::Docker {
            String::new()
        } else {
            (0..65000)
                .map(net::vm_ip)
                .find(|ip| !used_ips.contains(ip.as_str()))
                .ok_or_else(|| eg!("IP address space exhausted"))?
        };
        let mut ports = req.ports.clone();
        if (matches!(req.engine, Engine::Qemu | Engine::Bhyve | Engine::Jail)
            || options.ssh.is_some())
            && !ports.contains(&22)
        {
            ports.push(22);
        }
        let port_map = allocate_ports(&vms, &ports, self.port_range.clone(), |p| {
            std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, p)).is_ok()
        })?;
        let mut vm = Vm {
            ssh: None,
            pending_resources: None,
            id: req.vm_id.clone(),
            env_id: req.env_id.clone(),
            host_id: self.host_id.clone(),
            image: req.image.clone(),
            engine: req.engine,
            cpu: req.cpu,
            mem: req.mem,
            disk,
            ip,
            port_map,
            options,
            error: None,
            state: VmState::Creating,
            created_at: now(),
        };
        // Reserve identity, ports and resources before the first external side effect.
        save_vm(&self.db, &vm)?;
        self.recount()?;
        let result = (|| -> Result<()> {
            let mut guest_config = req.guest_config.clone();
            if let Some(settings) = &vm.options.ssh {
                let (script, host_key) = ttcore::ssh::bootstrap(settings, &vm.options.ssh_keys)?;
                use std::io::Write;
                let mut file =
                    tempfile::NamedTempFile::new_in(RUN_DIR).c(d!("SSH seed staging"))?;
                file.write_all(script.as_bytes())
                    .c(d!("write initial SSH seed"))?;
                file.as_file().sync_all().c(d!("sync initial SSH seed"))?;
                file.persist(ttcore::ssh::seed_path(&vm.id))
                    .map_err(|e| eg!(e.error.to_string()))?;
                vm.ssh = Some(ttcore::ssh::SshInfo {
                    user: settings.user.clone(),
                    sudo: settings.sudo,
                    host: self
                        .ssh_ingress
                        .as_ref()
                        .map(|c| c.public_address.to_string())
                        .unwrap_or_default(),
                    port: vm.port_map[&22],
                    host_key,
                    checked_at: 0,
                    initialized: false,
                    ready: false,
                });
                save_vm(&self.db, &vm)?;
                if req.engine == Engine::Firecracker {
                    guest_config.insert(ttcore::ssh::SEED_FILE.into(), script);
                }
            }
            if req.engine != Engine::Docker {
                self.ensure_network()?;
                let clone_path = self.clone_path(&vm);
                self.store.clone_image(&base_image, &clone_path)?;
                #[cfg(target_os = "linux")]
                if vm.engine == Engine::Firecracker {
                    if req.disk > 0 {
                        self.store.resize_firecracker(&clone_path, req.disk)?;
                    }
                    let directory = self.store.firecracker_dir(&clone_path)?;
                    if vm.options.ssh.is_some() {
                        ttcore::ssh::prepared_rootfs(
                            &std::path::Path::new(&directory).join("rootfs.ext4"),
                        )?;
                    }
                    ttcore::guest_config::write_disk(
                        std::path::Path::new(&directory),
                        &guest_config,
                    )?;
                }
                if vm.engine == Engine::Qemu {
                    self.store.resize_disk(&clone_path, vm.disk)?;
                }
                self.restore_network(&vm)?;
            }
            let path = self.image_path(&vm)?;
            (self.engine_factory)(vm.engine, self.container_runtime)?.create(
                &vm,
                &path,
                self.store.disk_format(),
                &vm.options.ssh_keys,
            )?;
            Ok(())
        })();
        if let Err(e) = result {
            // Preserve the record, including partial resources, for reliable cleanup.
            vm.state = VmState::Failed;
            vm.error = Some(e.to_string());
            save_vm(&self.db, &vm)?;
            return Err(e);
        }
        vm.state = VmState::Running;
        save_vm(&self.db, &vm)?;
        Ok(vm)
    }

    fn ensure_network(&mut self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if !self.network_ready {
            net::setup_bridge()?;
            net::setup_nat()?;
            self.network_ready = true;
        }
        Ok(())
    }

    fn clone_path(&self, vm: &Vm) -> String {
        format!("{}/clone-{}", self.runtime_dir, vm.id)
    }
    fn image_path(&self, vm: &Vm) -> Result<String> {
        let path = self.clone_path(vm);
        match vm.engine {
            Engine::Firecracker => self.store.firecracker_dir(&path),
            Engine::Qemu | Engine::Bhyve => self.store.resolve_disk(&path),
            Engine::Jail => Ok(path),
            Engine::Docker => Ok(vm.image.clone()),
        }
    }
    fn restore_network(&self, vm: &Vm) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if vm.engine != Engine::Docker {
            if vm.engine == Engine::Jail && vm.options.deny_outgoing {
                return Err(eg!(
                    "Jail does not support deny_outgoing; retained guest requires operator review"
                ));
            }
            #[cfg(target_os = "linux")]
            if vm.options.isolated_network {
                net::isolate(&vm.id, &vm.ip)?;
            }
            if vm.engine != Engine::Jail
                && matches!(vm.state, VmState::Running | VmState::Paused)
                && !net::tap_exists(&vm.id)?
            {
                return Err(eg!(
                    "live VM tap is missing; stop/start the VM to attach a new tap"
                ));
            }
            if vm.engine != Engine::Jail {
                net::create_tap(&vm.id, &vm.ip)?;
            }
            net::remove_port_forwards(&vm.ip)?;
            for (&guest, &host) in &vm.port_map {
                net::add_port_forward(host, &vm.ip, guest)?;
            }
            if vm.options.deny_outgoing {
                net::deny_outgoing(&vm.ip)?;
            } else {
                net::allow_outgoing(&vm.ip)?;
            }
        }
        if let Some(ingress) = &self.ssh_ingress {
            ingress.apply(vm)?;
        }
        Ok(())
    }

    pub fn stop_vm(&mut self, id: &str) -> Result<()> {
        let mut vm = load_vm(&self.db, id)?.ok_or_else(|| eg!(format!("VM not found: {id}")))?;
        if vm.state == VmState::Stopped {
            return Ok(());
        }
        if matches!(vm.state, VmState::Creating | VmState::Deleting) {
            return Err(eg!("VM is {}", vm.state));
        }
        match (self.engine_factory)(vm.engine, self.container_runtime)?.stop(&vm) {
            Ok(()) => {
                vm.state = VmState::Stopped;
                if let Some(ssh) = &mut vm.ssh {
                    ssh.ready = false;
                    ssh.checked_at = now();
                }
                vm.error = None;
            }
            Err(e) => {
                vm.error = Some(e.to_string());
                save_vm(&self.db, &vm)?;
                return Err(e);
            }
        }
        save_vm(&self.db, &vm)?;
        self.recount()
    }

    pub fn start_vm(&mut self, id: &str) -> Result<()> {
        let mut vm = load_vm(&self.db, id)?.ok_or_else(|| eg!(format!("VM not found: {id}")))?;
        if vm.pending_resources.is_some() {
            return Err(eg!(
                "resource update unfinished; retry the recorded resources before starting"
            ));
        }
        if vm.state == VmState::Running {
            return Ok(());
        }
        if !matches!(vm.state, VmState::Stopped | VmState::Paused) {
            return Err(eg!("cannot start VM in state {}", vm.state));
        }
        self.recount()?;
        if vm.state == VmState::Stopped
            && !self
                .resource
                .can_fit(vm.cpu, vm.engine.memory_reservation(vm.mem), 0)
        {
            return Err(eg!("insufficient resources to restart VM"));
        }
        let previous = vm.state;
        vm.state = VmState::Creating;
        save_vm(&self.db, &vm)?;
        let result = (|| -> Result<()> {
            if vm.engine != Engine::Docker {
                self.ensure_network()?;
            }
            self.restore_network(&vm)?;
            let eng = (self.engine_factory)(vm.engine, self.container_runtime)?;
            if previous == VmState::Stopped
                && matches!(
                    vm.engine,
                    Engine::Qemu | Engine::Firecracker | Engine::Bhyve | Engine::Jail
                )
            {
                eng.create(
                    &vm,
                    &self.image_path(&vm)?,
                    self.store.disk_format(),
                    &vm.options.ssh_keys,
                )
            } else {
                eng.start(&vm)
            }
        })();
        match result {
            Ok(()) => {
                vm.state = VmState::Running;
                vm.error = None;
            }
            Err(e) => {
                vm.state = VmState::Failed;
                vm.error = Some(e.to_string());
                save_vm(&self.db, &vm)?;
                self.recount()?;
                return Err(e);
            }
        }
        save_vm(&self.db, &vm)?;
        self.recount()
    }

    /// Change a stopped VM without replacing its disk or identity.
    pub fn resize_vm(&mut self, id: &str, target: VmResources) -> Result<Vm> {
        let mut vm = load_vm(&self.db, id)?.ok_or_else(|| eg!(format!("VM not found: {id}")))?;
        if !matches!(vm.engine, Engine::Qemu | Engine::Firecracker) {
            return Err(eg!("resource updates require QEMU or Firecracker"));
        }
        if target.cpu == 0 || target.mem == 0 || target.disk == 0 {
            return Err(eg!("cpu, memory and root disk must be > 0"));
        }
        if vm.state != VmState::Stopped
            || (self.engine_factory)(vm.engine, self.container_runtime)?.state(&vm)?
                != VmState::Stopped
        {
            return Err(eg!("resource updates require a confirmed stopped VM"));
        }
        if vm
            .pending_resources
            .is_some_and(|pending| pending != target)
        {
            return Err(eg!(
                "resource update unfinished; retry the recorded target first"
            ));
        }
        let path = self.clone_path(&vm);
        let current = if vm.engine == Engine::Qemu {
            self.store.qemu_size(&path)?
        } else {
            self.store.firecracker_size(&path)?
        }
        .div_ceil(1024 * 1024);
        let overhead = vm.options.config_disk_mib(vm.engine);
        if u64::from(target.disk) < current
            || target.disk < vm.disk.saturating_sub(overhead)
            || target.disk < vm.options.requested_disk
        {
            return Err(eg!("disk shrinking is not supported"));
        }
        let disk = target
            .disk
            .checked_add(overhead)
            .ok_or_else(|| eg!("disk capacity overflow"))?;
        self.recount()?;
        if !self.resource.can_fit(
            target.cpu,
            vm.engine.memory_reservation(target.mem),
            disk.saturating_sub(vm.reserved_disk()),
        ) {
            return Err(eg!("insufficient resources for resource update"));
        }
        if vm.pending_resources.is_none()
            && vm.cpu == target.cpu
            && vm.mem == target.mem
            && vm.options.requested_disk == target.disk
            && vm.disk == disk
        {
            return Ok(vm);
        }
        // Reserve the larger disk and record intent before touching the filesystem.
        // Old CPU/memory/requested_disk remain observable until the operation succeeds.
        let grow = u64::from(target.disk) > current || vm.pending_resources.is_some();
        vm.pending_resources = Some(target);
        vm.error = Some("resource update unfinished; retry the recorded target".into());
        save_vm(&self.db, &vm)?;
        self.recount()?;
        if let Err(e) = if grow {
            if vm.engine == Engine::Qemu {
                self.store.resize_disk(&path, target.disk)
            } else {
                self.store.resize_firecracker(&path, target.disk)
            }
        } else {
            Ok(())
        } {
            vm.error = Some(format!("resource update unfinished: {e}"));
            save_vm(&self.db, &vm)?;
            return Err(e);
        }
        vm.cpu = target.cpu;
        vm.mem = target.mem;
        vm.disk = disk;
        vm.options.requested_disk = target.disk;
        vm.pending_resources = None;
        vm.error = None;
        save_vm(&self.db, &vm)?;
        self.recount()?;
        Ok(vm)
    }

    pub fn destroy_vm(&mut self, id: &str) -> Result<()> {
        let Some(mut vm) = load_vm(&self.db, id)? else {
            return Ok(());
        };
        vm.state = VmState::Deleting;
        save_vm(&self.db, &vm)?;
        // Disk deletion is safe only after process termination is confirmed.
        let engine = (self.engine_factory)(vm.engine, self.container_runtime)?;
        if let Err(e) = engine.stop(&vm) {
            vm.error = Some(e.to_string());
            save_vm(&self.db, &vm)?;
            return Err(e);
        }
        let mut errors = Vec::new();
        let mut collect = |result: Result<()>| {
            if let Err(e) = result {
                errors.push(e.to_string());
            }
        };
        collect(engine.destroy(&vm));
        if let Some(ingress) = &self.ssh_ingress {
            collect(ingress.remove(id));
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if vm.engine != Engine::Docker {
            collect(net::remove_port_forwards(&vm.ip));
            collect(net::allow_outgoing(&vm.ip));
            if vm.engine != Engine::Jail {
                collect(net::destroy_tap(id));
            }
            #[cfg(target_os = "linux")]
            if vm.options.isolated_network {
                collect(net::remove_isolation(id));
            }
        }
        if vm.engine != Engine::Docker {
            collect(self.store.remove_image(&self.clone_path(&vm)));
        }
        if !errors.is_empty() {
            vm.error = Some(errors.join("; "));
            save_vm(&self.db, &vm)?;
            return Err(eg!(errors.join("; ")));
        }
        match std::fs::remove_file(ttcore::ssh::seed_path(id)) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(eg!(e.to_string())),
        }
        delete_vm(&self.db, id)?;
        self.recount()
    }

    /// Re-read under the mutation lock; a queued scan must not resurrect a deleted VM.
    pub fn reconcile_vm(&mut self, id: &str) -> Result<()> {
        let Some(mut vm) = load_vm(&self.db, id)? else {
            return Ok(());
        };
        if vm.state == VmState::Deleting {
            return self.destroy_vm(&vm.id);
        }
        let needs_recovery = vm.error.is_some();
        let was_running = matches!(vm.state, VmState::Running | VmState::Paused);
        match (self.engine_factory)(vm.engine, self.container_runtime)
            .and_then(|eng| eng.state(&vm))
        {
            Ok(actual) => {
                if vm
                    .error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("cannot query engine:"))
                {
                    vm.error = None;
                }
                if vm.state == VmState::Creating
                    && !matches!(actual, VmState::Running | VmState::Paused)
                {
                    vm.state = VmState::Failed;
                    vm.error = Some(
                        "operation interrupted; delete this VM to clean up and recreate".into(),
                    );
                } else if vm.state != VmState::Failed
                    || matches!(actual, VmState::Running | VmState::Paused)
                {
                    vm.state = actual;
                }
                if actual == VmState::Stopped
                    && vm.pending_resources.is_none()
                    && vm.error.as_deref().is_some_and(|error| {
                        error == "network recovery pending"
                            || error.starts_with("network recovery failed:")
                    })
                {
                    vm.error = None;
                }
                if matches!(actual, VmState::Running | VmState::Paused)
                    && (!was_running || needs_recovery || !self.network_ready)
                {
                    let recovery = if vm.engine == Engine::Docker {
                        Ok(())
                    } else {
                        self.ensure_network()
                            .and_then(|_| self.restore_network(&vm))
                    };
                    match recovery {
                        Ok(()) => vm.error = None,
                        Err(e) => {
                            self.network_ready = false;
                            vm.error = Some(format!("network recovery failed: {e}"));
                        }
                    }
                }
            }
            Err(e) => {
                vm.error = Some(format!("cannot query engine: {e}"));
            }
        }
        if let Some(ssh) = &mut vm.ssh {
            ssh.host = self
                .ssh_ingress
                .as_ref()
                .map(|c| c.public_address.to_string())
                .unwrap_or_default();
            let reachable = vm.state == VmState::Running && ttcore::ssh::ready(&vm.ip);
            if reachable && !ssh.initialized {
                ssh.initialized = ttcore::ssh::initial_identity_ready(&vm.ip, &ssh.host_key);
            }
            ssh.ready = reachable && ssh.initialized;
            ssh.checked_at = now();
        }
        save_vm(&self.db, &vm)?;
        Ok(())
    }

    #[cfg(test)]
    fn reconcile(&mut self) -> Result<()> {
        for vm in load_recoverable_vms(&self.db)?.0 {
            self.reconcile_vm(&vm.id)?;
        }
        self.recount()
    }

    fn recount(&mut self) -> Result<()> {
        let (vms, corrupt) = load_recoverable_vms(&self.db)?;
        self.resource = resources_for(&self.resource, &vms);
        if corrupt {
            // Unknown allocations cannot be treated as free; keep admission closed.
            self.resource.cpu_used = self.resource.cpu_total;
            self.resource.mem_used = self.resource.mem_total;
            self.resource.disk_used = self.resource.disk_total;
            self.resource.vm_count = MAX_VMS as u32;
        }
        Ok(())
    }
    pub fn list_vms(&self) -> Result<Vec<Vm>> {
        load_all_vms(&self.db)
    }
    pub fn agent_info(&self) -> Result<AgentInfo> {
        Ok(AgentInfo {
            vms: None,
            warnings: vec![],
            image_sizes: Default::default(),
            capabilities: detect_capabilities(&self.engines, self.storage),
            host_id: self.host_id.clone(),
            resource: self.resource.clone(),
            engines: self.engines.clone(),
            storage: self.storage,
            // The background catalog worker fills these after the listener starts.
            images: vec![],
        })
    }
}

pub fn resources_for(totals: &Resource, vms: &[Vm]) -> Resource {
    let mut resource = Resource {
        cpu_total: totals.cpu_total,
        mem_total: totals.mem_total,
        disk_total: totals.disk_total,
        ..Default::default()
    };
    for vm in vms {
        resource.account(vm);
    }
    resource
}

/// Readers use a separate SQLite connection; slow mutations never hold a read lock.
pub fn read_vms(db_path: &str) -> Result<Vec<Vm>> {
    let db = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .c(d!("open agent snapshot"))?;
    db.busy_timeout(std::time::Duration::from_secs(2)).c(d!())?;
    load_all_vms(&db)
}

pub fn read_vm(db_path: &str, id: &str) -> Result<Option<Vm>> {
    let db = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .c(d!("open VM snapshot"))?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .c(d!("snapshot timeout"))?;
    load_vm(&db, id)
}

pub fn read_recovery_snapshot(db_path: &str) -> Result<(Vec<Vm>, bool)> {
    let db = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .c(d!("open agent recovery snapshot"))?;
    db.busy_timeout(std::time::Duration::from_secs(2))
        .c(d!("snapshot timeout"))?;
    load_recoverable_vms(&db)
}

fn allocate_ports(
    vms: &[Vm],
    ports: &[u16],
    range: std::ops::RangeInclusive<u16>,
    available: impl Fn(u16) -> bool,
) -> Result<BTreeMap<u16, u16>> {
    let used: HashSet<u16> = vms
        .iter()
        .flat_map(|v| v.port_map.values().copied())
        .collect();
    let mut free = range.filter(|p| !used.contains(p) && available(*p));
    let mut result = BTreeMap::new();
    for &guest in ports {
        if result.contains_key(&guest) {
            continue;
        }
        let port = free
            .next()
            .ok_or_else(|| eg!("host TCP port pool exhausted"))?;
        result.insert(guest, port);
    }
    Ok(result)
}

// ── SQLite Schema & Operations ──────────────────────────────────────

fn bind_container_runtime(
    db: &Connection,
    requested: Option<ContainerRuntime>,
    detect: impl FnOnce() -> Option<ContainerRuntime>,
) -> Result<Option<ContainerRuntime>> {
    let stored: Option<String> = db
        .query_row(
            "SELECT value FROM _meta WHERE key='container_runtime'",
            [],
            |row| row.get(0),
        )
        .optional()
        .c(d!("read container runtime binding"))?;
    let bound = stored
        .as_deref()
        .map(str::parse::<ContainerRuntime>)
        .transpose()
        .map_err(|e| eg!(e))?;
    let (vms, corrupt) = load_recoverable_vms(db)?;
    let containers = vms.iter().any(|vm| vm.engine == Engine::Docker);
    if bound.is_none() && containers {
        return Err(eg!(
            "agent database has container records but no runtime binding; inspect the database before starting"
        ));
    }
    let occupied = corrupt || containers;
    let selected = match (bound, requested) {
        (None, Some(_)) if corrupt => {
            return Err(eg!(
                "cannot select a container runtime while unreadable records remain"
            ));
        }
        (Some(old), Some(new)) if old != new && occupied => {
            return Err(eg!(
                "cannot change the container runtime while container or unreadable records remain; retain {} and drain them first",
                old.command()
            ));
        }
        (_, Some(runtime)) | (Some(runtime), None) => Some(runtime),
        (None, None) if occupied => None,
        (None, None) => detect(),
    };
    if let Some(runtime) = selected {
        db.execute(
            "INSERT OR REPLACE INTO _meta (key,value) VALUES ('container_runtime',?1)",
            [runtime.command()],
        )
        .c(d!("persist container runtime binding"))?;
    }
    Ok(selected)
}

/// The only supported agent database format. There is no in-agent migration.
const SCHEMA_VERSION: u32 = 5;

fn init_db(db: &Connection) -> Result<()> {
    let populated: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*')",
            [],
            |row| row.get(0),
        )
        .c(d!("inspect agent database format"))?;
    if populated {
        let current = get_schema_version(db)?;
        if current != SCHEMA_VERSION {
            return Err(eg!(
                "unsupported agent DB schema v{current}; expected v{SCHEMA_VERSION}; database migration or cleanup belongs to the caller's deployment script"
            ));
        }
        // A damaged native database is not an invitation to recreate missing tables.
        db.prepare("SELECT id, data FROM vms")
            .c(d!("validate agent VM table"))?;
    } else {
        let transaction = db
            .unchecked_transaction()
            .c(d!("initialize agent database"))?;
        transaction
            .execute_batch(
                "CREATE TABLE _meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE vms (
                id       TEXT PRIMARY KEY,
                data     TEXT NOT NULL
            );",
            )
            .c(d!("create native agent schema"))?;
        transaction
            .execute(
                "INSERT INTO _meta (key,value) VALUES ('schema_version',?1)",
                [SCHEMA_VERSION.to_string()],
            )
            .c(d!("record agent database format"))?;
        transaction
            .commit()
            .c(d!("commit agent database initialization"))?;
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .c(d!("configure agent database"))?;
    Ok(())
}

fn get_schema_version(db: &Connection) -> Result<u32> {
    let value: Option<String> = db.query_row(
        "SELECT value FROM _meta WHERE key='schema_version'", [], |row| row.get(0)
    ).optional().c(d!("read agent database format; incompatible state must be handled by the caller's deployment script"))?;
    value
        .ok_or_else(|| {
            eg!("agent database has no schema marker; deployment must supply a native database")
        })?
        .parse()
        .c(d!("invalid agent database schema marker"))
}

/// Load or generate a stable host_id persisted in the agent database.
///
/// If `--host-id` is provided on the CLI, that value takes precedence and
/// is saved for future restarts. Otherwise we check the database; only
/// when neither is available do we generate a new random ID.
pub fn resolve_host_id(db_path: &str, cli_id: Option<String>) -> Result<String> {
    let conn = Connection::open(db_path).c(d!("open DB for host_id"))?;
    init_db(&conn)?;

    if let Some(id) = cli_id {
        validate_name(&id, "host_id").map_err(|e| eg!(e))?;
        // CLI takes precedence — persist it
        conn.execute(
            "INSERT OR REPLACE INTO _meta (key, value) VALUES ('host_id', ?1)",
            rusqlite::params![id],
        )
        .c(d!("persist CLI host_id"))?;
        return Ok(id);
    }

    // Try to load from DB
    let mut stmt = conn
        .prepare("SELECT value FROM _meta WHERE key = 'host_id'")
        .c(d!())?;
    let mut rows = stmt.query([]).c(d!())?;
    if let Some(row) = rows.next().c(d!())? {
        let val: String = row.get(0).c(d!())?;
        return Ok(val);
    }
    drop(rows);
    drop(stmt);

    // Generate and persist a new ID
    let id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    conn.execute(
        "INSERT OR REPLACE INTO _meta (key, value) VALUES ('host_id', ?1)",
        rusqlite::params![id],
    )
    .c(d!("persist generated host_id"))?;
    Ok(id)
}

fn save_vm(db: &Connection, vm: &Vm) -> Result<()> {
    let data = serde_json::to_string(vm).c(d!("serialize VM"))?;
    db.execute(
        "INSERT OR REPLACE INTO vms (id, data) VALUES (?1, ?2)",
        rusqlite::params![vm.id, data],
    )
    .c(d!("save VM"))?;
    Ok(())
}

fn load_vm(db: &Connection, id: &str) -> Result<Option<Vm>> {
    let mut stmt = db
        .prepare("SELECT data FROM vms WHERE id = ?1")
        .c(d!("prepare load VM"))?;
    let mut rows = stmt.query(rusqlite::params![id]).c(d!("query VM"))?;
    match rows.next().c(d!("next row"))? {
        Some(row) => {
            let data: String = row.get(0).c(d!("get data"))?;
            let vm: Vm = serde_json::from_str(&data).c(d!("deserialize VM"))?;
            Ok(Some(vm))
        }
        None => Ok(None),
    }
}

fn load_all_vms(db: &Connection) -> Result<Vec<Vm>> {
    let mut stmt = db
        .prepare("SELECT data FROM vms")
        .c(d!("prepare list VMs"))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .c(d!("query all VMs"))?;
    let mut vms = Vec::new();
    for row in rows {
        let data = row.c(d!("read row"))?;
        let vm: Vm = serde_json::from_str(&data).c(d!("deserialize VM"))?;
        vms.push(vm);
    }
    Ok(vms)
}

fn load_recoverable_vms(db: &Connection) -> Result<(Vec<Vm>, bool)> {
    let mut stmt = db
        .prepare("SELECT id,data FROM vms")
        .c(d!("read recovery records"))?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .c(d!("recovery records"))?;
    let mut vms = Vec::new();
    let mut corrupt = false;
    for row in rows {
        let (id, data) = row.c(d!("recovery row"))?;
        match serde_json::from_str::<Vm>(&data) {
            Ok(vm) if vm.id == id => vms.push(vm),
            _ => {
                corrupt = true;
                eprintln!("[agent] unreadable VM record {id}; retained, admission disabled");
            }
        }
    }
    Ok((vms, corrupt))
}

fn delete_vm(db: &Connection, id: &str) -> Result<()> {
    db.execute("DELETE FROM vms WHERE id = ?1", rusqlite::params![id])
        .c(d!("delete VM"))?;
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn probe(cmd: &str, args: &[&str]) -> bool {
    use ttcore::command::CommandExt;
    std::process::Command::new(cmd)
        .args(args)
        .output_timeout(std::time::Duration::from_secs(5))
        .is_ok_and(|o| o.status.success())
}
fn detect_engines(container_runtime: Option<ContainerRuntime>) -> Vec<Engine> {
    let mut engines = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let kvm = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm")
            .is_ok();
        if kvm && probe("qemu-system-x86_64", &["--version"]) && probe("qemu-img", &["--version"]) {
            engines.push(Engine::Qemu);
        }
        if kvm && probe("firecracker", &["--version"]) && probe("jailer", &["--version"]) {
            engines.push(Engine::Firecracker);
        }
    }
    #[cfg(target_os = "freebsd")]
    {
        let vmm = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/vmmctl")
            .is_ok();
        if vmm
            && ["bhyve", "bhyveload", "bhyvectl"]
                .iter()
                .all(|tool| probe("which", &[tool]))
        {
            engines.push(Engine::Bhyve);
        }
        let sysctl = |key: &str| -> Option<u32> {
            use ttcore::command::CommandExt;
            let out = std::process::Command::new("sysctl")
                .args(["-n", key])
                .bounded_output()
                .ok()?;
            out.status.success().then_some(())?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        };
        let can_create_jail = sysctl("security.jail.jailed") == Some(0)
            || sysctl("security.jail.children.max").is_some_and(|n| n > 0);
        if can_create_jail && probe("which", &["jail"]) && probe("which", &["jls"]) {
            engines.push(Engine::Jail);
        }
    }

    if container_runtime.is_some_and(ContainerRuntime::available) {
        engines.push(Engine::Docker);
    }
    engines
}
fn detect_capabilities(engines: &[Engine], storage: Storage) -> Vec<String> {
    use ttcore::command::CommandExt;
    let mut caps = Vec::new();
    if cfg!(target_os = "freebsd") && engines.contains(&Engine::Bhyve) {
        caps.push("bhyve_deny_outgoing".into());
    }
    if cfg!(target_os = "linux") {
        if (engines.contains(&Engine::Qemu) || engines.contains(&Engine::Firecracker))
            && ["ssh-keygen", "ssh-keyscan"].iter().all(|tool| {
                std::process::Command::new(tool)
                    .arg("-?")
                    .output_timeout(std::time::Duration::from_secs(5))
                    .is_ok()
            })
        {
            caps.push("ssh_bootstrap".into());
        }

        if engines.contains(&Engine::Qemu)
            && (storage == Storage::File || probe("zfs", &["version"]))
        {
            caps.push("qemu_resources".into());
        }
        if probe("nft", &["--version"]) && probe("ip", &["-Version"]) {
            caps.push("isolated_network".into());
        }
        if engines.contains(&Engine::Firecracker) {
            let controllers =
                std::fs::read_to_string("/sys/fs/cgroup/cgroup.controllers").unwrap_or_default();
            if ["cpu", "memory", "pids"]
                .iter()
                .all(|c| controllers.split_whitespace().any(|v| v == *c))
            {
                caps.push("firecracker_jailer".into());
            }
            if probe("mkfs.ext4", &["-V"]) {
                caps.push("guest_config".into());
            }
            if probe("e2fsck", &["-V"]) && probe("which", &["resize2fs"]) {
                caps.push("firecracker_disk_resize".into());
                caps.push("firecracker_resources".into());
            }
            if storage == Storage::Zvol && probe("zfs", &["version"]) {
                caps.push("firecracker_zvol".into());
            }
        }
    }
    caps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let db = Connection::open(":memory:").unwrap();
        init_db(&db).unwrap();
        db
    }

    fn make_vm(id: &str, state: VmState) -> Vm {
        Vm {
            ssh: None,
            pending_resources: None,
            id: id.into(),
            env_id: "env1".into(),
            host_id: "h1".into(),
            image: "ubuntu".into(),
            engine: Engine::Qemu,
            cpu: 2,
            mem: 1024,
            disk: 40960,
            ip: "10.10.0.2".into(),
            port_map: BTreeMap::new(),
            options: VmOptions::default(),
            error: None,
            state,
            created_at: 1000,
        }
    }

    #[test]
    fn db_save_and_load_vm() {
        let db = test_db();
        let vm = make_vm("vm1", VmState::Running);
        save_vm(&db, &vm).unwrap();

        let loaded = load_vm(&db, "vm1").unwrap().unwrap();
        assert_eq!(loaded.id, "vm1");
        assert_eq!(loaded.state, VmState::Running);
        assert_eq!(loaded.cpu, 2);
    }

    #[test]
    fn retained_engines_survive_database_reopen_and_unknown_engine_keeps_its_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("agent.db");
        let engines = [
            Engine::Qemu,
            Engine::Firecracker,
            Engine::Docker,
            Engine::Bhyve,
            Engine::Jail,
        ];
        {
            let db = Connection::open(&path).unwrap();
            init_db(&db).unwrap();
            for engine in engines {
                let mut vm = make_vm(&engine.to_string(), VmState::Stopped);
                vm.engine = engine;
                vm.options.requested_disk = 8192;
                save_vm(&db, &vm).unwrap();
            }
        }
        let db = Connection::open(&path).unwrap();
        init_db(&db).unwrap();
        for engine in engines {
            let vm = load_vm(&db, &engine.to_string()).unwrap().unwrap();
            assert_eq!(vm.engine, engine);
            assert_eq!(vm.state, VmState::Stopped);
            assert_eq!(vm.options.requested_disk, 8192);
        }
        let mut unknown = serde_json::to_value(make_vm("unknown", VmState::Stopped)).unwrap();
        unknown["engine"] = serde_json::json!("unsupported-engine");
        let original = unknown.to_string();
        db.execute(
            "INSERT INTO vms (id,data) VALUES (?1,?2)",
            rusqlite::params!["unknown", &original],
        )
        .unwrap();
        assert!(load_vm(&db, "unknown").is_err());
        assert!(load_all_vms(&db).is_err());
        let stored: String = db
            .query_row("SELECT data FROM vms WHERE id='unknown'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, original);
    }

    #[test]
    fn db_load_nonexistent_vm() {
        let db = test_db();
        let result = load_vm(&db, "nope").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn db_load_all_vms() {
        let db = test_db();
        save_vm(&db, &make_vm("a", VmState::Running)).unwrap();
        save_vm(&db, &make_vm("b", VmState::Stopped)).unwrap();
        let all = load_all_vms(&db).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn db_delete_vm() {
        let db = test_db();
        save_vm(&db, &make_vm("vm1", VmState::Running)).unwrap();
        delete_vm(&db, "vm1").unwrap();
        assert!(load_vm(&db, "vm1").unwrap().is_none());
    }

    #[test]
    fn db_delete_nonexistent_vm() {
        let db = test_db();
        // Should not error
        delete_vm(&db, "nope").unwrap();
    }

    #[test]
    fn db_upsert_vm() {
        let db = test_db();
        let mut vm = make_vm("vm1", VmState::Creating);
        save_vm(&db, &vm).unwrap();

        vm.state = VmState::Running;
        save_vm(&db, &vm).unwrap();

        let loaded = load_vm(&db, "vm1").unwrap().unwrap();
        assert_eq!(loaded.state, VmState::Running);
        // Only one row
        assert_eq!(load_all_vms(&db).unwrap().len(), 1);
    }

    #[test]
    fn db_schema_version_persisted() {
        let db = test_db();
        let ver = get_schema_version(&db).unwrap();
        assert_eq!(ver, SCHEMA_VERSION);
    }

    #[test]
    fn incompatible_databases_are_rejected_before_any_identity_or_schema_write() {
        for marker in [
            None,
            Some("0"),
            Some("1"),
            Some("2"),
            Some("3"),
            Some("4"),
            Some("6"),
            Some("invalid"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("agent.db");
            {
                let db = Connection::open(&path).unwrap();
                db.execute_batch(
                    "CREATE TABLE _meta (key TEXT PRIMARY KEY,value TEXT NOT NULL);
                    CREATE TABLE vms (id TEXT PRIMARY KEY,data TEXT NOT NULL);
                    INSERT INTO _meta VALUES ('host_id','original-host');
                    INSERT INTO vms VALUES ('retained','original-payload');",
                )
                .unwrap();
                if let Some(marker) = marker {
                    db.execute("INSERT INTO _meta VALUES ('schema_version',?1)", [marker])
                        .unwrap();
                }
            }
            let before = std::fs::read(&path).unwrap();
            assert!(
                resolve_host_id(path.to_str().unwrap(), Some("replacement-host".into())).is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let db = Connection::open(&path).unwrap();
            assert_eq!(
                db.query_row("SELECT value FROM _meta WHERE key='host_id'", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                "original-host"
            );
            assert_eq!(
                db.query_row("SELECT data FROM vms WHERE id='retained'", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "original-payload"
            );
        }
    }

    #[test]
    fn unversioned_or_incomplete_databases_are_not_automatically_repaired() {
        // Only SQLite's reserved sqlite_ prefix denotes internal objects.
        let named = Connection::open_in_memory().unwrap();
        named
            .execute_batch("CREATE TABLE sqliteCustom (value TEXT);")
            .unwrap();
        assert!(init_db(&named).is_err());
        assert_eq!(
            named
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='_meta'",
                    [],
                    |r| r.get::<_, u32>(0)
                )
                .unwrap(),
            0
        );

        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE vms (id TEXT PRIMARY KEY,data TEXT NOT NULL);
            INSERT INTO vms VALUES ('retained','original-payload');",
        )
        .unwrap();
        assert!(init_db(&db).is_err());
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='_meta'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM vms", [], |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );

        let db = test_db();
        db.execute_batch("DROP TABLE vms;").unwrap();
        assert!(init_db(&db).is_err());
        assert_eq!(get_schema_version(&db).unwrap(), SCHEMA_VERSION);
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='vms'",
                [],
                |r| r.get::<_, u32>(0)
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn container_runtime_binding_survives_reopen_and_rejects_switching() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let db = Connection::open(&path).unwrap();
        init_db(&db).unwrap();
        let mut vm = make_vm("container", VmState::Running);
        vm.engine = Engine::Docker;
        assert_eq!(
            bind_container_runtime(&db, Some(ContainerRuntime::Podman), || panic!(
                "must not detect"
            ))
            .unwrap(),
            Some(ContainerRuntime::Podman)
        );
        save_vm(&db, &vm).unwrap();
        drop(db);
        let db = Connection::open(path).unwrap();
        init_db(&db).unwrap();
        assert_eq!(
            bind_container_runtime(&db, None, || Some(ContainerRuntime::Docker)).unwrap(),
            Some(ContainerRuntime::Podman)
        );
        assert!(bind_container_runtime(&db, Some(ContainerRuntime::Docker), || None).is_err());
        assert!(load_vm(&db, &vm.id).unwrap().is_some());
        assert_eq!(
            bind_container_runtime(&db, None, || None).unwrap(),
            Some(ContainerRuntime::Podman)
        );
        delete_vm(&db, &vm.id).unwrap();
        assert_eq!(
            bind_container_runtime(&db, Some(ContainerRuntime::Docker), || None).unwrap(),
            Some(ContainerRuntime::Docker)
        );
        db.execute("DELETE FROM _meta WHERE key='container_runtime'", [])
            .unwrap();
        assert_eq!(
            bind_container_runtime(&db, None, || Some(ContainerRuntime::Podman)).unwrap(),
            Some(ContainerRuntime::Podman)
        );
        db.execute("INSERT INTO vms VALUES ('broken','not-json')", [])
            .unwrap();
        assert!(bind_container_runtime(&db, Some(ContainerRuntime::Docker), || None).is_err());
    }

    // ── resolve_host_id ────────────────────────────────────────────

    #[test]
    fn resolve_host_id_generates_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let path_str = path.to_str().unwrap();

        let id1 = resolve_host_id(path_str, None).unwrap();
        assert!(!id1.is_empty());
        assert!(id1.len() <= 8);

        // Second call returns the same persisted ID
        let id2 = resolve_host_id(path_str, None).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn resolve_host_id_cli_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let path_str = path.to_str().unwrap();

        let id1 = resolve_host_id(path_str, None).unwrap();
        let id2 = resolve_host_id(path_str, Some("custom-id".into())).unwrap();
        assert_eq!(id2, "custom-id");
        assert_ne!(id1, id2);

        // Persisted the CLI override
        let id3 = resolve_host_id(path_str, None).unwrap();
        assert_eq!(id3, "custom-id");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ttcore::engine::VmEngine;
    static CREATES: AtomicUsize = AtomicUsize::new(0);
    struct FakeEngine;
    impl VmEngine for FakeEngine {
        fn create(&self, _: &Vm, _: &str, _: &str, _: &[String]) -> Result<()> {
            CREATES.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn start(&self, _: &Vm) -> Result<()> {
            Ok(())
        }
        fn stop(&self, _: &Vm) -> Result<()> {
            Ok(())
        }
        fn destroy(&self, vm: &Vm) -> Result<()> {
            if vm.image == "fail-delete" {
                Err(eg!("injected deletion failure"))
            } else {
                Ok(())
            }
        }
        fn state(&self, vm: &Vm) -> Result<VmState> {
            if vm.image == "live" {
                Ok(VmState::Running)
            } else {
                Ok(VmState::Stopped)
            }
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }
    fn runtime(db: Connection) -> Runtime {
        init_db(&db).unwrap();
        Runtime {
            ssh_ingress: None,
            host_id: "h1".into(),
            db,
            engines: vec![Engine::Docker],
            store: storage::create_store(Storage::File),
            storage: Storage::File,
            image_dir: String::new(),
            runtime_dir: String::new(),
            resource: Resource {
                cpu_total: 8,
                mem_total: 8192,
                disk_total: 100000,
                ..Default::default()
            },
            engine_factory: |_, _| Ok(Box::new(FakeEngine)),
            container_runtime: None,
            port_range: 20000..=65535,
            network_ready: false,
        }
    }
    /// Fault injection at the storage boundary, without changing process-wide PATH.
    struct QemuTestStore {
        fail_after_growth: bool,
    }
    impl storage::ImageStore for QemuTestStore {
        fn clone_image(&self, _: &str, _: &str) -> Result<()> {
            unreachable!()
        }
        fn remove_image(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        fn list_images(&self, _: &str) -> Result<Vec<String>> {
            unreachable!()
        }
        fn image_exists(&self, _: &str) -> Result<bool> {
            unreachable!()
        }
        fn resolve_disk(&self, path: &str) -> Result<String> {
            Ok(path.into())
        }
        fn disk_format(&self) -> &'static str {
            "qcow2"
        }
        fn name(&self) -> &'static str {
            "qemu-test"
        }
        fn qemu_size(&self, path: &str) -> Result<u64> {
            Ok(std::fs::metadata(path).c(d!())?.len())
        }
        fn resize_disk(&self, path: &str, size: u32) -> Result<()> {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .c(d!())?
                .set_len(u64::from(size) * 1024 * 1024)
                .c(d!())?;
            if self.fail_after_growth {
                return Err(eg!("injected failure after disk growth"));
            }
            Ok(())
        }
    }
    #[test]
    fn qemu_resources_recover_growth_after_reopen_and_allow_cpu_memory_reduction() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("agent.db");
        let mut rt = runtime(Connection::open(&database).unwrap());
        rt.runtime_dir = dir.path().to_str().unwrap().into();
        rt.store = Box::new(QemuTestStore {
            fail_after_growth: true,
        });
        let mut guest = vm("resize-qemu", "offline", VmState::Stopped);
        guest.engine = Engine::Qemu;
        guest.disk = 64;
        guest.options.requested_disk = 64;
        guest.options.ssh_keys = vec!["ssh-ed25519 AAAA retained".into()];
        guest.options.deny_outgoing = true;
        guest.ip = "10.10.0.2".into();
        guest.port_map.insert(22, 20000);
        save_vm(&rt.db, &guest).unwrap();
        let disk = rt.clone_path(&guest);
        std::fs::write(&disk, b"retained").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&disk)
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
        let target = VmResources {
            cpu: 4,
            mem: 2048,
            disk: 96,
        };
        assert!(rt.resize_vm(&guest.id, target).is_err());
        let pending = load_vm(&rt.db, &guest.id).unwrap().unwrap();
        assert_eq!(pending.pending_resources, Some(target));
        assert_eq!((pending.cpu, pending.mem, pending.disk), (2, 1024, 64));
        assert_eq!(rt.resource.disk_used, 96);
        assert_eq!(rt.resource.cpu_used, 0);
        assert!(rt.start_vm(&guest.id).is_err());
        assert!(
            rt.resize_vm(&guest.id, VmResources { cpu: 1, ..target })
                .is_err()
        );
        drop(rt);

        let mut rt = runtime(Connection::open(&database).unwrap());
        rt.runtime_dir = dir.path().to_str().unwrap().into();
        rt.store = Box::new(QemuTestStore {
            fail_after_growth: false,
        });
        let done = rt.resize_vm(&guest.id, target).unwrap();
        assert_eq!((done.cpu, done.mem, done.disk), (4, 2048, 96));
        assert_eq!(done.state, VmState::Stopped);
        assert!(done.pending_resources.is_none());
        assert!(done.error.is_none());
        assert_eq!(done.id, guest.id);
        assert_eq!(done.ip, guest.ip);
        assert_eq!(done.port_map, guest.port_map);
        assert_eq!(done.options.ssh_keys, guest.options.ssh_keys);
        assert!(done.options.deny_outgoing);
        let mut marker = [0; 8];
        std::fs::File::open(&disk)
            .unwrap()
            .read_exact(&mut marker)
            .unwrap();
        assert_eq!(&marker, b"retained");
        assert!(rt.resize_vm(&guest.id, target).is_ok());
        // CPU/RAM-only updates must not invoke disk growth.
        rt.store = Box::new(QemuTestStore {
            fail_after_growth: true,
        });
        let reduced = rt
            .resize_vm(
                &guest.id,
                VmResources {
                    cpu: 1,
                    mem: 256,
                    ..target
                },
            )
            .unwrap();
        assert_eq!((reduced.cpu, reduced.mem, reduced.disk), (1, 256, 96));
        for invalid in [
            VmResources { cpu: 0, ..target },
            VmResources { mem: 0, ..target },
            VmResources { disk: 64, ..target },
            VmResources { cpu: 9, ..target },
        ] {
            assert!(rt.resize_vm(&guest.id, invalid).is_err());
        }
        for (engine, state, image) in [
            (Engine::Qemu, VmState::Running, "offline"),
            (Engine::Qemu, VmState::Stopped, "live"),
            (Engine::Docker, VmState::Stopped, "offline"),
        ] {
            let mut invalid = reduced.clone();
            invalid.engine = engine;
            invalid.state = state;
            invalid.image = image.into();
            save_vm(&rt.db, &invalid).unwrap();
            assert!(rt.resize_vm(&guest.id, target).is_err());
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn resource_update_retries_after_reopen_and_keeps_disk_and_identity() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("agent.db");
        let mut rt = runtime(Connection::open(&database).unwrap());
        rt.runtime_dir = dir.path().to_str().unwrap().into();
        let mut guest = vm("resize", "offline", VmState::Stopped);
        guest.engine = Engine::Firecracker;
        guest.disk = 64 + ttcore::guest_config::CONFIG_DISK_MIB;
        guest.options.requested_disk = 64;
        guest.options.guest_config_digest = Some("immutable-config".into());
        save_vm(&rt.db, &guest).unwrap();
        let clone = std::path::PathBuf::from(rt.clone_path(&guest));
        std::fs::create_dir(&clone).unwrap();
        let root = clone.join("rootfs.ext4");
        // Fail before a filesystem exists: intent and larger reservation must survive.
        std::fs::File::create(&root)
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
        let target = VmResources {
            cpu: 4,
            mem: 2048,
            disk: 96,
        };
        assert!(rt.resize_vm(&guest.id, target).is_err());
        let pending = load_vm(&rt.db, &guest.id).unwrap().unwrap();
        assert_eq!(pending.pending_resources, Some(target));
        assert_eq!(pending.disk, guest.disk);
        assert_eq!(rt.resource.disk_used, 100);
        assert!(rt.start_vm(&guest.id).is_err());
        assert!(
            rt.resize_vm(&guest.id, VmResources { cpu: 2, ..target })
                .is_err()
        );
        drop(rt);

        let contents = dir.path().join("contents");
        std::fs::create_dir(&contents).unwrap();
        std::fs::write(contents.join("marker"), "retained").unwrap();
        let mkfs = std::process::Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(contents)
            .arg(&root)
            .output()
            .unwrap();
        assert!(mkfs.status.success());
        // Simulate a crash after growing the device but before resize2fs.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&root)
            .unwrap()
            .set_len(96 * 1024 * 1024)
            .unwrap();
        let mut rt = runtime(Connection::open(&database).unwrap());
        rt.runtime_dir = dir.path().to_str().unwrap().into();
        let done = rt.resize_vm(&guest.id, target).unwrap();
        assert!(done.pending_resources.is_none());
        assert_eq!((done.cpu, done.mem, done.disk), (4, 2048, 100));
        assert_eq!(done.options.requested_disk, 96);
        assert_eq!(done.id, guest.id);
        assert_eq!(
            done.options.guest_config_digest,
            guest.options.guest_config_digest
        );
        let marker = std::process::Command::new("debugfs")
            .args(["-R", "cat /marker"])
            .arg(&root)
            .output()
            .unwrap();
        assert_eq!(marker.stdout, b"retained");
        let header = std::process::Command::new("dumpe2fs")
            .arg("-h")
            .arg(&root)
            .output()
            .unwrap();
        let text = String::from_utf8(header.stdout).unwrap();
        let value = |key: &str| -> u64 {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        assert_eq!(
            value("Block count:") * value("Block size:"),
            96 * 1024 * 1024
        );
        assert!(rt.resize_vm(&guest.id, target).is_ok());
        assert!(
            rt.resize_vm(&guest.id, VmResources { disk: 64, ..target })
                .is_err()
        );
        assert!(
            rt.resize_vm(&guest.id, VmResources { cpu: 9, ..target })
                .is_err()
        );
        let mut running = done.clone();
        running.image = "live".into();
        save_vm(&rt.db, &running).unwrap();
        assert!(
            rt.resize_vm(&guest.id, VmResources { cpu: 2, ..target })
                .is_err()
        );
        running.image = "offline".into();
        running.state = VmState::Running;
        save_vm(&rt.db, &running).unwrap();
        assert!(rt.resize_vm(&guest.id, target).is_err());
    }
    fn vm(id: &str, image: &str, state: VmState) -> Vm {
        Vm {
            ssh: None,
            pending_resources: None,
            id: id.into(),
            env_id: "env".into(),
            host_id: "h1".into(),
            image: image.into(),
            engine: Engine::Docker,
            cpu: 2,
            mem: 1024,
            disk: 512,
            ip: String::new(),
            port_map: BTreeMap::new(),
            options: Default::default(),
            error: None,
            state,
            created_at: 0,
        }
    }
    #[test]
    fn corrupt_record_does_not_block_healthy_cleanup_or_release_unknown_capacity() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.db
            .execute("INSERT INTO vms VALUES ('broken', 'not-json')", [])
            .unwrap();
        save_vm(&rt.db, &vm("healthy", "live", VmState::Deleting)).unwrap();
        rt.reconcile().unwrap();
        assert!(load_vm(&rt.db, "healthy").unwrap().is_none());
        assert!(load_vm(&rt.db, "broken").is_err());
        assert_eq!(rt.resource.cpu_free(), 0);
        assert_eq!(rt.resource.disk_free(), 0);
        assert!(rt.stop_vm("absent").is_err());
    }

    #[test]
    fn confirmed_stop_clears_only_obsolete_network_recovery_errors() {
        for state in [VmState::Running, VmState::Paused, VmState::Stopped] {
            for error in [
                "network recovery pending",
                "network recovery failed: transient",
            ] {
                let mut rt = runtime(Connection::open_in_memory().unwrap());
                let mut guest = vm("rebooted", "offline", state);
                guest.engine = Engine::Qemu;
                guest.error = Some(error.into());
                save_vm(&rt.db, &guest).unwrap();
                rt.reconcile().unwrap();
                rt.stop_vm(&guest.id).unwrap();
                let saved = load_vm(&rt.db, &guest.id).unwrap().unwrap();
                assert_eq!(saved.state, VmState::Stopped);
                assert!(saved.error.is_none());
            }
        }
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        let mut guest = vm("pending", "offline", VmState::Stopped);
        guest.error = Some("disk growth failed".into());
        guest.pending_resources = Some(VmResources {
            cpu: 1,
            mem: 128,
            disk: 1024,
        });
        save_vm(&rt.db, &guest).unwrap();
        rt.reconcile().unwrap();
        assert_eq!(
            load_vm(&rt.db, &guest.id).unwrap().unwrap().error,
            guest.error
        );
        guest.state = VmState::Creating;
        guest.pending_resources = None;
        guest.error = Some("network recovery pending".into());
        save_vm(&rt.db, &guest).unwrap();
        rt.reconcile().unwrap();
        let saved = load_vm(&rt.db, &guest.id).unwrap().unwrap();
        assert_eq!(saved.state, VmState::Failed);
        assert!(saved.error.unwrap().contains("operation interrupted"));
        delete_vm(&rt.db, &guest.id).unwrap();
        rt.reconcile_vm(&guest.id).unwrap();
        assert!(load_vm(&rt.db, &guest.id).unwrap().is_none());
    }

    #[test]
    fn network_binding_prevents_losing_owned_ingress_during_reconfiguration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let ingress = crate::ssh_ingress::SshIngress {
            public_address: "192.0.2.1".parse().unwrap(),
            namespace: cfg!(target_os = "linux").then(|| dir.path().to_path_buf()),
            target: cfg!(target_os = "linux").then(|| "192.0.2.2".parse().unwrap()),
        };
        let mut rt = runtime(Connection::open(&path).unwrap());
        rt.configure_network(Some(ingress.clone()), 21000, 21999)
            .unwrap();
        save_vm(&rt.db, &vm("retained", "offline", VmState::Stopped)).unwrap();
        drop(rt);
        let mut rt = runtime(Connection::open(&path).unwrap());
        rt.configure_network(Some(ingress.clone()), 21000, 21999)
            .unwrap();
        assert!(rt.configure_network(None, 21000, 21999).is_err());
        assert!(
            rt.configure_network(Some(ingress.clone()), 22000, 22999)
                .is_err()
        );
        let changed = crate::ssh_ingress::SshIngress {
            public_address: "192.0.2.3".parse().unwrap(),
            ..ingress.clone()
        };
        assert!(rt.configure_network(Some(changed), 21000, 21999).is_err());
        assert_eq!(rt.ssh_ingress, Some(ingress.clone()));
        delete_vm(&rt.db, "retained").unwrap();
        rt.db
            .execute("INSERT INTO vms VALUES ('broken','not-json')", [])
            .unwrap();
        assert!(rt.configure_network(None, 21000, 21999).is_err());
        delete_vm(&rt.db, "broken").unwrap();
        rt.configure_network(None, 22000, 22999).unwrap();
        assert!(rt.configure_network(None, 0, 10).is_err());
        assert!(rt.configure_network(None, 22000, 21999).is_err());
        let first = allocate_ports(&[], &[22, 80], 21000..=21001, |_| true).unwrap();
        let second = allocate_ports(&[], &[22, 80], 22000..=22001, |_| true).unwrap();
        assert_eq!(
            first.values().copied().collect::<Vec<_>>(),
            vec![21000, 21001]
        );
        assert_eq!(
            second.values().copied().collect::<Vec<_>>(),
            vec![22000, 22001]
        );
        assert!(allocate_ports(&[], &[22, 80, 443], 21000..=21001, |_| true).is_err());
    }

    #[test]
    fn missing_native_bindings_are_rejected_instead_of_adopted() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        let guest = vm("retained", "live", VmState::Running);
        save_vm(&rt.db, &guest).unwrap();
        assert!(rt.configure_network(None, 20000, 65535).is_err());
        for requested in [
            None,
            Some(ContainerRuntime::Docker),
            Some(ContainerRuntime::Podman),
        ] {
            assert!(
                bind_container_runtime(&rt.db, requested, || panic!("must not infer ownership"))
                    .is_err()
            );
        }
        assert_eq!(
            load_vm(&rt.db, &guest.id).unwrap().unwrap().state,
            VmState::Running
        );
        assert_eq!(rt.db.query_row("SELECT COUNT(*) FROM _meta WHERE key IN ('network_config','container_runtime')", [], |r| r.get::<_, u32>(0)).unwrap(), 0);
    }

    #[test]
    fn corrupt_vm_row_does_not_block_existing_native_bindings_or_healthy_stop() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.configure_network(None, 20000, 65535).unwrap();
        bind_container_runtime(&rt.db, Some(ContainerRuntime::Podman), || {
            panic!("explicit runtime")
        })
        .unwrap();
        save_vm(&rt.db, &vm("healthy", "live", VmState::Running)).unwrap();
        rt.db
            .execute("INSERT INTO vms VALUES ('broken','not-json')", [])
            .unwrap();
        rt.configure_network(None, 20000, 65535).unwrap();
        assert_eq!(
            bind_container_runtime(&rt.db, None, || panic!("must retain runtime")).unwrap(),
            Some(ContainerRuntime::Podman)
        );
        rt.stop_vm("healthy").unwrap();
        assert_eq!(
            load_vm(&rt.db, "healthy").unwrap().unwrap().state,
            VmState::Stopped
        );
        assert!(load_vm(&rt.db, "broken").is_err());
    }

    #[test]
    fn late_readiness_clears_errors_without_a_restart() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        let mut record = vm("late", "live", VmState::Failed);
        record.error = Some("boot readiness timed out".into());
        save_vm(&rt.db, &record).unwrap();
        rt.reconcile().unwrap();
        let record = load_vm(&rt.db, "late").unwrap().unwrap();
        assert_eq!(record.state, VmState::Running);
        assert!(record.error.is_none());
    }

    #[test]
    fn failed_delete_keeps_record_and_reservations_then_retry_succeeds() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        let mut record = vm("vm", "fail-delete", VmState::Running);
        save_vm(&rt.db, &record).unwrap();
        rt.recount().unwrap();
        assert!(rt.destroy_vm("vm").is_err());
        let kept = load_vm(&rt.db, "vm").unwrap().unwrap();
        assert_eq!(kept.state, VmState::Deleting);
        assert!(kept.error.unwrap().contains("injected"));
        assert_eq!(rt.resource.disk_used, 512);
        record.image = "success".into();
        save_vm(&rt.db, &record).unwrap();
        rt.destroy_vm("vm").unwrap();
        rt.destroy_vm("vm").unwrap();
        assert!(load_vm(&rt.db, "vm").unwrap().is_none());
        assert_eq!(rt.resource.vm_count, 0);
    }
    #[test]
    fn repeated_create_is_idempotent_and_changed_request_is_rejected() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        let req = CreateVmReq {
            ssh: None,
            vm_id: "idempotent".into(),
            isolated_network: false,
            guest_config: Default::default(),
            env_id: "env".into(),
            image: "live".into(),
            engine: Engine::Docker,
            cpu: 1,
            mem: 128,
            disk: 0,
            ports: vec![80, 443],
            deny_outgoing: false,
            ssh_keys: vec![],
        };
        let before = CREATES.load(Ordering::SeqCst);
        let first = rt.create_vm(&req).unwrap();
        let mut reordered = req.clone();
        reordered.ports.reverse();
        let second = rt.create_vm(&reordered).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(CREATES.load(Ordering::SeqCst) - before, 1);
        assert_eq!(rt.resource.vm_count, 1);
        let mut changed = req;
        changed.mem += 1;
        assert!(rt.create_vm(&changed).is_err());
    }
    #[test]
    fn recovery_rechecks_processes_and_keeps_stopped_disk_allocations() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        save_vm(&rt.db, &vm("dead", "dead", VmState::Running)).unwrap();
        save_vm(&rt.db, &vm("partial", "dead", VmState::Creating)).unwrap();
        save_vm(&rt.db, &vm("booted", "live", VmState::Creating)).unwrap();
        rt.reconcile().unwrap();
        assert_eq!(
            load_vm(&rt.db, "dead").unwrap().unwrap().state,
            VmState::Stopped
        );
        assert_eq!(
            load_vm(&rt.db, "partial").unwrap().unwrap().state,
            VmState::Failed
        );
        assert_eq!(
            load_vm(&rt.db, "booted").unwrap().unwrap().state,
            VmState::Running
        );
        assert_eq!(rt.resource.disk_used, 1536);
        assert_eq!(rt.resource.vm_count, 3);
    }
    #[test]
    fn ports_reuse_holes_skip_conflicts_and_fail_explicitly_when_exhausted() {
        let mut first = vm("first", "live", VmState::Running);
        first.port_map.insert(22, 20000);
        let next = allocate_ports(&[first], &[22, 80, 22], 20000..=65535, |p| p != 20001).unwrap();
        assert_eq!(next[&22], 20002);
        assert_eq!(next[&80], 20003);
        assert_eq!(
            allocate_ports(&[], &[22], 20000..=65535, |_| true).unwrap()[&22],
            20000
        );
        assert!(allocate_ports(&[], &[22], 20000..=65535, |_| false).is_err());
        let many = (1..=1000).collect::<Vec<_>>();
        assert_eq!(
            allocate_ports(&[], &many, 20000..=65535, |_| true)
                .unwrap()
                .len(),
            1000
        );
    }
    #[test]
    fn snapshot_is_readable_while_mutation_lock_is_held() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.db");
        let rt = runtime(Connection::open(&path).unwrap());
        save_vm(&rt.db, &vm("vm", "live", VmState::Creating)).unwrap();
        let lock = std::sync::Mutex::new(rt);
        let _guard = lock.lock().unwrap();
        assert_eq!(read_vms(path.to_str().unwrap()).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reconciliation_yields_to_queued_mutations_between_vm_probes() {
        struct GatedEngine;
        impl VmEngine for GatedEngine {
            fn create(&self, _: &Vm, _: &str, _: &str, _: &[String]) -> Result<()> {
                unreachable!()
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
            fn name(&self) -> &'static str {
                "gated"
            }
            fn state(&self, vm: &Vm) -> Result<VmState> {
                let dir = std::path::Path::new(&vm.image);
                if !dir.join("entered").exists() {
                    std::fs::write(dir.join("entered"), b"").unwrap();
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while !dir.join("release").exists() && std::time::Instant::now() < deadline {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                } else {
                    std::fs::write(dir.join("second"), vm.state.to_string()).unwrap();
                }
                Ok(VmState::Stopped)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let mut rt = runtime(Connection::open(&path).unwrap());
        rt.engine_factory = |_, _| Ok(Box::new(GatedEngine));
        for id in ["first", "second"] {
            save_vm(
                &rt.db,
                &vm(id, dir.path().to_str().unwrap(), VmState::Running),
            )
            .unwrap();
        }
        let state = std::sync::Arc::new(crate::handler::AgentShared {
            info: AgentInfo {
                vms: None,
                warnings: vec![],
                image_sizes: Default::default(),
                capabilities: vec![],
                host_id: "h1".into(),
                resource: rt.resource.clone(),
                engines: vec![],
                storage: Storage::File,
                images: vec![],
            },
            runtime: std::sync::Arc::new(tokio::sync::Mutex::new(rt)),
            db_path: path.to_string_lossy().into(),
            image_dir: String::new(),
            images: Default::default(),
        });
        let scan = tokio::spawn(crate::handler::reconcile_once(state.clone()));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !dir.path().join("entered").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let waiting = state.runtime.lock();
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut waiting)
                .await
                .is_err()
        );
        std::fs::write(dir.path().join("release"), b"").unwrap();
        let mut rt = waiting.await;
        assert!(!dir.path().join("second").exists());
        rt.stop_vm("first").unwrap();
        rt.stop_vm("second").unwrap();
        drop(rt);
        scan.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("second")).unwrap(),
            "stopped"
        );
    }
    #[test]
    fn jail_rejects_zvol_before_creating_a_record_or_resources() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.storage = Storage::Zvol;
        rt.engines = vec![Engine::Jail];
        let req = CreateVmReq {
            ssh: None,
            vm_id: "jail-zvol".into(),
            env_id: "env".into(),
            image: "freebsd-base".into(),
            engine: Engine::Jail,
            cpu: 1,
            mem: 128,
            disk: 0,
            ports: vec![],
            deny_outgoing: false,
            isolated_network: false,
            guest_config: Default::default(),
            ssh_keys: vec![],
        };
        assert!(
            rt.create_vm(&req)
                .unwrap_err()
                .to_string()
                .contains("file storage")
        );
        assert!(rt.list_vms().unwrap().is_empty());
        assert_eq!(rt.resource.vm_count, 0);
    }

    #[test]
    fn bhyve_propagates_strict_file_disk_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.store = Box::new(ttcore::storage::file::FileStore);
        rt.runtime_dir = dir.path().to_str().unwrap().into();
        let mut record = vm("bhyve-disk", "image", VmState::Stopped);
        record.engine = Engine::Bhyve;
        let clone = dir.path().join("clone-bhyve-disk");
        assert!(rt.image_path(&record).is_err());
        std::fs::create_dir(&clone).unwrap();
        let disk = clone.join("disk.raw");
        std::fs::write(&disk, b"raw disk").unwrap();
        assert_eq!(rt.image_path(&record).unwrap(), disk.to_str().unwrap());
        std::fs::write(clone.join("other.raw"), b"another disk").unwrap();
        assert!(rt.image_path(&record).is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(clone.join("other.raw")).unwrap();
            std::fs::remove_file(&disk).unwrap();
            let outside = dir.path().join("outside.raw");
            std::fs::write(&outside, b"unowned").unwrap();
            std::os::unix::fs::symlink(outside, &disk).unwrap();
            assert!(rt.image_path(&record).is_err());
        }
    }

    #[test]
    fn firecracker_and_jail_receive_root_directories_not_qcow2_paths() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.runtime_dir = "/tmp/tt-runtime".into();
        let mut record = vm("guest", "image", VmState::Stopped);
        record.engine = Engine::Firecracker;
        assert_eq!(
            rt.image_path(&record).unwrap(),
            "/tmp/tt-runtime/clone-guest"
        );
        record.engine = Engine::Jail;
        assert_eq!(
            rt.image_path(&record).unwrap(),
            "/tmp/tt-runtime/clone-guest"
        );
    }

    #[test]
    fn slow_image_inspection_does_not_block_snapshots_and_publishes_when_ready() {
        use ttcore::command::CommandExt;
        const CHILD: &str = "TT_IMAGE_CATALOG_TEST_DIR";
        if let Some(path) = std::env::var_os(CHILD) {
            let dir = std::path::PathBuf::from(path);
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                use axum::response::IntoResponse;
                let db_path = dir.join("agent.db");
                let rt = runtime(Connection::open(&db_path).unwrap());
                let image_dir = dir.join("images");
                std::fs::create_dir(&image_dir).unwrap();
                std::fs::write(image_dir.join("test.qcow2"), b"fixture").unwrap();
                let info = AgentInfo {
                    vms: None,
                    warnings: vec![],
                    image_sizes: Default::default(),
                    capabilities: vec![],
                    host_id: rt.host_id.clone(),
                    resource: rt.resource.clone(),
                    engines: vec![Engine::Docker],
                    storage: Storage::File,
                    images: vec![],
                };
                let state = std::sync::Arc::new(crate::handler::AgentShared {
                    runtime: std::sync::Arc::new(tokio::sync::Mutex::new(rt)),
                    db_path: db_path.to_string_lossy().into(),
                    info,
                    image_dir: image_dir.to_string_lossy().into(),
                    images: Default::default(),
                });
                let refresh = tokio::spawn(crate::handler::refresh_images(state.clone()));
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while !dir.join("entered").exists() {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .unwrap();
                for _ in 0..3 {
                    let response = tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        crate::handler::get_info(axum::extract::State(state.clone())),
                    )
                    .await
                    .unwrap()
                    .into_response();
                    assert_eq!(response.status(), axum::http::StatusCode::OK);
                    let bytes = axum::body::to_bytes(response.into_body(), 16384)
                        .await
                        .unwrap();
                    let reply: ttcore::api::ApiResp<AgentInfo> =
                        serde_json::from_slice(&bytes).unwrap();
                    let info = reply.data.unwrap();
                    assert!(info.images.is_empty());
                    assert!(info.vms.is_some());
                    assert!(!info.warnings.is_empty());
                }
                std::fs::write(dir.join("release"), b"").unwrap();
                refresh.await.unwrap();
                let catalog = state.images.read().unwrap();
                assert_eq!(catalog.images, ["test.qcow2"]);
                assert_eq!(catalog.sizes["test.qcow2"], 64);
                assert!(catalog.warning.is_none());
            });
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("qemu-img");
        std::fs::write(
            &script,
            r#"#!/bin/sh
: > "$TT_IMAGE_CATALOG_TEST_DIR/entered"
n=0
while [ ! -e "$TT_IMAGE_CATALOG_TEST_DIR/release" ] && [ "$n" -lt 300 ]; do
  /bin/sleep 0.01
  n=$((n + 1))
done
printf '%s\n' '{"format":"qcow2","virtual-size":67108864}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "runtime::lifecycle_tests::slow_image_inspection_does_not_block_snapshots_and_publishes_when_ready", "--nocapture"])
            .env(CHILD, dir.path())
            .env("PATH", dir.path())
            .output_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
