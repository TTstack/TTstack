//! Strict disk snapshots/reflinks. No file-copy or conversion fallback exists.
use crate::backup::{Backend, Generation, Pending};
use crate::command::CommandExt;
use crate::model::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt as UnixCommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, String>;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone)]
pub struct Context {
    pub vm_id: String,
    pub engine: Engine,
    pub backend: Backend,
    /// A file path or the exact root zvol dataset.
    pub disk: String,
    pub root: PathBuf,
    pub lock_path: PathBuf,
    pub dependencies: Vec<PathBuf>,
    pub options_digest: String,
    pub config_disk: bool,
    pub deadline: std::time::Instant,
}

#[derive(Debug)]
pub struct Witness {
    pub root_bytes: u64,
    pub disk_identity: String,
    pub dependencies: String,
}

/// The lock is inherited by storage children, so a surviving child fences restart.
pub struct WorkerLock(nix::fcntl::Flock<File>);

impl Context {
    pub fn lock(&self) -> Result<WorkerLock> {
        crate::lock_state(&self.lock_path)
            .map(WorkerLock)
            .map_err(|_| "conflict: backup storage writer is still active; inspect or retry".into())
    }

    fn command(&self, lock: &WorkerLock, program: &str, args: &[&str]) -> Result<String> {
        let fd = lock.0.as_raw_fd();
        let remaining = self
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(
                "backup work budget exhausted; inspect and retry the same operation".into(),
            );
        }
        let mut command = Command::new(program);
        command.args(args);
        // SAFETY: pre_exec only performs async-signal-safe fcntl on an owned FD.
        // Clearing CLOEXEC in the child retains the same open file description.
        unsafe {
            command.pre_exec(move || {
                if nix::libc::fcntl(fd, nix::libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = command
            .output_timeout(remaining.min(std::time::Duration::from_secs(60)))
            .map_err(err)?;
        if output.stdout.len() >= 4 * 1024 * 1024 || output.stderr.len() >= 4 * 1024 * 1024 {
            return Err(
                "backup storage output reached its safety bound; inventory is unconfirmed".into(),
            );
        }
        if !output.status.success() {
            return Err(format!(
                "{program} failed: {}",
                String::from_utf8_lossy(&output.stderr)
                    .trim()
                    .chars()
                    .take(4096)
                    .collect::<String>()
            ));
        }
        String::from_utf8(output.stdout)
            .map(|s| s.trim().to_owned())
            .map_err(err)
    }

    fn property(&self, lock: &WorkerLock, object: &str, name: &str) -> Result<String> {
        self.command(lock, "zfs", &["get", "-Hp", "-o", "value", name, object])
    }

    pub fn inspect(&self, lock: &WorkerLock, probe: bool) -> Result<Witness> {
        let mut dependencies = self.dependencies.clone();
        if self.engine == Engine::Firecracker && self.backend == Backend::Zvol {
            let parent = self
                .disk
                .strip_suffix("/rootfs")
                .ok_or("invalid Firecracker volume")?;
            let directory = self.property(lock, parent, "mountpoint")?;
            if !Path::new(&directory).is_absolute()
                || self.property(lock, parent, "mounted")? != "yes"
            {
                return Err("conflict: Firecracker kernel/config dataset must be mounted".into());
            }
            dependencies.push(Path::new(&directory).join("vmlinux"));
            if self.config_disk {
                dependencies.push(Path::new(&directory).join(crate::guest_config::CONFIG_DISK));
            }
        }
        let mut digest = Sha256::new();
        digest.update(self.options_digest.as_bytes());
        for path in &dependencies {
            let mut file = regular(path)?;
            digest.update(
                path.file_name()
                    .ok_or("invalid dependency path")?
                    .as_encoded_bytes(),
            );
            let mut chunk = [0; 16384];
            loop {
                let n = file.read(&mut chunk).map_err(err)?;
                if n == 0 {
                    break;
                }
                digest.update(&chunk[..n]);
            }
        }
        let dependencies = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let (root_bytes, disk_identity) = match self.backend {
            Backend::Zvol => {
                if self.property(lock, &self.disk, "type")? != "volume" {
                    return Err("backup unsupported: root disk is not a zvol".into());
                }
                let size = self
                    .property(lock, &self.disk, "volsize")?
                    .parse::<u64>()
                    .map_err(err)?;
                let id = self.property(lock, &self.disk, "guid")?;
                if probe {
                    if self.property(lock, &self.disk, "readonly")? != "off"
                        || self.property(lock, &self.disk, "sync")? == "disabled"
                    {
                        return Err("backup unsupported: zvol must be writable with synchronous writes enabled".into());
                    }
                    let available = self
                        .property(lock, &self.disk, "available")?
                        .parse::<u64>()
                        .map_err(err)?;
                    if available < 16 * 1024 * 1024 {
                        return Err("conflict: insufficient storage headroom for backup".into());
                    }
                }
                (size, id)
            }
            Backend::Reflink => {
                let file = regular(Path::new(&self.disk))?;
                let meta = file.metadata().map_err(err)?;
                let root_meta = std::fs::symlink_metadata(&self.root).map_err(err)?;
                if !root_meta.is_dir() || root_meta.dev() != meta.dev() {
                    return Err(
                        "backup unsupported: disk and backup location require the same filesystem"
                            .into(),
                    );
                }
                let size = if self.engine == Engine::Qemu {
                    let data =
                        self.command(lock, "qemu-img", &["info", "--output=json", &self.disk])?;
                    let info: serde_json::Value = serde_json::from_str(&data).map_err(err)?;
                    qemu_capacity(&info)?
                } else {
                    meta.len()
                };
                if probe {
                    let fs = nix::sys::statvfs::statvfs(&self.root).map_err(err)?;
                    if fs.blocks_available().saturating_mul(fs.fragment_size()) < 16 * 1024 * 1024 {
                        return Err("conflict: insufficient storage headroom for backup".into());
                    }
                    probe_reflink(&self.root)?;
                }
                (size, file_identity(&file)?)
            }
        };
        if root_bytes == 0 {
            return Err("invalid empty root disk".into());
        }
        Ok(Witness {
            root_bytes,
            disk_identity,
            dependencies,
        })
    }

    fn directory(&self) -> Result<PathBuf> {
        crate::model::validate_name(&self.vm_id, "vm_id")?;
        private_dir(&self.root.join(".tt-backups"))?;
        let path = self.root.join(".tt-backups").join(&self.vm_id);
        private_dir(&path)?;
        Ok(path)
    }

    fn artifact_path(&self, id: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(id).map_err(|_| "invalid backup generation")?;
        Ok(self.directory()?.join(id))
    }

    fn snapshot(&self, generation: &Generation) -> Result<String> {
        uuid::Uuid::parse_str(&generation.id).map_err(|_| "invalid backup generation")?;
        Ok(format!("{}@ttbackup-{}", self.disk, generation.id))
    }

    fn snapshots(&self, lock: &WorkerLock) -> Result<Vec<(String, String, String)>> {
        let text = self.command(
            lock,
            "zfs",
            &[
                "list",
                "-Hpr",
                "-d",
                "1",
                "-t",
                "snapshot",
                "-o",
                "name,guid,ttstack:backup",
                "-s",
                "createtxg",
                &self.disk,
            ],
        )?;
        text.lines()
            .map(|line| {
                let values: Vec<_> = line.split('\t').collect();
                if values.len() != 3 {
                    return Err("invalid ZFS snapshot listing".into());
                }
                Ok((values[0].into(), values[1].into(), values[2].into()))
            })
            .collect()
    }

    pub fn verify(&self, lock: &WorkerLock, generation: &Generation) -> Result<Generation> {
        let mut result = generation.clone();
        result.identity = match generation.backend {
            Backend::Zvol => {
                let name = self.snapshot(generation)?;
                let (_, id, owner) = self
                    .snapshots(lock)?
                    .into_iter()
                    .find(|v| v.0 == name)
                    .ok_or("not found: backup snapshot is missing")?;
                if owner != format!("{}:{}", self.vm_id, generation.id) {
                    return Err("conflict: backup snapshot ownership changed".into());
                }
                id
            }
            Backend::Reflink => {
                let dir = self.artifact_path(&generation.id)?;
                self.check_owner(&dir, &generation.id)?;
                let manifest: Manifest =
                    serde_json::from_reader(regular(&dir.join("ready.json"))?).map_err(err)?;
                let file = regular(&dir.join("disk"))?;
                if manifest.vm_id != self.vm_id
                    || manifest.generation.id != generation.id
                    || manifest.generation.root_bytes != generation.root_bytes
                    || manifest.generation.dependencies != generation.dependencies
                    || manifest.generation.identity != file_identity(&file)?
                {
                    return Err("conflict: backup file identity or manifest changed".into());
                }
                manifest.generation.identity
            }
        };
        if !generation.identity.is_empty() && generation.identity != result.identity {
            return Err("conflict: backup artifact was replaced".into());
        }
        Ok(result)
    }

    fn check_owner(&self, dir: &Path, id: &str) -> Result<()> {
        let meta = std::fs::symlink_metadata(dir).map_err(err)?;
        if !meta.is_dir() {
            return Err("conflict: backup directory is not a directory".into());
        }
        let mut owner = String::new();
        regular(&dir.join("owner"))?
            .read_to_string(&mut owner)
            .map_err(err)?;
        if owner != format!("{}:{id}", self.vm_id) {
            return Err("conflict: backup directory ownership changed".into());
        }
        Ok(())
    }

    fn prepare_directory(&self, id: &str) -> Result<PathBuf> {
        let dir = self.artifact_path(id)?;
        match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {
                atomic_write(
                    &dir.join("owner"),
                    format!("{}:{id}", self.vm_id).as_bytes(),
                )?;
                sync_directory(dir.parent().ok_or("invalid backup directory")?)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = std::fs::symlink_metadata(&dir).map_err(err)?;
                if metadata.is_dir()
                    && metadata.permissions().mode() & 0o077 == 0
                    && std::fs::read_dir(&dir).map_err(err)?.next().is_none()
                {
                    // The persisted operation owns this UUID, including interrupted mkdir.
                    atomic_write(
                        &dir.join("owner"),
                        format!("{}:{id}", self.vm_id).as_bytes(),
                    )?;
                } else {
                    self.check_owner(&dir, id)?;
                }
            }
            Err(e) => return Err(err(e)),
        }
        Ok(dir)
    }

    pub fn create(&self, lock: &WorkerLock, pending: &Pending) -> Result<Generation> {
        let generation = &pending.artifact;
        let witness = self.inspect(lock, false)?;
        check_witness(pending, &witness)?;
        match self.backend {
            Backend::Zvol => {
                let name = self.snapshot(generation)?;
                if !self.snapshots(lock)?.iter().any(|v| v.0 == name) {
                    self.command(
                        lock,
                        "zfs",
                        &[
                            "snapshot",
                            "-o",
                            &format!("ttstack:backup={}:{}", self.vm_id, generation.id),
                            &name,
                        ],
                    )?;
                }
                self.verify(lock, generation)
            }
            Backend::Reflink => {
                let dir = self.prepare_directory(&generation.id)?;
                if dir.join("ready.json").try_exists().map_err(err)? {
                    return self.verify(lock, generation);
                }
                remove_regular(&dir.join("disk"))?;
                let source = regular(Path::new(&self.disk))?;
                source.sync_all().map_err(err)?;
                let destination = new_file(&dir.join("disk"))?;
                reflink(&source, &destination)?;
                destination.sync_all().map_err(err)?;
                let mut result = generation.clone();
                result.identity = file_identity(&destination)?;
                atomic_write(
                    &dir.join("ready.json"),
                    &serde_json::to_vec(&Manifest {
                        vm_id: self.vm_id.clone(),
                        generation: result.clone(),
                    })
                    .map_err(err)?,
                )?;
                self.verify(lock, &result)
            }
        }
    }

    pub fn restore(
        &self,
        lock: &WorkerLock,
        pending: &Pending,
        source: &Generation,
    ) -> Result<Option<Generation>> {
        self.verify(lock, source)?;
        let witness = self.inspect(lock, false)?;
        if witness.root_bytes != source.root_bytes || witness.dependencies != source.dependencies {
            return Err("conflict: backup capacity or retained boot dependencies changed".into());
        }
        if self.backend == Backend::Zvol {
            check_witness(pending, &witness)?;
            self.command(lock, "zfs", &["rollback", &self.snapshot(source)?])?;
            return Ok(None);
        }
        let stage = self.prepare_directory(&pending.artifact.id)?;
        let mut replacement = pending.artifact.clone();
        if stage.join("ready.json").try_exists().map_err(err)? {
            let saved: Manifest =
                serde_json::from_reader(regular(&stage.join("ready.json"))?).map_err(err)?;
            if saved.vm_id != self.vm_id || saved.generation.id != replacement.id {
                return Err("conflict: restore staging identity changed".into());
            }
            replacement = saved.generation;
            if witness.disk_identity == replacement.identity {
                // Rename completed before an interrupted database publication.
                regular(Path::new(&self.disk))?.sync_all().map_err(err)?;
                sync_directory(Path::new(&self.disk).parent().ok_or("invalid disk path")?)?;
                return Ok(Some(replacement));
            }
        }
        check_witness(pending, &witness)?;
        if replacement.identity.is_empty() {
            remove_regular(&stage.join("disk"))?;
            let input = regular(&self.artifact_path(&source.id)?.join("disk"))?;
            let output = new_file(&stage.join("disk"))?;
            reflink(&input, &output)?;
            output.sync_all().map_err(err)?;
            replacement.identity = file_identity(&output)?;
            atomic_write(
                &stage.join("ready.json"),
                &serde_json::to_vec(&Manifest {
                    vm_id: self.vm_id.clone(),
                    generation: replacement.clone(),
                })
                .map_err(err)?,
            )?;
        }
        if file_identity(&regular(&stage.join("disk"))?)? != replacement.identity {
            return Err("conflict: restore file was replaced".into());
        }
        std::fs::rename(stage.join("disk"), &self.disk).map_err(err)?;
        sync_directory(Path::new(&self.disk).parent().ok_or("invalid disk path")?)?;
        sync_directory(&stage)?;
        Ok(Some(replacement))
    }

    /// Older retired snapshots do not block restore, even if their cleanup is held.
    /// Only exact owned newer candidates may be removed to permit ZFS rollback.
    pub fn prepare_restore(
        &self,
        lock: &WorkerLock,
        source: &Generation,
        retired: &[Generation],
    ) -> Result<Vec<String>> {
        if self.backend != Backend::Zvol {
            return Ok(vec![]);
        }
        let snapshots = self.snapshots(lock)?;
        let name = self.snapshot(source)?;
        let index = snapshots
            .iter()
            .position(|s| s.0 == name)
            .ok_or("not found: backup snapshot missing")?;
        let mut removed = vec![];
        for (name, _, _) in snapshots.iter().skip(index + 1) {
            let generation = retired
                .iter()
                .find(|g| self.snapshot(g).is_ok_and(|n| n == *name))
                .ok_or("conflict: newer foreign snapshot prevents backup restore")?;
            self.remove(lock, generation)?;
            removed.push(generation.id.clone());
        }
        Ok(removed)
    }

    pub fn remove(&self, lock: &WorkerLock, generation: &Generation) -> Result<()> {
        match generation.backend {
            Backend::Zvol => {
                let name = self.snapshot(generation)?;
                if self.snapshots(lock)?.iter().any(|v| v.0 == name) {
                    self.verify(lock, generation)?;
                    self.command(lock, "zfs", &["destroy", &name])?;
                }
            }
            Backend::Reflink => {
                let dir = self.artifact_path(&generation.id)?;
                if !dir.try_exists().map_err(err)? {
                    return Ok(());
                }
                let metadata = std::fs::symlink_metadata(&dir).map_err(err)?;
                if metadata.is_dir()
                    && metadata.permissions().mode() & 0o077 == 0
                    && std::fs::read_dir(&dir).map_err(err)?.next().is_none()
                {
                    std::fs::remove_dir(&dir).map_err(err)?;
                    return sync_directory(dir.parent().ok_or("invalid backup directory")?);
                }
                self.check_owner(&dir, &generation.id)?;
                if dir.join("disk").try_exists().map_err(err)? {
                    if !generation.identity.is_empty() {
                        self.verify(lock, generation)?;
                    }
                    remove_regular(&dir.join("disk"))?;
                }
                remove_regular(&dir.join("ready.json"))?;
                // Only these owned files are removable; never recursively erase a tree.
                let remaining: Vec<_> = std::fs::read_dir(&dir).map_err(err)?.collect();
                if remaining.len() != 1 {
                    return Err("conflict: unexpected backup directory contents".into());
                }
                remove_regular(&dir.join("owner"))?;
                std::fs::remove_dir(&dir).map_err(err)?;
                sync_directory(dir.parent().ok_or("invalid backup directory")?)?;
            }
        }
        Ok(())
    }
}

fn qemu_capacity(info: &serde_json::Value) -> Result<u64> {
    let details = &info["format-specific"]["data"];
    if info["format"] != "qcow2"
        || info.get("backing-filename").is_some()
        || info["encrypted"].as_bool().unwrap_or(false)
        || info["dirty-flag"].as_bool().unwrap_or(false)
        || details["corrupt"].as_bool().unwrap_or(false)
        || details["encrypt"].is_object()
        || details.get("data-file").is_some()
        || info["snapshots"].as_array().is_some_and(|s| !s.is_empty())
    {
        return Err("backup unsupported: QEMU requires a clean standalone qcow2 without snapshots, encryption or external data".into());
    }
    info["virtual-size"]
        .as_u64()
        .ok_or_else(|| "invalid QEMU disk capacity".into())
}

fn check_witness(pending: &Pending, witness: &Witness) -> Result<()> {
    if witness.disk_identity != pending.disk_identity
        || witness.root_bytes != pending.artifact.root_bytes
        || witness.dependencies != pending.artifact.dependencies
    {
        return Err("conflict: disk identity, capacity or boot dependencies changed".into());
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    vm_id: String,
    generation: Generation,
}

fn regular(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)
        .map_err(err)?;
    if !file.metadata().map_err(err)?.is_file() {
        return Err("conflict: backup requires a regular file".into());
    }
    Ok(file)
}
fn new_file(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(err)
}
fn file_identity(file: &File) -> Result<String> {
    let meta = file.metadata().map_err(err)?;
    Ok(format!("{}:{}", meta.dev(), meta.ino()))
}
fn private_dir(path: &Path) -> Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => sync_directory(path.parent().ok_or("invalid backup root")?)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(err(e)),
    }
    let meta = std::fs::symlink_metadata(path).map_err(err)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err("conflict: backup directory must be private and not a symbolic link".into());
    }
    Ok(())
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path).and_then(|f| f.sync_all()).map_err(err)
}
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("invalid metadata path")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(err)?;
    file.write_all(data).map_err(err)?;
    file.as_file().sync_all().map_err(err)?;
    file.persist(path).map_err(err)?;
    sync_directory(parent)
}
fn remove_regular(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => std::fs::remove_file(path).map_err(err),
        Ok(_) => Err("conflict: refusing to remove non-regular backup file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(err(e)),
    }
}
#[cfg(target_os = "linux")]
fn reflink(source: &File, target: &File) -> Result<()> {
    // SAFETY: Linux FICLONE uses two live file descriptors and no pointer argument.
    let result =
        unsafe { nix::libc::ioctl(target.as_raw_fd(), nix::libc::FICLONE, source.as_raw_fd()) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(nix::libc::EOPNOTSUPP | nix::libc::EXDEV | nix::libc::EINVAL | nix::libc::ENOTTY)
        ) {
            return Err(format!(
                "backup unsupported: filesystem cannot create a strict reflink ({error})"
            ));
        }
        return Err(err(error));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn reflink(_: &File, _: &File) -> Result<()> {
    Err("backup unsupported: strict file reflinks are only implemented on Linux; use zvol storage for bhyve".into())
}

pub fn probe_reflink(root: &Path) -> Result<()> {
    let mut source = tempfile::tempfile_in(root).map_err(err)?;
    let mut target = tempfile::tempfile_in(root).map_err(err)?;
    source.write_all(b"backup-probe").map_err(err)?;
    reflink(&source, &target)?;
    target.write_all(b"changed").map_err(err)?;
    use std::io::{Seek, SeekFrom};
    source.seek(SeekFrom::Start(0)).map_err(err)?;
    let mut contents = Vec::new();
    source.read_to_end(&mut contents).map_err(err)?;
    if contents != b"backup-probe" {
        return Err("backup unsupported: cloned data is not independent".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::{self, Action, Request};

    fn context(root: &Path) -> Context {
        Context {
            vm_id: "owned-vm".into(),
            engine: Engine::Firecracker,
            backend: Backend::Reflink,
            disk: root.join("rootfs.ext4").to_string_lossy().into(),
            root: root.into(),
            lock_path: root.join("worker.lock"),
            dependencies: vec![],
            options_digest: String::new(),
            config_disk: false,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
        }
    }
    fn generation() -> Generation {
        Generation {
            id: backup::token(),
            backend: Backend::Reflink,
            root_bytes: 4096,
            created_at: 1,
            identity: String::new(),
            dependencies: String::new(),
            restore_staging: false,
        }
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn unsupported_reflink_leaves_source_and_destination_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::write(&source, b"retained disk").unwrap();
        std::fs::write(&target, b"existing destination").unwrap();
        let error =
            reflink(&File::open(&source).unwrap(), &File::open(&target).unwrap()).unwrap_err();
        assert!(error.starts_with("backup unsupported:"));
        assert_eq!(std::fs::read(source).unwrap(), b"retained disk");
        assert_eq!(std::fs::read(target).unwrap(), b"existing destination");
        assert!(
            probe_reflink(dir.path())
                .unwrap_err()
                .starts_with("backup unsupported:")
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn qcow2_corruption_and_external_dependencies_are_not_publishable_backups() {
        let clean = serde_json::json!({"format":"qcow2", "virtual-size":1048576,
            "format-specific":{"data":{"corrupt":false}}});
        assert_eq!(qemu_capacity(&clean).unwrap(), 1048576);
        for (name, value) in [
            ("corrupt", serde_json::json!(true)),
            ("encrypt", serde_json::json!({"format":"luks"})),
            ("data-file", serde_json::json!("external.raw")),
        ] {
            let mut info = clean.clone();
            info["format-specific"]["data"][name] = value;
            assert!(
                qemu_capacity(&info)
                    .unwrap_err()
                    .starts_with("backup unsupported:")
            );
        }
    }

    #[test]
    fn backup_cleanup_rejects_foreign_files_and_recovers_an_empty_tombstone() {
        let temp = tempfile::tempdir().unwrap();
        let context = context(temp.path());
        let lock = context.lock().unwrap();
        let generation = generation();
        let dir = context.prepare_directory(&generation.id).unwrap();
        std::fs::write(dir.join("foreign"), b"keep").unwrap();
        assert!(context.remove(&lock, &generation).is_err());
        assert_eq!(std::fs::read(dir.join("foreign")).unwrap(), b"keep");
        std::fs::remove_file(dir.join("foreign")).unwrap();
        std::fs::remove_file(dir.join("owner")).unwrap();
        context.remove(&lock, &generation).unwrap();
        context.remove(&lock, &generation).unwrap();
        assert!(!dir.exists());
        let foreign = temp.path().join("foreign-dir");
        std::fs::create_dir(&foreign).unwrap();
        std::os::unix::fs::symlink(&foreign, &dir).unwrap();
        assert!(context.remove(&lock, &generation).is_err());
        assert!(foreign.is_dir());
    }

    #[test]
    fn reflink_unsupported_never_falls_back_and_supported_restore_keeps_source() {
        let temp = tempfile::tempdir().unwrap();
        let context = context(temp.path());
        std::fs::write(&context.disk, [b'A'; 4096]).unwrap();
        let lock = context.lock().unwrap();
        let witness = context.inspect(&lock, false).unwrap();
        let mut artifact = generation();
        artifact.dependencies = witness.dependencies;
        let pending = Pending {
            request: Request {
                operation_id: backup::token(),
                expected_revision: backup::token(),
                action: Action::Create,
                generation: None,
            },
            artifact,
            disk_identity: witness.disk_identity,
            error: None,
        };
        let backup = match context.create(&lock, &pending) {
            Ok(generation) => generation,
            Err(e) => {
                assert!(e.starts_with("backup unsupported:"), "{e}");
                let dir = context.artifact_path(&pending.artifact.id).unwrap();
                assert!(!dir.join("ready.json").exists());
                assert_eq!(std::fs::metadata(dir.join("disk")).unwrap().len(), 0);
                assert_eq!(std::fs::read(&context.disk).unwrap(), [b'A'; 4096]);
                context.remove(&lock, &pending.artifact).unwrap();
                return;
            }
        };
        std::fs::write(&context.disk, [b'B'; 4096]).unwrap();
        let witness = context.inspect(&lock, false).unwrap();
        let mut restore = pending.clone();
        restore.request.action = Action::Restore;
        restore.request.generation = Some(backup.id.clone());
        restore.artifact.id = backup::token();
        restore.artifact.restore_staging = true;
        restore.disk_identity = witness.disk_identity;
        let stage = context.restore(&lock, &restore, &backup).unwrap().unwrap();
        assert_eq!(std::fs::read(&context.disk).unwrap(), [b'A'; 4096]);
        // Lost publication: repeat the same rename/apply attempt while fenced.
        context.restore(&lock, &restore, &backup).unwrap();
        context.remove(&lock, &stage).unwrap();
        std::fs::write(&context.disk, [b'C'; 4096]).unwrap();
        assert_eq!(
            std::fs::read(context.artifact_path(&backup.id).unwrap().join("disk")).unwrap(),
            [b'A'; 4096]
        );
        context.remove(&lock, &backup).unwrap();
    }

    #[test]
    fn storage_child_retains_exclusion_when_agent_process_dies() {
        const CHILD: &str = "TT_BACKUP_LOCK_CHILD";
        if let Some(root) = std::env::var_os(CHILD) {
            let context = context(Path::new(&root));
            let lock = context.lock().unwrap();
            context
                .command(
                    &lock,
                    "sh",
                    &[
                        "-c",
                        "printf ready > \"$1\"; sleep 1",
                        "backup-child",
                        context.root.join("ready").to_str().unwrap(),
                    ],
                )
                .unwrap();
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let context = context(temp.path());
        let mut process = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::backup::tests::storage_child_retains_exclusion_when_agent_process_dies",
            ])
            .env(CHILD, temp.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        while !temp.path().join("ready").exists() {
            assert!(start.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        process.kill().unwrap();
        process.wait().unwrap();
        assert!(
            context.lock().is_err(),
            "orphan storage child must retain the lock"
        );
        while context.lock().is_err() {
            assert!(start.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn truncated_inventory_output_cannot_prove_a_backup_absent() {
        let temp = tempfile::tempdir().unwrap();
        let context = context(temp.path());
        let lock = context.lock().unwrap();
        let error = context
            .command(&lock, "sh", &["-c", "head -c 4194304 /dev/zero"])
            .unwrap_err();
        assert!(error.contains("inventory is unconfirmed"));
    }
}
