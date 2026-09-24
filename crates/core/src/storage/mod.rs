//! Image storage abstraction.
//!
//! Two backends: plain file copies (`FileStore`) and ZFS zvols (`ZvolStore`).

pub mod file;
pub mod zvol;

use crate::model::Storage;
use ruc::*;

/// Trait for image storage operations.
///
/// Implementations handle the mechanics of cloning base images into
/// per-VM working copies and cleaning them up on destruction.
pub trait ImageStore: Send + Sync {
    /// Clone a base image to a new path for a VM instance.
    ///
    /// - `base`: path (or dataset name) of the base / template image
    /// - `target`: desired path (or dataset name) for the VM's working copy
    fn clone_image(&self, base: &str, target: &str) -> Result<()>;

    /// Remove a VM's image clone.
    fn remove_image(&self, path: &str) -> Result<()>;

    /// List available base images under the given directory / dataset.
    fn list_images(&self, base_dir: &str) -> Result<Vec<String>>;

    /// Check whether an image exists at the given path / dataset.
    fn image_exists(&self, path: &str) -> Result<bool>;

    /// Resolve a clone path to the actual disk path the engine should use.
    ///
    /// - `FileStore`: searches the directory for a qcow2 file.
    /// - `ZvolStore`: returns `/dev/zvol/{dataset}`.
    fn resolve_disk(&self, clone_path: &str) -> String;

    /// Disk format string for the engine (e.g. `"qcow2"` or `"raw"`).
    fn disk_format(&self) -> &'static str;

    /// Grow a QEMU disk, rejecting shrink requests.
    fn resize_disk(&self, clone_path: &str, size_mib: u32) -> Result<()>;

    /// Directory containing the Firecracker kernel, root disk and optional config.
    fn firecracker_dir(&self, path: &str) -> Result<String> {
        Ok(path.into())
    }

    /// Logical root disk capacity, excluding the separate configuration drive.
    fn firecracker_size(&self, path: &str) -> Result<u64> {
        let disk = std::fs::symlink_metadata(format!("{path}/rootfs.ext4"))
            .c(d!("inspect Firecracker rootfs"))?;
        if !disk.is_file() {
            return Err(eg!("file storage requires a regular rootfs.ext4"));
        }
        Ok(disk.len())
    }

    /// Grow an offline clone's root disk and its unpartitioned ext4 filesystem.
    fn resize_firecracker(&self, path: &str, size_mib: u32) -> Result<()> {
        file::resize_ext4(&std::path::Path::new(path).join("rootfs.ext4"), size_mib)
    }

    /// Backend name for logging.
    fn name(&self) -> &'static str;
}

/// Create an [`ImageStore`] for the given backend.
pub fn create_store(kind: Storage) -> Box<dyn ImageStore> {
    match kind {
        Storage::File => Box::new(file::FileStore),
        Storage::Zvol => Box::new(zvol::ZvolStore),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_store_names() {
        assert_eq!(create_store(Storage::File).name(), "file");
        assert_eq!(create_store(Storage::Zvol).name(), "zvol");
    }
}
