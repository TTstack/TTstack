//! TTstack core library.
//!
//! Provides shared types, engine abstractions, storage backends, and
//! network utilities used by both the host agent and central controller.
//!
//! The [`api`] and [`model`] modules are platform-independent and used
//! by all components (CLI, controller, agent).
//!
//! The [`engine`], [`net`], and [`storage`] modules contain host-specific
//! implementations for Linux hosts.

pub mod api;
pub mod auth;
pub mod model;

pub mod engine;
pub mod net;
pub mod storage;

pub mod command;
pub mod guest_config;

/// Hold one writer process per state directory for the lifetime of the service.
#[cfg(target_os = "linux")]
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
