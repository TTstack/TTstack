//! Jailer resources: persistent file hard links or private block-device nodes.
use super::*;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, chown};

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Sandbox {
    pub root: std::path::PathBuf,
    pub marker: String,
    pub cgroup: std::path::PathBuf,
}

impl Sandbox {
    pub fn path(vm: &Vm) -> String {
        format!("{RUN_DIR}/fc-{}.sandbox.json", vm.id)
    }
    pub fn load(vm: &Vm) -> Result<Option<Self>> {
        match std::fs::read(Self::path(vm)) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).c(d!("read Firecracker sandbox"))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).c(d!("read Firecracker sandbox")),
        }
    }
    pub fn socket(&self) -> String {
        self.root
            .join(self.marker.trim_start_matches('/'))
            .to_string_lossy()
            .into_owned()
    }

    pub fn prepare(vm: &Vm, image: &Path) -> Result<(Self, Command)> {
        let binary = executable("firecracker")?;
        let jailer = executable("jailer")?;
        if !Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
            return Err(eg!("Firecracker sandbox requires cgroup v2"));
        }
        let image = image.canonicalize().c(d!("Firecracker image path"))?;
        let id = crate::net::tap_name(&vm.id);
        let base = image
            .parent()
            .ok_or_else(|| eg!("invalid image path"))?
            .join(".jailer");
        let root = base
            .join(
                binary
                    .file_name()
                    .ok_or_else(|| eg!("invalid Firecracker binary"))?,
            )
            .join(&id)
            .join("root");
        let marker = format!("/fc-{id}.sock");
        let sandbox = Self {
            root,
            marker,
            cgroup: Path::new("/sys/fs/cgroup/ttstack").join(&id),
        };
        if sandbox.socket().len() >= 108 {
            return Err(eg!(
                "runtime directory is too long for a Firecracker Unix socket; use a shorter --runtime-dir"
            ));
        }
        std::fs::create_dir_all(&sandbox.root).c(d!("create Firecracker sandbox"))?;
        let [a, b, c, d] = vm
            .ip
            .parse::<std::net::Ipv4Addr>()
            .c(d!("guest IP"))?
            .octets();
        let uid = 100_000 + u32::from(c) * 256 + u32::from(d);
        private_write(
            Path::new(&Self::path(vm)),
            &serde_json::to_vec(&sandbox).c(d!("sandbox metadata"))?,
        )?;
        for name in ["vmlinux", "rootfs.ext4"].into_iter().chain(
            vm.options
                .guest_config_digest
                .as_ref()
                .map(|_| crate::guest_config::CONFIG_DISK),
        ) {
            let source = image.join(name);
            let target = sandbox.root.join(name);
            remove_file(&target)?;
            stage_member(&source, &target, name == "rootfs.ext4")?;
            chown(&target, Some(uid), Some(uid)).c(d!("guest file ownership"))?;
            std::fs::set_permissions(
                &target,
                std::fs::Permissions::from_mode(if name == "rootfs.ext4" { 0o600 } else { 0o400 }),
            )
            .c(d!("guest file permissions"))?;
        }
        remove_file(Path::new(&sandbox.socket()))?;
        let config = super::configuration(vm, &format!("02:54:{a:02x}:{b:02x}:{c:02x}:{d:02x}"));
        let config_path = sandbox.root.join("config.json");
        private_write(
            &config_path,
            &serde_json::to_vec(&config).c(d!("Firecracker config"))?,
        )?;
        chown(config_path, Some(uid), Some(uid)).c(d!("config ownership"))?;
        crate::net::prepare_jailed_tap(&vm.id, uid)?;
        let mut command = Command::new(jailer);
        command
            .args(["--id", &id, "--exec-file"])
            .arg(binary)
            .args([
                "--uid",
                &uid.to_string(),
                "--gid",
                &uid.to_string(),
                "--chroot-base-dir",
            ])
            .arg(base)
            .args(["--cgroup-version", "2", "--parent-cgroup", "ttstack"])
            .args([
                "--cgroup",
                &format!("cpu.max={} 100000", u64::from(vm.cpu) * 100_000),
            ])
            .args([
                "--cgroup",
                &format!("memory.max={}", (u64::from(vm.mem) + 128) * 1024 * 1024),
            ])
            .args(["--cgroup", "memory.swap.max=0"])
            .args([
                "--cgroup",
                &format!("pids.max={}", 64 + u64::from(vm.cpu) * 2),
            ])
            .args([
                "--",
                "--api-sock",
                &sandbox.marker,
                "--config-file",
                "/config.json",
            ]);
        Ok((sandbox, command))
    }

    pub fn cleanup(&self) -> Result<()> {
        match std::fs::remove_dir_all(
            self.root
                .parent()
                .ok_or_else(|| eg!("invalid sandbox path"))?,
        ) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).c(d!("remove Firecracker sandbox")),
        }
        match std::fs::remove_dir(&self.cgroup) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).c(d!("remove Firecracker cgroup")),
        }
    }
}

