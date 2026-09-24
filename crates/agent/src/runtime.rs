//! Durable VM lifecycle management for one host.
use ruc::*;
use rusqlite::Connection;
use std::collections::{BTreeMap, HashSet};
use ttcore::api::{AgentInfo, CreateVmReq};
use ttcore::engine;
use ttcore::model::*;
use ttcore::net;
use ttcore::storage::{self, ImageStore};

pub struct Runtime {
    pub host_id: String,
    db: Connection,
    engines: Vec<Engine>,
    store: Box<dyn ImageStore>,
    storage: Storage,
    image_dir: String,
    runtime_dir: String,
    network_ready: bool,
    pub resource: Resource,
    engine_factory: fn(Engine) -> Result<Box<dyn engine::VmEngine>>,
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
        init_db(&db)?;
        let mut rt = Self {
            host_id,
            db,
            engines: detect_engines(),
            store: storage::create_store(storage),
            storage,
            image_dir,
            runtime_dir,
            resource,
            engine_factory: engine::create_engine,
            network_ready: false,
        };
        #[cfg(target_os = "linux")]
        if rt.list_vms()?.iter().any(|vm| vm.engine != Engine::Docker) {
            rt.ensure_network()?;
        }
        rt.reconcile()?;
        for vm in rt.list_vms()? {
            if matches!(vm.state, VmState::Running | VmState::Paused)
                && let Err(e) = rt.restore_network(&vm)
            {
                let mut vm = vm;
                vm.error = Some(format!("network recovery failed: {e}"));
                save_vm(&rt.db, &vm)?;
            }
        }
        Ok(rt)
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
        if self.storage == Storage::Zvol && req.engine == Engine::Firecracker {
            return Err(eg!("{} requires file storage", req.engine));
        }
        if req.engine == Engine::Qemu && req.disk == 0 {
            return Err(eg!("QEMU disk size must be > 0"));
        }
        if req.cpu == 0 || req.mem == 0 {
            return Err(eg!("cpu and memory must be > 0"));
        }
        let options = VmOptions {
            ports: req.ports.clone(),
            ssh_keys: req.ssh_keys.clone(),
            deny_outgoing: req.deny_outgoing,
            requested_disk: req.disk,
            isolated_network: req.isolated_network,
            guest_config_digest: ttcore::guest_config::digest(&req.guest_config),
        };
        if let Some(vm) = load_vm(&self.db, &req.vm_id)? {
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
        let disk = match req.engine {
            Engine::Docker => 0,
            Engine::Firecracker => {
                let bytes = std::fs::metadata(format!("{base_image}/rootfs.ext4"))
                    .c(d!("rootfs.ext4"))?
                    .len();
                let base_mib =
                    u32::try_from(bytes.div_ceil(1024 * 1024)).c(d!("rootfs too large"))?;
                if req.disk != 0 && req.disk < base_mib {
                    return Err(eg!(
                        "requested disk is smaller than the base image ({base_mib} MiB)"
                    ));
                }
                req.disk
                    .max(base_mib)
                    .checked_add(if req.guest_config.is_empty() {
                        0
                    } else {
                        ttcore::guest_config::CONFIG_DISK_MIB
                    })
                    .ok_or_else(|| eg!("rootfs and configuration disk are too large"))?
            }
            _ => req.disk,
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
        if req.engine == Engine::Qemu && !ports.contains(&22) {
            ports.push(22);
        }
        let port_map = allocate_ports(&vms, &ports, |p| {
            std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, p)).is_ok()
        })?;
        let mut vm = Vm {
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
            if req.engine != Engine::Docker {
                self.ensure_network()?;
                let clone_path = self.clone_path(&vm);
                self.store.clone_image(&base_image, &clone_path)?;
                #[cfg(target_os = "linux")]
                if vm.engine == Engine::Firecracker {
                    if req.disk > 0 {
                        ttcore::storage::file::resize_ext4(
                            &std::path::Path::new(&clone_path).join("rootfs.ext4"),
                            req.disk,
                        )?;
                    }
                    ttcore::guest_config::write_disk(
                        std::path::Path::new(&clone_path),
                        &req.guest_config,
                    )?;
                }
                if vm.engine == Engine::Qemu {
                    self.store.resize_disk(&clone_path, vm.disk)?;
                }
                self.restore_network(&vm)?;
            }
            let path = self.image_path(&vm);
            (self.engine_factory)(vm.engine)?.create(
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
        #[cfg(target_os = "linux")]
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
    fn image_path(&self, vm: &Vm) -> String {
        let path = self.clone_path(vm);
        if vm.engine == Engine::Firecracker {
            path
        } else {
            self.store.resolve_disk(&path)
        }
    }
    fn restore_network(&self, vm: &Vm) -> Result<()> {
        #[cfg(target_os = "linux")]
        if vm.engine != Engine::Docker {
            #[cfg(target_os = "linux")]
            if vm.options.isolated_network {
                net::isolate(&vm.id, &vm.ip)?;
            }
            net::create_tap(&vm.id, &vm.ip)?;
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
        match (self.engine_factory)(vm.engine)?.stop(&vm) {
            Ok(()) => {
                vm.state = VmState::Stopped;
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
            let eng = (self.engine_factory)(vm.engine)?;
            if previous == VmState::Stopped
                && matches!(vm.engine, Engine::Qemu | Engine::Firecracker)
            {
                eng.create(
                    &vm,
                    &self.image_path(&vm),
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

    pub fn destroy_vm(&mut self, id: &str) -> Result<()> {
        let Some(mut vm) = load_vm(&self.db, id)? else {
            return Ok(());
        };
        vm.state = VmState::Deleting;
        save_vm(&self.db, &vm)?;
        let result = (|| -> Result<()> {
            (self.engine_factory)(vm.engine)?.destroy(&vm)?;
            #[cfg(target_os = "linux")]
            if vm.engine != Engine::Docker {
                net::remove_port_forwards(&vm.ip)?;
                net::allow_outgoing(&vm.ip)?;
                net::destroy_tap(id)?;
                #[cfg(target_os = "linux")]
                if vm.options.isolated_network {
                    net::remove_isolation(id)?;
                }
            }
            if vm.engine != Engine::Docker {
                self.store.remove_image(&self.clone_path(&vm))?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            vm.error = Some(e.to_string());
            save_vm(&self.db, &vm)?;
            return Err(e);
        }
        delete_vm(&self.db, id)?;
        self.recount()
    }

    pub fn reconcile(&mut self) -> Result<()> {
        for mut vm in self.list_vms()? {
            if vm.state == VmState::Deleting {
                if let Err(e) = self.destroy_vm(&vm.id) {
                    eprintln!("[agent] cleanup {}: {e}", vm.id);
                }
                continue;
            }
            match (self.engine_factory)(vm.engine)?.state(&vm) {
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
                }
                Err(e) => {
                    vm.error = Some(format!("cannot query engine: {e}"));
                }
            }
            save_vm(&self.db, &vm)?;
        }
        self.recount()
    }
    fn recount(&mut self) -> Result<()> {
        self.resource = resources_for(&self.resource, &self.list_vms()?);
        Ok(())
    }
    pub fn list_vms(&self) -> Result<Vec<Vm>> {
        load_all_vms(&self.db)
    }
    pub fn agent_info(&self) -> Result<AgentInfo> {
        Ok(AgentInfo {
            capabilities: if cfg!(target_os = "linux") {
                vec![
                    "guest_config".into(),
                    "isolated_network".into(),
                    "firecracker_jailer".into(),
                    "firecracker_disk_resize".into(),
                ]
            } else {
                vec![]
            },
            host_id: self.host_id.clone(),
            resource: self.resource.clone(),
            engines: self.engines.clone(),
            storage: self.storage,
            images: self.store.list_images(&self.image_dir)?,
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

fn allocate_ports(
    vms: &[Vm],
    ports: &[u16],
    available: impl Fn(u16) -> bool,
) -> Result<BTreeMap<u16, u16>> {
    let used: HashSet<u16> = vms
        .iter()
        .flat_map(|v| v.port_map.values().copied())
        .collect();
    let mut free = (20000..=65535u16).filter(|p| !used.contains(p) && available(*p));
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

/// Current agent schema version.
const SCHEMA_VERSION: u32 = 2;

fn init_db(db: &Connection) -> Result<()> {
    db.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS _meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
    )
    .c(d!("init meta table"))?;

    let current = get_schema_version(db)?;

    if current > SCHEMA_VERSION {
        return Err(eg!(
            "agent DB schema v{} is newer than this binary (v{}); upgrade TTstack first",
            current,
            SCHEMA_VERSION
        ));
    }

    if current < 1 {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS vms (
                id       TEXT PRIMARY KEY,
                data     TEXT NOT NULL
            );",
        )
        .c(d!("migration v1"))?;
    }

    // v2 adds lifecycle fields in serialized records, decoded with serde defaults.
    // The version guard prevents older binaries from opening this state.

    set_schema_version(db, SCHEMA_VERSION)?;
    if current < SCHEMA_VERSION {
        eprintln!("agent DB migrated: v{current} → v{SCHEMA_VERSION}");
    }

    Ok(())
}

fn get_schema_version(db: &Connection) -> Result<u32> {
    let mut stmt = db
        .prepare("SELECT value FROM _meta WHERE key = 'schema_version'")
        .c(d!())?;
    let mut rows = stmt.query([]).c(d!())?;
    match rows.next().c(d!())? {
        Some(row) => {
            let val: String = row.get(0).c(d!())?;
            val.parse::<u32>()
                .map_err(|_| eg!(format!("invalid schema_version: {val}")))
        }
        None => Ok(0),
    }
}

fn set_schema_version(db: &Connection, ver: u32) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO _meta (key, value) VALUES ('schema_version', ?1)",
        rusqlite::params![ver.to_string()],
    )
    .c(d!("set schema version"))?;
    Ok(())
}

/// Load or generate a stable host_id persisted in the agent database.
///
/// If `--host-id` is provided on the CLI, that value takes precedence and
/// is saved for future restarts. Otherwise we check the database; only
/// when neither is available do we generate a new random ID.
pub fn resolve_host_id(db_path: &str, cli_id: Option<String>) -> Result<String> {
    let conn = Connection::open(db_path).c(d!("open DB for host_id"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _meta (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );",
    )
    .c(d!("ensure meta table"))?;

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

fn detect_engines() -> Vec<Engine> {
    let mut engines = Vec::new();

    #[cfg(target_os = "linux")]
    {
        if which("qemu-system-x86_64") {
            engines.push(Engine::Qemu);
        }
        if which("firecracker") && which("jailer") {
            engines.push(Engine::Firecracker);
        }
    }

    // Detect the local container runtime.
    if which("docker") || which("podman") {
        engines.push(Engine::Docker);
    }

    engines
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
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
        let engines = [Engine::Qemu, Engine::Firecracker, Engine::Docker];
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
            engine_factory: |_| Ok(Box::new(FakeEngine)),
            network_ready: false,
        }
    }
    fn vm(id: &str, image: &str, state: VmState) -> Vm {
        Vm {
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
            vm_id: "idempotent".into(),
            isolated_network: false,
            guest_config: Default::default(),
            env_id: "env".into(),
            image: "live".into(),
            engine: Engine::Docker,
            cpu: 1,
            mem: 128,
            disk: 0,
            ports: vec![],
            deny_outgoing: false,
            ssh_keys: vec![],
        };
        let before = CREATES.load(Ordering::SeqCst);
        let first = rt.create_vm(&req).unwrap();
        let second = rt.create_vm(&req).unwrap();
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
        let next = allocate_ports(&[first], &[22, 80, 22], |p| p != 20001).unwrap();
        assert_eq!(next[&22], 20002);
        assert_eq!(next[&80], 20003);
        assert_eq!(allocate_ports(&[], &[22], |_| true).unwrap()[&22], 20000);
        assert!(allocate_ports(&[], &[22], |_| false).is_err());
        let many = (1..=1000).collect::<Vec<_>>();
        assert_eq!(allocate_ports(&[], &many, |_| true).unwrap().len(), 1000);
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
    #[test]
    fn firecracker_receives_root_directory_not_qcow2_path() {
        let mut rt = runtime(Connection::open_in_memory().unwrap());
        rt.runtime_dir = "/tmp/tt-runtime".into();
        let mut record = vm("guest", "image", VmState::Stopped);
        record.engine = Engine::Firecracker;
        assert_eq!(rt.image_path(&record), "/tmp/tt-runtime/clone-guest");
    }
}
