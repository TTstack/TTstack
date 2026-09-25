//! File-based storage backend.
//!
//! Uses plain file/directory copies for image provisioning. Works on
//! any filesystem. On Linux with CoW filesystems, `cp --reflink=auto`
//! makes copies near-instant.

use super::ImageStore;
use crate::command::CommandExt;
use ruc::*;
use std::path::Path;

pub struct FileStore;

/// Grow an offline, unpartitioned ext4 clone before its first boot.
/// The caller keeps a failed clone for diagnosis/explicit deletion.
pub fn resize_ext4(path: &Path, size_mib: u32) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).c(d!("inspect ext4 clone"))?;
    if !metadata.is_file() {
        return Err(eg!("rootfs.ext4 must be a regular file"));
    }
    let requested = u64::from(size_mib) * 1024 * 1024;
    if requested < metadata.len() {
        return Err(eg!("requested disk is smaller than the base image"));
    }
    if requested == metadata.len() {
        return Ok(());
    }
    check_ext4(path)?;
    let disk = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .c(d!("open ext4 clone"))?;
    disk.set_len(requested).c(d!("grow ext4 clone"))?;
    grow_ext4(path)?;
    disk.sync_all().c(d!("sync ext4 clone"))?;
    Ok(())
}

pub(super) fn check_ext4(path: &Path) -> Result<()> {
    let check = std::process::Command::new("e2fsck")
        .args(["-f", "-p"])
        .arg(path)
        .bounded_output()
        .c(d!("check ext4 clone"))?;
    // e2fsck uses 1 for successfully corrected filesystem errors.
    if !matches!(check.status.code(), Some(0 | 1)) {
        return Err(eg!(
            "e2fsck failed: {} {}",
            String::from_utf8_lossy(&check.stdout),
            String::from_utf8_lossy(&check.stderr)
        ));
    }
    Ok(())
}

pub(super) fn grow_ext4(path: &Path) -> Result<()> {
    let resize = std::process::Command::new("resize2fs")
        .arg(path)
        .bounded_output()
        .c(d!("grow ext4 filesystem"))?;
    if !resize.status.success() {
        return Err(eg!(
            "resize2fs failed: {}",
            String::from_utf8_lossy(&resize.stderr)
        ));
    }
    Ok(())
}

impl ImageStore for FileStore {
    fn clone_image(&self, base: &str, target: &str) -> Result<()> {
        // Restrict the destination before copying; cp -a would preserve public modes.
        let metadata = std::fs::symlink_metadata(base).c(d!("inspect base image"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            if metadata.is_dir() {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(target)
                    .c(d!("private clone directory"))?;
            } else if metadata.is_file() {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(target)
                    .c(d!("private clone disk"))?;
            } else {
                return Err(eg!("base image must be a regular file or directory"));
            }
        }
        let source = if metadata.is_dir() {
            format!("{base}/.")
        } else {
            base.to_owned()
        };
        let mut cmd = std::process::Command::new("cp");
        #[cfg(target_os = "linux")]
        cmd.args([
            "--reflink=auto",
            "-R",
            "--no-preserve=mode,ownership",
            &source,
            target,
        ]);
        #[cfg(not(target_os = "linux"))]
        cmd.args(["-R", &source, target]);
        let output = cmd.bounded_output().c(d!("cp image"))?;

        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("image copy failed: {}", err));
        }

