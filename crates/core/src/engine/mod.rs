//! VM / container engine abstraction.
//!
//! Each engine implements [`VmEngine`] to provide a uniform interface
//! for creating, starting, stopping, and destroying instances.
//!
//! Platform-specific engines:
//! - **Linux**: Qemu, Firecracker, Docker/Podman

pub mod docker;
#[cfg(target_os = "linux")]
pub mod firecracker;
#[cfg(target_os = "linux")]
pub mod qemu;

use crate::model::{Engine, Vm, VmState};
use ruc::*;

/// Trait implemented by each hypervisor / container engine.
pub trait VmEngine: Send + Sync {
    /// Create and boot a new VM from the given disk path.
    ///
    /// - `disk_format`: image format (`"qcow2"` for file-based, `"raw"` for zvol).
    /// - `ssh_keys`: root public keys for QEMU cloud-init.
    fn create(
        &self,
        vm: &Vm,
        image_path: &str,
        disk_format: &str,
        ssh_keys: &[String],
    ) -> Result<()>;

    /// Start a previously stopped VM.
    fn start(&self, vm: &Vm) -> Result<()>;

    /// Stop execution and release memory; preserve the disk for restart.
    fn stop(&self, vm: &Vm) -> Result<()>;

    /// Destroy the VM and clean up all associated resources.
    fn destroy(&self, vm: &Vm) -> Result<()>;

    /// Query the current state of the VM.
    fn state(&self, vm: &Vm) -> Result<VmState>;

    /// Human-readable engine name.
    fn name(&self) -> &'static str;
}

/// Create an engine instance for the given [`Engine`] kind.
///
/// Returns an error for unsupported platforms.
pub fn create_engine(kind: Engine) -> Result<Box<dyn VmEngine>> {
    Ok(match kind {
        #[cfg(target_os = "linux")]
        Engine::Qemu => Box::new(qemu::QemuEngine::new()),
        #[cfg(target_os = "linux")]
        Engine::Firecracker => Box::new(firecracker::FirecrackerEngine::new()),
        Engine::Docker => Box::new(docker::DockerEngine::new()),
        #[allow(unreachable_patterns)]
        other => {
            return Err(eg!(format!(
                "engine {other} is not supported on this platform"
            )));
        }
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn process_matches(pid: u32, marker: &str) -> Result<bool> {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(cmdline) => Ok(cmdline
            .split(|c| *c == 0)
            .any(|arg| arg == marker.as_bytes())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).c(d!("read process identity")),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn terminate(pid: u32, marker: &str) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    for signal in [Signal::SIGTERM, Signal::SIGKILL] {
        if !process_matches(pid, marker)? {
            return Ok(());
        }
        match kill(Pid::from_raw(pid as i32), signal) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(e) => return Err(e).c(d!("terminate VM")),
        }
        for _ in 0..100 {
            if !process_matches(pid, marker)? {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    Err(eg!(format!(
        "VM process {pid} did not exit; resources retained"
    )))
}
