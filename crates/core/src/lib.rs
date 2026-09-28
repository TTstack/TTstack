//! TTstack core library.
//!
//! Provides shared types, engine abstractions, storage backends, and
//! network utilities used by both the host agent and central controller.
//!
//! The [`api`] and [`model`] modules are platform-independent and used
//! by all components (CLI, controller, agent).
//!
//! The [`engine`], [`net`], and [`storage`] modules contain host-specific
//! implementations. Linux is primary; FreeBSD support is experimental.

pub mod api;
pub mod auth;
pub mod model;

pub mod engine;
pub mod net;
pub mod storage;

pub mod command;
pub mod guest_config;

/// Hold one writer process per state directory for the lifetime of the service.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub fn lock_state(path: &std::path::Path) -> ruc::Result<nix::fcntl::Flock<std::fs::File>> {
    use ruc::*;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
        .c(d!("open service lock"))?;
    nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock).map_err(|(_, e)| {
        eg!(format!(
            "state directory is already in use or cannot be locked: {e}"
        ))
    })
}

pub mod ssh;
#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod lock_tests {
    #[test]
    fn state_lock_excludes_a_second_writer_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("service.lock");
        let first = super::lock_state(&path).unwrap();
        assert!(super::lock_state(&path).is_err());
        drop(first);
        assert!(super::lock_state(&path).is_ok());
    }
}