fn stage_member(source: &Path, target: &Path, writable_root: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source).c(d!("guest image member"))?;
    if metadata.is_file() {
        match std::fs::hard_link(source, target) {
            Ok(()) => return Ok(()),
            // ZFS kernel/config files can live in a different dataset from the
            // jail. A writable file root must remain a link to its retained disk.
            Err(e) if !writable_root && e.raw_os_error() == Some(nix::libc::EXDEV) => {
                std::fs::copy(source, target).c(d!("copy read-only jail member"))?;
                return Ok(());
            }
            Err(e) => return Err(e).c(d!("link persistent jail member")),
        }
    }
    let device = std::fs::metadata(source).c(d!("resolve root block device"))?;
    if !writable_root || !device.file_type().is_block_device() {
        return Err(eg!(
            "Firecracker requires regular kernel/config files and a regular or block root disk"
        ));
    }
    // Never chown the host /dev/zvol node or expose the host's /dev directory.
    // Only this VM's root disk is recreated inside its private jail.
    nix::sys::stat::mknod(
        target,
        nix::sys::stat::SFlag::S_IFBLK,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        device.rdev(),
    )
    .c(d!("create jailed root block device"))?;
    Ok(())
}

pub(super) fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| eg!("invalid private file path"))?,
    )
    .c(d!("private staging file"))?;
    file.write_all(bytes).c(d!("write private file"))?;
    file.as_file().sync_all().c(d!("sync private file"))?;
    file.persist(path).map_err(|e| eg!(e.to_string()))?;
    Ok(())
}

fn executable(name: &str) -> Result<std::path::PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(name);
        if std::fs::metadata(&path)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return path.canonicalize().c(d!("resolve executable"));
        }
    }
    Err(eg!(
        "Firecracker requires matching firecracker and jailer binaries in PATH"
    ))
}

pub(super) fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).c(d!("remove runtime file")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_root_keeps_the_persistent_inode() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("rootfs.ext4");
        let target = dir.path().join("jailed-root");
        std::fs::write(&source, b"before").unwrap();
        stage_member(&source, &target, true).unwrap();
        std::fs::write(&target, b"after").unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"after");
    }

    #[test]
    fn rejects_symlinked_regular_members_and_character_devices() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("link");
        let target = dir.path().join("jailed");
        std::os::unix::fs::symlink("/dev/null", &source).unwrap();
        assert!(stage_member(&source, &target, true).is_err());
        assert!(stage_member(&source, &target, false).is_err());
        std::fs::remove_file(&source).unwrap();
        std::fs::write(dir.path().join("file"), b"kernel").unwrap();
        std::os::unix::fs::symlink(dir.path().join("file"), &source).unwrap();
        assert!(stage_member(&source, &target, false).is_err());
        assert!(stage_member(&source, &target, true).is_err());
        assert!(!target.exists());
    }
}
