//! Persistent state management backed by SQLite.
//!
//! All fleet state (hosts, environments, VMs) is stored in a single
//! SQLite database. Data survives controller restarts.

use ruc::*;
use rusqlite::Connection;
use ttcore::api::FleetStatus;
use ttcore::model::*;

/// Current schema version. Bump this when schema changes.
const SCHEMA_VERSION: u32 = 2;

/// Fleet database — the single source of truth for the controller.
pub struct Db {
    conn: Connection,
}

impl Db {
    /// Open or create the database at the given path.
    ///
    /// Performs automatic schema migration if the existing database
    /// has an older version.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).c(d!("open DB"))?;

        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )
        .c(d!("set pragmas"))?;

        Self::migrate(&conn)?;

        Ok(Self { conn })
    }

    /// Run schema migrations from the current version to SCHEMA_VERSION.
    fn migrate(conn: &Connection) -> Result<()> {
        // Create the meta table if it doesn't exist
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .c(d!("create meta table"))?;

        let current = Self::get_schema_version(conn)?;

        if current > SCHEMA_VERSION {
            return Err(eg!(
                "database schema v{} is newer than this binary (v{}); upgrade TTstack first",
                current,
                SCHEMA_VERSION
            ));
        }

        if current < 1 {
            // v0 → v1: initial schema
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS hosts (
                     id   TEXT PRIMARY KEY,
                     data TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS envs (
                     id   TEXT PRIMARY KEY,
                     data TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS vms (
                     id      TEXT PRIMARY KEY,
                     env_id  TEXT NOT NULL,
                     host_id TEXT NOT NULL,
                     data    TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_vms_env  ON vms(env_id);
                 CREATE INDEX IF NOT EXISTS idx_vms_host ON vms(host_id);",
            )
            .c(d!("migration v1"))?;
        }

        // v2 adds lifecycle fields in serialized records, decoded with serde defaults.
        // The version guard prevents older binaries from opening this state.

        Self::set_schema_version(conn, SCHEMA_VERSION)?;

        if current < SCHEMA_VERSION {
            eprintln!("database migrated: v{current} → v{SCHEMA_VERSION}");
        }

        Ok(())
    }

    fn get_schema_version(conn: &Connection) -> Result<u32> {
        let mut stmt = conn
            .prepare("SELECT value FROM _meta WHERE key = 'schema_version'")
            .c(d!())?;
        let mut rows = stmt.query([]).c(d!())?;
        match rows.next().c(d!())? {
            Some(row) => {
                let val: String = row.get(0).c(d!())?;
                val.parse::<u32>()
                    .map_err(|_| eg!(format!("invalid schema_version: {val}")))
            }
            None => Ok(0), // fresh database
        }
    }

    fn set_schema_version(conn: &Connection, ver: u32) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO _meta (key, value) VALUES ('schema_version', ?1)",
            rusqlite::params![ver.to_string()],
        )
        .c(d!("set schema version"))?;
        Ok(())
    }

    // ── Hosts ───────────────────────────────────────────────────────

    pub fn put_host(&self, host: &Host) -> Result<()> {
        let data = serde_json::to_string(host).c(d!("serialize host"))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO hosts (id, data) VALUES (?1, ?2)",
                rusqlite::params![host.id, data],
            )
            .c(d!("put host"))?;
        Ok(())
    }

    pub fn get_host(&self, id: &str) -> Result<Option<Host>> {
        query_one(&self.conn, "SELECT data FROM hosts WHERE id = ?1", [id])
    }

    pub fn remove_host(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM hosts WHERE id = ?1", [id])
            .c(d!("remove host"))?;
        Ok(())
    }

    pub fn list_hosts(&self) -> Result<Vec<Host>> {
        query_all(&self.conn, "SELECT data FROM hosts", [])
    }

    pub fn host_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM hosts", [], |row| row.get(0))
            .c(d!("count hosts"))?;
        Ok(count as usize)
    }

    // ── Environments ────────────────────────────────────────────────

    pub fn put_env(&self, env: &Env) -> Result<()> {
        let data = serde_json::to_string(env).c(d!("serialize env"))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO envs (id, data) VALUES (?1, ?2)",
                rusqlite::params![env.id, data],
            )
            .c(d!("put env"))?;
        Ok(())
    }

    /// Save the complete creation plan atomically before contacting any agent.
    pub fn put_environment(&self, env: &Env, vms: &[Vm]) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .c(d!("begin environment plan"))?;
        self.put_env(env)?;
        for vm in vms {
            self.put_vm(vm)?;
        }
        tx.commit().c(d!("commit environment plan"))?;
        Ok(())
    }

    pub fn get_env(&self, id: &str) -> Result<Option<Env>> {
        query_one(&self.conn, "SELECT data FROM envs WHERE id = ?1", [id])
    }

    pub fn remove_env(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM envs WHERE id = ?1", [id])
            .c(d!("remove env"))?;
        Ok(())
    }

    pub fn list_envs(&self) -> Result<Vec<Env>> {
        query_all(&self.conn, "SELECT data FROM envs", [])
    }

    pub fn env_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM envs", [], |row| row.get(0))
            .c(d!("count envs"))?;
        Ok(count as usize)
    }

    // ── VMs ─────────────────────────────────────────────────────────

    pub fn put_vm(&self, vm: &Vm) -> Result<()> {
        let data = serde_json::to_string(vm).c(d!("serialize VM"))?;
        self.conn
            .execute(
                "INSERT OR REPLACE INTO vms (id, env_id, host_id, data)
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![vm.id, vm.env_id, vm.host_id, data],
            )
            .c(d!("put VM"))?;
        Ok(())
    }

    pub fn get_vm(&self, id: &str) -> Result<Option<Vm>> {
        query_one(&self.conn, "SELECT data FROM vms WHERE id = ?1", [id])
    }

    pub fn remove_vm(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM vms WHERE id = ?1", [id])
            .c(d!("remove VM"))?;
        Ok(())
    }

    pub fn vm_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vms", [], |row| row.get(0))
            .c(d!("count VMs"))?;
        Ok(count as usize)
    }

    pub fn vms_by_env(&self, env_id: &str) -> Result<Vec<Vm>> {
        query_all(
            &self.conn,
            "SELECT data FROM vms WHERE env_id = ?1",
            [env_id],
        )
    }

    pub fn vms_by_host(&self, host_id: &str) -> Result<Vec<Vm>> {
        query_all(
            &self.conn,
            "SELECT data FROM vms WHERE host_id = ?1",
            [host_id],
        )
    }

    // ── Aggregate Status ────────────────────────────────────────────

    pub fn fleet_status(&self) -> Result<FleetStatus> {
        let hosts = self.list_hosts()?;
        let hosts_online = hosts
            .iter()
            .filter(|h| h.state == HostState::Online)
            .count() as u32;

        let (mut cpu_t, mut cpu_u) = (0u32, 0u32);
        let (mut mem_t, mut mem_u) = (0u32, 0u32);
        let (mut disk_t, mut disk_u) = (0u32, 0u32);
        for h in &hosts {
            cpu_t += h.resource.cpu_total;
            cpu_u += h.resource.cpu_used;
            mem_t += h.resource.mem_total;
            mem_u += h.resource.mem_used;
            disk_t += h.resource.disk_total;
            disk_u += h.resource.disk_used;
        }

        Ok(FleetStatus {
            hosts: hosts.len() as u32,
            hosts_online,
            total_vms: self.vm_count()? as u32,
            total_envs: self.env_count()? as u32,
            cpu_total: cpu_t,
            cpu_used: cpu_u,
            mem_total: mem_t,
            mem_used: mem_u,
            disk_total: disk_t,
            disk_used: disk_u,
        })
    }
}