        Ok(())
    }

    fn remove_image(&self, path: &str) -> Result<()> {
        let p = Path::new(path);
        if p.is_dir() {
            std::fs::remove_dir_all(p).c(d!("remove dir"))?;
        } else if p.exists() {
            std::fs::remove_file(p).c(d!("remove file"))?;
        }
        Ok(())
    }

    fn list_images(&self, base_dir: &str) -> Result<Vec<String>> {
        let dir = Path::new(base_dir);
        if !dir.is_dir() {
            return Ok(vec![]);
        }

        let mut images = Vec::new();
        for entry in std::fs::read_dir(dir).c(d!("read image dir"))? {
            let entry = entry.c(d!("read dir entry"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with('.') && !name.starts_with("clone-") {
                images.push(name);
            }
        }
        images.sort();
        Ok(images)
    }

    fn image_exists(&self, path: &str) -> Result<bool> {
        Ok(Path::new(path).exists())
    }

    fn resolve_disk(&self, clone_path: &str) -> String {
        let p = Path::new(clone_path);
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(p) {
                let mut files: Vec<_> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file())
                    .collect();
                files.sort_by_key(|f| f.file_name());
                // Deterministic selection is preserved across cold starts.
                // Prefer .qcow2 file
                if let Some(qcow2) = files
                    .iter()
                    .find(|f| f.path().extension().is_some_and(|ext| ext == "qcow2"))
                {
                    return qcow2.path().to_string_lossy().into_owned();
                }
                // Single file — use it directly
                if files.len() == 1 {
                    return files[0].path().to_string_lossy().into_owned();
                }
            }
            format!("{clone_path}/disk.qcow2")
        } else {
            clone_path.to_string()
        }
    }

    fn disk_format(&self) -> &'static str {
        "qcow2"
    }

    fn resize_disk(&self, clone_path: &str, size_mib: u32) -> Result<()> {
        let path = self.resolve_disk(clone_path);
        let info = std::process::Command::new("qemu-img")
            .args(["info", "--output=json", &path])
            .bounded_output()
            .c(d!("inspect disk"))?;
        if !info.status.success() {
            return Err(eg!(
                "qemu-img info: {}",
                String::from_utf8_lossy(&info.stderr)
            ));
        }
        let info: serde_json::Value = serde_json::from_slice(&info.stdout).c(d!("disk info"))?;
        let size = info["virtual-size"]
            .as_u64()
            .ok_or_else(|| eg!("missing virtual disk size"))?;
        let format = info["format"]
            .as_str()
            .ok_or_else(|| eg!("missing disk format"))?;
        if format != "qcow2" {
            return Err(eg!("file storage currently requires a qcow2 QEMU image"));
        }
        let requested = u64::from(size_mib) * 1024 * 1024;
        if requested < size {
            return Err(eg!(
                "requested disk is smaller than the base image; choose at least {} MiB",
                size.div_ceil(1024 * 1024)
            ));
        }
        if requested > size {
            let output = std::process::Command::new("qemu-img")
                .args(["resize", "-f", format, &path, &requested.to_string()])
                .bounded_output()
                .c(d!("resize disk"))?;
            if !output.status.success() {
                return Err(eg!(
                    "resize disk: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn public_base_images_produce_private_clones() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        for directory in [false, true] {
            let base = dir
                .path()
                .join(if directory { "base-dir" } else { "base-file" });
            let target = dir
                .path()
                .join(if directory { "clone-dir" } else { "clone-file" });
            if directory {
                std::fs::create_dir(&base).unwrap();
                std::fs::write(base.join("disk.qcow2"), b"guest").unwrap();
            } else {
                std::fs::write(&base, b"guest").unwrap();
            }
            std::fs::set_permissions(
                &base,
                std::fs::Permissions::from_mode(if directory { 0o755 } else { 0o644 }),
            )
            .unwrap();
            FileStore
                .clone_image(base.to_str().unwrap(), target.to_str().unwrap())
                .unwrap();
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_ne!(
                std::fs::metadata(&base).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

    #[test]
    fn ext4_growth_preserves_files_and_rejects_shrinking() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.ext4");
        let clone = dir.path().join("clone.ext4");
        let contents = dir.path().join("contents");
        std::fs::create_dir(&contents).unwrap();
        std::fs::write(contents.join("marker"), "persistent-data").unwrap();
        std::fs::File::create(&base)
            .unwrap()
            .set_len(64 * 1024 * 1024)
            .unwrap();
        let mkfs = std::process::Command::new("mkfs.ext4")
            .args(["-q", "-F", "-d"])
            .arg(&contents)
            .arg(&base)
            .output()
            .unwrap();
        assert!(
            mkfs.status.success(),
            "{}",
            String::from_utf8_lossy(&mkfs.stderr)
        );
        FileStore
            .clone_image(base.to_str().unwrap(), clone.to_str().unwrap())
            .unwrap();
        resize_ext4(&clone, 96).unwrap();
        assert_eq!(std::fs::metadata(&base).unwrap().len(), 64 * 1024 * 1024);
        assert_eq!(std::fs::metadata(&clone).unwrap().len(), 96 * 1024 * 1024);
        let fs = std::process::Command::new("dumpe2fs")
            .arg("-h")
            .arg(&clone)
            .output()
            .unwrap();
        assert!(fs.status.success());
        let header = String::from_utf8(fs.stdout).unwrap();
        let value = |key: &str| -> u64 {
            header
                .lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap()
                .trim()
                .parse()
                .unwrap()
        };
        assert_eq!(
            value("Block count:") * value("Block size:"),
            96 * 1024 * 1024
        );
        let read = std::process::Command::new("debugfs")
            .args(["-R", "cat /marker"])
            .arg(&clone)
            .output()
            .unwrap();
        assert!(read.status.success());
        assert_eq!(read.stdout, b"persistent-data");
        resize_ext4(&clone, 96).unwrap();
        assert!(resize_ext4(&clone, 64).is_err());
        assert_eq!(std::fs::metadata(&clone).unwrap().len(), 96 * 1024 * 1024);
        let invalid = dir.path().join("invalid.ext4");
        std::fs::write(&invalid, b"not an ext4 filesystem").unwrap();
        assert!(resize_ext4(&invalid, 64).is_err());
        assert_eq!(std::fs::read(invalid).unwrap(), b"not an ext4 filesystem");
    }

    #[test]
    fn clone_and_remove_file() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.img");
        let clone = dir.path().join("clone.img");
        std::fs::write(&base, b"image-data").unwrap();

        let store = FileStore;
        store
            .clone_image(base.to_str().unwrap(), clone.to_str().unwrap())
            .unwrap();
        assert!(clone.exists());
        assert_eq!(std::fs::read(&clone).unwrap(), b"image-data");

        store.remove_image(clone.to_str().unwrap()).unwrap();
        assert!(!clone.exists());
    }

    #[test]
    fn clone_and_remove_directory() {
        let dir = tempfile::tempdir().unwrap();
        let base_dir = dir.path().join("base");
        let clone_dir = dir.path().join("clone");
        std::fs::create_dir(&base_dir).unwrap();
        std::fs::write(base_dir.join("disk.qcow2"), b"data").unwrap();

        let store = FileStore;
        store
            .clone_image(base_dir.to_str().unwrap(), clone_dir.to_str().unwrap())
            .unwrap();
        assert!(clone_dir.join("disk.qcow2").exists());

        store.remove_image(clone_dir.to_str().unwrap()).unwrap();
        assert!(!clone_dir.exists());
    }

    #[test]
    fn list_images_filters_clones_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ubuntu"), b"").unwrap();
        std::fs::write(dir.path().join("alpine"), b"").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"").unwrap();
        std::fs::write(dir.path().join("clone-abc"), b"").unwrap();

        let store = FileStore;
        let images = store.list_images(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(images, vec!["alpine", "ubuntu"]);
    }

    #[test]
    fn list_images_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore;
        let images = store.list_images(dir.path().to_str().unwrap()).unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn list_images_nonexistent_dir() {
        let store = FileStore;
        let images = store.list_images("/no/such/path").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn image_exists_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("img");
        let store = FileStore;
        assert!(!store.image_exists(path.to_str().unwrap()).unwrap());
        std::fs::write(&path, b"").unwrap();
        assert!(store.image_exists(path.to_str().unwrap()).unwrap());
    }

    #[test]
    fn remove_nonexistent_is_ok() {
        let store = FileStore;
        store.remove_image("/no/such/file").unwrap();
    }

    #[test]
    fn name_is_file() {
        assert_eq!(FileStore.name(), "file");
    }

    #[test]
    fn disk_format_is_qcow2() {
        assert_eq!(FileStore.disk_format(), "qcow2");
    }

    #[test]
    fn resolve_disk_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("image.qcow2");
        std::fs::write(&file, b"fake").unwrap();
        let resolved = FileStore.resolve_disk(file.to_str().unwrap());
        assert_eq!(resolved, file.to_str().unwrap());
    }

    #[test]
    fn resolve_disk_dir_with_qcow2() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("disk.qcow2"), b"fake").unwrap();
        std::fs::write(dir.path().join("other.txt"), b"other").unwrap();
        let resolved = FileStore.resolve_disk(dir.path().to_str().unwrap());
        assert!(resolved.ends_with("disk.qcow2"));
    }

    #[test]
    fn resolve_disk_dir_single_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("myimage"), b"fake").unwrap();
        let resolved = FileStore.resolve_disk(dir.path().to_str().unwrap());
        assert!(resolved.ends_with("myimage"));
    }

    #[test]
    fn resolve_disk_dir_fallback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"fake").unwrap();
        std::fs::write(dir.path().join("b"), b"fake").unwrap();
        let resolved = FileStore.resolve_disk(dir.path().to_str().unwrap());
        assert!(resolved.ends_with("disk.qcow2"));
    }

    #[test]
    fn resolve_disk_empty_dir_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = FileStore.resolve_disk(dir.path().to_str().unwrap());
        assert!(resolved.ends_with("disk.qcow2"));
    }
}
