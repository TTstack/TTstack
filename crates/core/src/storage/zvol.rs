//! ZFS zvol storage backend.
//!
//! Uses ZFS volumes (zvols) as raw block devices for VMs. Each base
//! image is a zvol; VM copies are instant clones via snapshots.
//!
//! Only provisioning and deletion are exposed.

use super::ImageStore;
use crate::command::CommandExt;
use ruc::*;
use std::process::{Command, Stdio};

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
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl ZvolStore {
    fn ensure_clone_snap(dataset: &str) -> Result<()> {
        let snap = format!("{dataset}@{CLONE_SNAP}");
        if !zfs_ok(&["list", "-t", "snapshot", &snap]) {
            zfs_cmd(&["snapshot", &snap])?;
        }
        Ok(())
    }
}

// ── ImageStore implementation ───────────────────────────────────────

impl ImageStore for ZvolStore {
    fn clone_image(&self, base: &str, target: &str) -> Result<()> {
        Self::ensure_clone_snap(base)?;
        let snap = format!("{base}@{CLONE_SNAP}");
        zfs_cmd(&["clone", &snap, target])?;
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
        let out = zfs_cmd(&["list", "-H", "-o", "name", "-r", "-t", "volume", base_dir])?;

        Ok(out
            .lines()
            .filter(|l| !l.is_empty() && *l != base_dir)
            .filter_map(|l| l.rsplit('/').next())
            .map(String::from)
            .collect())
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
        let current: u64 = zfs_cmd(&["get", "-Hp", "-o", "value", "volsize", clone_path])?
            .parse()
            .c(d!("zvol size"))?;
        let requested = u64::from(size_mib) * 1024 * 1024;
        if requested < current {
            return Err(eg!("requested disk is smaller than base zvol"));
        }
        if requested > current {
            zfs_cmd(&["set", &format!("volsize={requested}"), clone_path])?;
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "zvol"
    }
}