// ── Generic Query Helpers ───────────────────────────────────────────

fn query_one<T: serde::de::DeserializeOwned, P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Option<T>> {
    let mut stmt = conn.prepare(sql).c(d!("prepare"))?;
    let mut rows = stmt.query(params).c(d!("query"))?;
    match rows.next().c(d!("next"))? {
        Some(row) => {
            let data: String = row.get(0).c(d!("get col"))?;
            let obj: T = serde_json::from_str(&data).c(d!("deserialize"))?;
            Ok(Some(obj))
        }
        None => Ok(None),
    }
}

fn query_all<T: serde::de::DeserializeOwned, P: rusqlite::Params>(
    conn: &Connection,
    sql: &str,
    params: P,
) -> Result<Vec<T>> {
    let mut stmt = conn.prepare(sql).c(d!("prepare"))?;
    let rows = stmt
        .query_map(params, |row| row.get::<_, String>(0))
        .c(d!("query"))?;
    let mut result = Vec::new();
    for row in rows {
        let data = row.c(d!("read row"))?;
        let obj: T = serde_json::from_str(&data).c(d!("deserialize"))?;
        result.push(obj);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn test_db() -> Db {
        Db::open(":memory:").unwrap()
    }

    fn make_host(id: &str) -> Host {
        Host {
            capabilities: vec![],
            id: id.into(),
            addr: format!("{id}:9100"),
            resource: Resource {
                cpu_total: 8,
                mem_total: 16384,
                disk_total: 500_000,
                ..Default::default()
            },
            state: HostState::Online,
            engines: vec![Engine::Qemu],
            storage: Storage::File,
            images: vec![],
            registered_at: 1000,
        }
    }

    fn make_env(id: &str) -> Env {
        Env {
            id: id.into(),
            owner: "tester".into(),
            vm_ids: vec!["vm1".into()],
            created_at: 1000,
            expires_at: 2000,
            error: None,
            state: EnvState::Active,
        }
    }

    fn make_vm(id: &str, env_id: &str, host_id: &str) -> Vm {
        Vm {
            id: id.into(),
            env_id: env_id.into(),
            host_id: host_id.into(),
            image: "ubuntu".into(),
            engine: Engine::Qemu,
            cpu: 2,
            mem: 1024,
            disk: 40960,
            ip: "10.10.0.1".into(),
            port_map: BTreeMap::new(),
            options: VmOptions::default(),
            error: None,
            state: VmState::Running,
            created_at: 1000,
        }
    }

    // ── Host CRUD ───────────────────────────────────────────────────

    #[test]
    fn engine_sets_roundtrip_and_unknown_records_fail_without_mutation() {
        let db = test_db();
        let mut host = make_host("host");
        host.engines = vec![
            Engine::Qemu,
            Engine::Firecracker,
            Engine::Docker,
            Engine::Bhyve,
            Engine::Jail,
        ];
        db.put_host(&host).unwrap();
        assert_eq!(db.get_host("host").unwrap().unwrap().engines, host.engines);
        for engine in &host.engines {
            let mut vm = make_vm(&engine.to_string(), "env", "host");
            vm.engine = *engine;
            vm.state = VmState::Stopped;
            db.put_vm(&vm).unwrap();
            assert_eq!(db.get_vm(&vm.id).unwrap().unwrap().engine, *engine);
        }
        let mut old = serde_json::to_value(&host).unwrap();
        old["engines"] = serde_json::json!(["unsupported-engine"]);
        let original = old.to_string();
        db.conn
            .execute("UPDATE hosts SET data=?1 WHERE id='host'", [&original])
            .unwrap();
        assert!(db.get_host("host").is_err());
        assert!(db.list_hosts().is_err());
        let stored: String = db
            .conn
            .query_row("SELECT data FROM hosts WHERE id='host'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, original);
        assert_eq!(db.vm_count().unwrap(), host.engines.len());
    }

    #[test]
    fn host_crud() {
        let db = test_db();
        assert_eq!(db.host_count().unwrap(), 0);

        let h = make_host("h1");
        db.put_host(&h).unwrap();

        assert_eq!(db.host_count().unwrap(), 1);
        let got = db.get_host("h1").unwrap().unwrap();
        assert_eq!(got.id, "h1");
        assert_eq!(got.resource.cpu_total, 8);

        db.remove_host("h1").unwrap();
        assert!(db.get_host("h1").unwrap().is_none());
        assert_eq!(db.host_count().unwrap(), 0);
    }

    #[test]
    fn host_list() {
        let db = test_db();
        db.put_host(&make_host("h1")).unwrap();
        db.put_host(&make_host("h2")).unwrap();
        let hosts = db.list_hosts().unwrap();
        assert_eq!(hosts.len(), 2);
    }

    #[test]
    fn host_upsert() {
        let db = test_db();
        let mut h = make_host("h1");
        db.put_host(&h).unwrap();
        h.state = HostState::Offline;
        db.put_host(&h).unwrap();
        assert_eq!(db.host_count().unwrap(), 1);
        let got = db.get_host("h1").unwrap().unwrap();
        assert_eq!(got.state, HostState::Offline);
    }

    // ── Env CRUD ────────────────────────────────────────────────────

    #[test]
    fn env_crud() {
        let db = test_db();
        let e = make_env("env1");
        db.put_env(&e).unwrap();

        assert_eq!(db.env_count().unwrap(), 1);
        let got = db.get_env("env1").unwrap().unwrap();
        assert_eq!(got.owner, "tester");

        db.remove_env("env1").unwrap();
        assert!(db.get_env("env1").unwrap().is_none());
    }

    #[test]
    fn env_list() {
        let db = test_db();
        db.put_env(&make_env("e1")).unwrap();
        db.put_env(&make_env("e2")).unwrap();
        db.put_env(&make_env("e3")).unwrap();
        assert_eq!(db.list_envs().unwrap().len(), 3);
    }

    // ── VM CRUD ─────────────────────────────────────────────────────

    #[test]
    fn vm_crud() {
        let db = test_db();
        let vm = make_vm("vm1", "env1", "h1");
        db.put_vm(&vm).unwrap();

        assert_eq!(db.vm_count().unwrap(), 1);
        let got = db.get_vm("vm1").unwrap().unwrap();
        assert_eq!(got.image, "ubuntu");

        db.remove_vm("vm1").unwrap();
        assert!(db.get_vm("vm1").unwrap().is_none());
    }

    #[test]
    fn vms_by_env() {
        let db = test_db();
        db.put_vm(&make_vm("v1", "env1", "h1")).unwrap();
        db.put_vm(&make_vm("v2", "env1", "h1")).unwrap();
        db.put_vm(&make_vm("v3", "env2", "h1")).unwrap();

        let vms = db.vms_by_env("env1").unwrap();
        assert_eq!(vms.len(), 2);

        let vms = db.vms_by_env("env2").unwrap();
        assert_eq!(vms.len(), 1);

        let vms = db.vms_by_env("nonexistent").unwrap();
        assert!(vms.is_empty());
    }

    #[test]
    fn vms_by_host() {
        let db = test_db();
        db.put_vm(&make_vm("v1", "e1", "h1")).unwrap();
        db.put_vm(&make_vm("v2", "e1", "h2")).unwrap();

        assert_eq!(db.vms_by_host("h1").unwrap().len(), 1);
        assert_eq!(db.vms_by_host("h2").unwrap().len(), 1);
        assert!(db.vms_by_host("h3").unwrap().is_empty());
    }

    // ── Fleet Status ────────────────────────────────────────────────

    #[test]
    fn fleet_status_aggregates() {
        let db = test_db();
        let mut h1 = make_host("h1");
        h1.resource.cpu_used = 2;
        h1.resource.mem_used = 4096;
        db.put_host(&h1).unwrap();

        let mut h2 = make_host("h2");
        h2.state = HostState::Offline;
        db.put_host(&h2).unwrap();

        db.put_env(&make_env("e1")).unwrap();
        db.put_vm(&make_vm("v1", "e1", "h1")).unwrap();

        let status = db.fleet_status().unwrap();
        assert_eq!(status.hosts, 2);
        assert_eq!(status.hosts_online, 1);
        assert_eq!(status.total_envs, 1);
        assert_eq!(status.total_vms, 1);
        assert_eq!(status.cpu_total, 16); // 8+8
        assert_eq!(status.cpu_used, 2);
    }
}
