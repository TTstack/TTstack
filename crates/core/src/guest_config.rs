//! Opaque guest configuration. TTstack never interprets or executes these files.
use crate::model::Engine;
use std::collections::BTreeMap;

pub type GuestConfig = BTreeMap<String, String>;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const CONFIG_DISK_MIB: u32 = 4;
pub const CONFIG_DISK: &str = "guest-config.ext4";

pub fn validate(engine: Engine, files: &GuestConfig, isolated: bool) -> Result<(), String> {
    if !files.is_empty() && engine != Engine::Firecracker {
        return Err("guest_config currently requires Firecracker".into());
    }
    if isolated && !matches!(engine, Engine::Firecracker | Engine::Qemu) {
        return Err("isolated_network requires Linux QEMU or Firecracker".into());
    }
    if files.len() > 32
        || files.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > MAX_CONFIG_BYTES
    {
        return Err("guest_config exceeds 32 files or 64 KiB".into());
    }
    for name in files.keys() {
        if name.is_empty()
            || name.len() > 128
            || name.starts_with('.')
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            // Do not echo arbitrary configuration content into diagnostics.
            return Err("guest_config keys must be simple file names (letters, digits, -, _, .; no leading dot)".into());
        }
    }
    Ok(())
}

pub fn digest(files: &GuestConfig) -> Option<String> {
    use sha2::{Digest, Sha256};
    if files.is_empty() {
        return None;
    }
    // BTreeMap + JSON provide canonical ordering and unambiguous boundaries.
    Some(
        Sha256::digest(serde_json::to_vec(files).expect("string map"))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Build without mounting or executing guest content. Publication is atomic.
#[cfg(target_os = "linux")]
pub fn write_disk(directory: &std::path::Path, files: &GuestConfig) -> ruc::Result<()> {
    use crate::command::CommandExt;
    use ruc::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    validate(Engine::Firecracker, files, false).map_err(|e| eg!(e))?;
    let destination = directory.join(CONFIG_DISK);
    if files.is_empty() {
        match std::fs::remove_file(destination) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).c(d!("remove config disk")),
        }
        return Ok(());
    }
    let contents = tempfile::tempdir_in(directory).c(d!("config staging"))?;
    for (name, value) in files {
        let file = contents.path().join(name);
        std::fs::write(&file, value).c(d!("write guest config"))?;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o400))
            .c(d!("config permissions"))?;
    }
    let disk = tempfile::NamedTempFile::new_in(directory).c(d!("config disk staging"))?;
    disk.as_file()
        .set_len(u64::from(CONFIG_DISK_MIB) * 1024 * 1024)
        .c(d!("config disk size"))?;
    let output = Command::new("mkfs.ext4")
        .args(["-q", "-F", "-L", "TTCONFIG", "-d"])
        .arg(contents.path())
        .arg(disk.path())
        .bounded_output()
        .c(d!("build config disk"))?;
    if !output.status.success() {
        return Err(eg!("mkfs.ext4 failed to build guest configuration disk"));
    }
    disk.as_file().sync_all().c(d!("sync config disk"))?;
    disk.persist(destination).map_err(|e| eg!(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_and_unsupported_engines_without_echoing_secrets() {
        for name in ["../escape", "/absolute", "a/b", ".hidden", "x\nsecret"] {
            let files = GuestConfig::from([(name.into(), "private-content".into())]);
            let error = validate(Engine::Firecracker, &files, false).unwrap_err();
            assert!(!error.contains("private-content"));
        }
        let files = GuestConfig::from([("app.json".into(), "{}".into())]);
        assert!(validate(Engine::Firecracker, &files, true).is_ok());
        assert!(validate(Engine::Qemu, &files, false).is_err());
        assert!(validate(Engine::Docker, &GuestConfig::new(), true).is_err());
        assert!(
            validate(
                Engine::Firecracker,
                &GuestConfig::from([("a".into(), "x".repeat(MAX_CONFIG_BYTES))]),
                false
            )
            .is_err()
        );
    }
    #[test]
    fn digest_distinguishes_contents_and_file_boundaries() {
        assert_eq!(digest(&GuestConfig::new()), None);
        assert_ne!(
            digest(&GuestConfig::from([("a".into(), "bc".into())])),
            digest(&GuestConfig::from([("ab".into(), "c".into())]))
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn config_disk_contains_exact_files_and_has_private_host_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let files =
            GuestConfig::from([("app.json".into(), "{\"token\":\"test-secret\"}\n".into())]);
        write_disk(dir.path(), &files).unwrap();
        let disk = dir.path().join(CONFIG_DISK);
        assert_eq!(
            std::fs::metadata(&disk).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let output = std::process::Command::new("debugfs")
            .args(["-R", "cat app.json"])
            .arg(&disk)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), files["app.json"]);
        write_disk(dir.path(), &GuestConfig::new()).unwrap();
        assert!(!disk.exists());
    }
}
