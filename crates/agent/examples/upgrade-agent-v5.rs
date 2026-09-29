//! Explicit offline conversion; the agent itself still accepts only native v6.
use clap::Parser;
use ruc::*;
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};
use ttcore::model::Vm;

#[derive(Parser)]
#[command(
    about = "Copy stopped-agent v5 metadata to a new v6 database without replacing the source"
)]
struct Args {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn convert(source: &Path, output: &Path) -> Result<usize> {
    let source = source.canonicalize().c(d!("resolve source database"))?;
    if output.exists() || !source.is_file() {
        return Err(eg!("source must be a file and output must not exist"));
    }
    let _lock = ttcore::lock_state(&source.parent().unwrap().join("service.lock"))?;
    let input = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .c(d!("open source read-only"))?;
    let snapshot = input
        .unchecked_transaction()
        .c(d!("read source snapshot"))?;
    let check: String = snapshot
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .c(d!())?;
    if check != "ok" {
        return Err(eg!("source integrity check failed"));
    }
    let objects = {
        let mut statement = snapshot
            .prepare(
                "SELECT type,name FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY name",
            )
            .c(d!())?;
        statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .c(d!())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .c(d!())?
    };
    if objects
        != [
            ("table".into(), "_meta".into()),
            ("table".into(), "vms".into()),
        ]
    {
        return Err(eg!("source is not the supported native v5 schema"));
    }
    for (table, columns) in [("_meta", vec!["key", "value"]), ("vms", vec!["id", "data"])] {
        let statement = snapshot
            .prepare(&format!("SELECT * FROM {table}"))
            .c(d!())?;
        if statement.column_names() != columns {
            return Err(eg!("unexpected source table columns"));
        }
    }
    let metadata = {
        let mut statement = snapshot
            .prepare("SELECT key,value FROM _meta ORDER BY key")
            .c(d!())?;
        statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .c(d!())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .c(d!())?
    };
    if !metadata
        .iter()
        .any(|(key, value)| key == "schema_version" && value == "5")
    {
        return Err(eg!("conversion requires agent schema v5"));
    }
    let host_id = metadata
        .iter()
        .find(|(key, value)| key == "host_id" && !value.is_empty())
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| eg!("source has no stable host identity"))?;
    let records = {
        let mut statement = snapshot
            .prepare("SELECT id,data FROM vms ORDER BY id")
            .c(d!())?;
        statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .c(d!())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .c(d!())?
    };
    let mut converted = Vec::with_capacity(records.len());
    for (id, data) in records {
        // Validate with the target's actual model, retaining every old JSON field.
        // Materialize the new revision once; a mere schema-marker edit would
        // generate a different default revision on every subsequent read.
        let vm: Vm = serde_json::from_str(&data).c(d!("invalid source VM record"))?;
        let mut value: serde_json::Value = serde_json::from_str(&data).c(d!())?;
        let object = value
            .as_object_mut()
            .ok_or_else(|| eg!("VM must be an object"))?;
        if vm.id != id || vm.host_id != host_id || object.contains_key("backup") {
            return Err(eg!(
                "VM identity mismatch or unexpected backup state in v5 source"
            ));
        }
        object.insert("backup".into(), serde_json::to_value(&vm.backup).c(d!())?);
        converted.push((id, value.to_string()));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary =
        tempfile::NamedTempFile::new_in(parent).c(d!("create private conversion file"))?;
    {
        let mut destination = Connection::open(temporary.path()).c(d!())?;
        destination
            .execute_batch("PRAGMA synchronous=FULL;")
            .c(d!())?;
        let transaction = destination.transaction().c(d!())?;
        transaction.execute_batch("CREATE TABLE _meta (key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE vms (id TEXT PRIMARY KEY,data TEXT NOT NULL);").c(d!())?;
        for (key, value) in metadata {
            let value = if key == "schema_version" {
                "6".into()
            } else {
                value
            };
            transaction
                .execute("INSERT INTO _meta VALUES (?1,?2)", params![key, value])
                .c(d!())?;
        }
        for (id, data) in &converted {
            transaction
                .execute("INSERT INTO vms VALUES (?1,?2)", params![id, data])
                .c(d!())?;
        }
        transaction.commit().c(d!("commit converted metadata"))?;
        let check: String = destination
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .c(d!())?;
        if check != "ok" {
            return Err(eg!("converted database integrity check failed"));
        }
    }
    temporary.as_file().sync_all().c(d!())?;
    temporary
        .persist_noclobber(output)
        .c(d!("publish new database without overwrite"))?;
    std::fs::File::open(parent).c(d!())?.sync_all().c(d!())?;
    Ok(converted.len())
}

fn main() {
    let args = Args::parse();
    match convert(&args.source, &args.output) {
        Ok(count) => println!(
            "Converted {count} VM records to a new v6 database; source and runtime disks were not changed"
        ),
        Err(error) => {
            eprintln!("Offline conversion failed: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(directory: &Path, version: &str, invalid: bool) -> PathBuf {
        let path = directory.join("agent.db");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE _meta (key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE vms (id TEXT PRIMARY KEY,data TEXT NOT NULL);").unwrap();
        db.execute("INSERT INTO _meta VALUES ('schema_version',?1),('host_id','retained-host'),('network_config','retained-network')", [version]).unwrap();
        let vm = serde_json::json!({"id":"retained-vm","env_id":"retained-env","host_id":"retained-host","image":"retained-image","engine":if invalid {"unknown"} else {"firecracker"},"cpu":2,"mem":1024,"disk":8196,"ip":"10.10.0.2","port_map":{"22":20022},"state":"running","created_at":123,"options":{"ports":[22],"ssh_keys":["fixture-public-key"],"deny_outgoing":false,"requested_disk":8192,"isolated_network":true,"guest_config_digest":"retained-digest"},"future_opaque_field":"preserved"});
        db.execute(
            "INSERT INTO vms VALUES ('retained-vm',?1)",
            [vm.to_string()],
        )
        .unwrap();
        path
    }

    #[test]
    fn preserves_identity_and_materializes_stable_native_backup_state() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path(), "5", false);
        let before = std::fs::read(&source).unwrap();
        let output = dir.path().join("converted.db");
        assert_eq!(convert(&source, &output).unwrap(), 1);
        assert_eq!(std::fs::read(&source).unwrap(), before);
        let db = Connection::open(&output).unwrap();
        let raw: String = db
            .query_row("SELECT data FROM vms", [], |r| r.get(0))
            .unwrap();
        let first: Vm = serde_json::from_str(&raw).unwrap();
        let second: Vm = serde_json::from_str(&raw).unwrap();
        assert_eq!(first.backup.revision, second.backup.revision);
        assert!(!first.backup.has_artifacts());
        assert_eq!(first.reserved_disk(), 8196);
        assert_eq!(first.id, "retained-vm");
        let original = Connection::open(&source).unwrap();
        let previous: String = original
            .query_row("SELECT data FROM vms", [], |r| r.get(0))
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        value.as_object_mut().unwrap().remove("backup");
        assert_eq!(
            value,
            serde_json::from_str::<serde_json::Value>(&previous).unwrap()
        );
        let meta: Vec<(String, String)> = db
            .prepare("SELECT key,value FROM _meta ORDER BY key")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            meta,
            [
                ("host_id".into(), "retained-host".into()),
                ("network_config".into(), "retained-network".into()),
                ("schema_version".into(), "6".into())
            ]
        );
        assert!(convert(&source, &output).is_err());
    }

    #[test]
    fn refuses_incompatible_records_without_publishing_or_changing_source() {
        for (version, invalid) in [("4", false), ("6", false), ("5", true)] {
            let dir = tempfile::tempdir().unwrap();
            let source = fixture(dir.path(), version, invalid);
            let before = std::fs::read(&source).unwrap();
            let output = dir.path().join("converted.db");
            assert!(convert(&source, &output).is_err());
            assert!(!output.exists());
            assert_eq!(std::fs::read(source).unwrap(), before);
        }
    }

    #[test]
    fn refuses_an_agent_that_still_owns_the_state_lock() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path(), "5", false);
        let _lock = ttcore::lock_state(&dir.path().join("service.lock")).unwrap();
        assert!(convert(&source, &dir.path().join("converted.db")).is_err());
    }

    #[test]
    fn refuses_a_host_identity_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture(dir.path(), "5", false);
        Connection::open(&source)
            .unwrap()
            .execute(
                "UPDATE _meta SET value='another-host' WHERE key='host_id'",
                [],
            )
            .unwrap();
        assert!(convert(&source, &dir.path().join("converted.db")).is_err());
        assert!(!dir.path().join("converted.db").exists());
    }
}
