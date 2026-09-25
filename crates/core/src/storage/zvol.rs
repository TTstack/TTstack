//! ZFS zvol storage backend.
//!
//! Uses ZFS volumes (zvols) as raw block devices for VMs.
//! QEMU images are volumes. Firecracker images are filesystem datasets containing
//! vmlinux and a child rootfs volume. VM copies are clones of fixed snapshots.
//!
//! Only provisioning and deletion are exposed.

use super::ImageStore;
use crate::command::CommandExt;
use ruc::*;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Fixed snapshot name used for cloning base images.
const CLONE_SNAP: &str = "ttsnap";

pub struct ZvolStore;

// ── Helper: run a zfs command and return stdout or a descriptive error ──

fn zfs_cmd(args: &[&str]) -> Result<String> {
    let output = Command::new("zfs")
        .args(args)
        .bounded_output()
        .c(d!("failed to execute zfs"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eg!("zfs {} failed: {}", args[0], stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn zfs_ok(args: &[&str]) -> bool {
    Command::new("zfs")
        .args(args)
        .bounded_output()
        .map(|s| s.status.success())
        .unwrap_or(false)
}

fn property(dataset: &str, name: &str) -> Result<String> {
    zfs_cmd(&["get", "-Hp", "-o", "value", name, dataset])
}

fn mountpoint(dataset: &str) -> Result<String> {
    let path = property(dataset, "mountpoint")?;
    if !Path::new(&path).is_absolute() || property(dataset, "mounted")? != "yes" {
        return Err(eg!(format!(
            "ZFS dataset {dataset} must have an active absolute mountpoint"
        )));
    }
    Ok(path)
}

fn volume_size(dataset: &str) -> Result<u64> {
    property(dataset, "volsize")?.parse().c(d!("zvol size"))
}

fn wait_device(dataset: &str) -> Result<String> {
    use std::os::unix::fs::FileTypeExt;
    let path = format!("/dev/zvol/{dataset}");
    let started = Instant::now();
    loop {
        if std::fs::metadata(&path).is_ok_and(|m| m.file_type().is_block_device()) {
            return Ok(path);
        }
        if started.elapsed() >= Duration::from_secs(5) {
            return Err(eg!(format!("ZFS block device did not appear: {path}")));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl ZvolStore {
    fn ensure_clone_snap(dataset: &str, recursive: bool) -> Result<()> {
        let snap = format!("{dataset}@{CLONE_SNAP}");
        if !zfs_ok(&["list", "-t", "snapshot", &snap]) {
            if recursive {
                zfs_cmd(&["snapshot", "-r", &snap])?;
            } else {
                zfs_cmd(&["snapshot", &snap])?;
            }
        }
        if recursive
            && !zfs_ok(&[
                "list",
                "-t",
                "snapshot",
                &format!("{dataset}/rootfs@{CLONE_SNAP}"),
            ])
        {
            return Err(eg!(
                "incomplete Firecracker base snapshot; import a new image revision"
            ));
        }
        Ok(())
    }
}

// ── ImageStore implementation ───────────────────────────────────────

impl ImageStore for ZvolStore {
    fn clone_image(&self, base: &str, target: &str) -> Result<()> {
        let firecracker = property(base, "type")? == "filesystem";
        if firecracker {
            self.firecracker_size(base)?;
        }
        Self::ensure_clone_snap(base, firecracker)?;
        let snap = format!("{base}@{CLONE_SNAP}");
        if firecracker {
            let (parent, name) = target
                .rsplit_once('/')
                .ok_or_else(|| eg!("invalid clone dataset"))?;
            let dir = format!("{}/{name}", mountpoint(parent)?);
            // Inherit the runtime location, never the base image's mountpoint.
            zfs_cmd(&[
                "clone",
                "-o",
                &format!("mountpoint={dir}"),
                "-o",
                "readonly=off",
                "-o",
                "canmount=on",
                &snap,
                target,
            ])?;
            zfs_cmd(&[
                "clone",
                "-o",
                "readonly=off",
                "-o",
                "volmode=dev",
                &format!("{base}/rootfs@{CLONE_SNAP}"),
                &format!("{target}/rootfs"),
            ])?;
            let disk = wait_device(&format!("{target}/rootfs"))?;
            // Only a block-device reference lives alongside the kernel/config files.
            std::os::unix::fs::symlink(disk, Path::new(&dir).join("rootfs.ext4"))
                .c(d!("link Firecracker root volume"))?;
        } else {
            zfs_cmd(&[
                "clone",
                "-o",
                "readonly=off",
                "-o",
                "volmode=dev",
                &snap,
                target,
            ])?;
            wait_device(target)?;
        }
        Ok(())
    }

    fn remove_image(&self, path: &str) -> Result<()> {
        let parent = path
            .rsplit_once('/')
            .ok_or_else(|| eg!("expected dataset/clone name"))?
            .0;
        let children = zfs_cmd(&["list", "-H", "-o", "name", "-r", parent])?;
        if children.lines().any(|name| name == path) {
            zfs_cmd(&["destroy", "-r", path])?;
        }
        Ok(())
    }

    fn list_images(&self, base_dir: &str) -> Result<Vec<String>> {
        let out = zfs_cmd(&["list", "-H", "-o", "name,type", "-d", "1", base_dir])?;
        let mut images = Vec::new();
        for line in out.lines() {
            let Some((name, kind)) = line.split_once('\t') else {
                continue;
            };
            if name == base_dir {
                continue;
            }
            if kind == "volume" || (kind == "filesystem" && self.firecracker_size(name).is_ok()) {
                images.push(name.rsplit('/').next().unwrap().to_string());
            }
        }
        images.sort();
        Ok(images)
    }

    fn image_exists(&self, path: &str) -> Result<bool> {
        Ok(zfs_ok(&["list", "-H", path]))
    }

    fn resolve_disk(&self, clone_path: &str) -> String {
        format!("/dev/zvol/{clone_path}")
    }

    fn disk_format(&self) -> &'static str {
        "raw"
    }

    fn resize_disk(&self, clone_path: &str, size_mib: u32) -> Result<()> {
        let current = volume_size(clone_path)?;
        let requested = u64::from(size_mib) * 1024 * 1024;
        if requested < current {
            return Err(eg!("requested disk is smaller than base zvol"));
        }
        if requested > current {
            zfs_cmd(&["set", &format!("volsize={requested}"), clone_path])?;
        }
        Ok(())
    }

    fn firecracker_dir(&self, path: &str) -> Result<String> {
        let dir = mountpoint(path)?;
        // Re-resolve /dev/zvol after reboot; never cache a /dev/zdN minor number.
        wait_device(&format!("{path}/rootfs"))?;
        Ok(dir)
    }

    fn firecracker_size(&self, path: &str) -> Result<u64> {
        let dir = mountpoint(path)?;
        if !std::fs::symlink_metadata(Path::new(&dir).join("vmlinux")).is_ok_and(|m| m.is_file()) {
            return Err(eg!("Firecracker ZFS image requires a regular vmlinux"));
        }
        volume_size(&format!("{path}/rootfs"))
    }

    fn resize_firecracker(&self, path: &str, size_mib: u32) -> Result<()> {
        let volume = format!("{path}/rootfs");
        let size = volume_size(&volume)?;
        let requested = u64::from(size_mib) * 1024 * 1024;
        if requested < size {
            return Err(eg!("requested disk is smaller than the base zvol"));
        }
        if requested == size {
            return Ok(());
        }
        let disk = wait_device(&volume)?;
        super::file::check_ext4(Path::new(&disk))?;
        self.resize_disk(&volume, size_mib)?;
        super::file::grow_ext4(Path::new(&disk))
    }

    fn name(&self) -> &'static str {
        "zvol"
    }
}
