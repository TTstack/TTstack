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
        Ok(cmdline) if cmdline.is_empty() => {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            match stat {
                Ok(stat)
                    if stat
                        .rsplit_once(") ")
                        .is_some_and(|(_, tail)| tail.starts_with("Z ")) =>
                {
                    Ok(false)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                _ => Err(eg!(format!(
                    "process {pid} has no readable identity; resources retained"
                ))),
            }
        }
        Ok(cmdline) => Ok(cmdline
            .split(|c| *c == 0)
            .any(|arg| arg == marker.as_bytes())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).c(d!("read process identity")),
    }
}

/// Recover missing or corrupt PID metadata using a VM-specific argv path.
/// Never interpret an unreadable PID file as proof of process exit.
#[cfg(target_os = "linux")]
pub(crate) fn recover_pid(path: &str, markers: &[String]) -> Result<Option<u32>> {
    let recorded = match std::fs::read_to_string(path) {
        Ok(s) => s
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|p| *p > 0 && *p <= i32::MAX as u32),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).c(d!("read VM PID")),
    };
    if let Some(pid) = recorded {
        for marker in markers {
            if process_matches(pid, marker)? {
                return Ok(Some(pid));
            }
        }
    }
    let mut found = None;
    for entry in std::fs::read_dir("/proc").c(d!("recover VM PID"))? {
        let entry = entry.c(d!("read process entry"))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).c(d!("recover process identity")),
        };
        if markers
            .iter()
            .any(|m| cmdline.split(|c| *c == 0).any(|a| a == m.as_bytes()))
        {
            if found.is_some() {
                return Err(eg!(
                    "multiple processes match VM identity; resources retained"
                ));
            }
            found = Some(pid);
        }
    }
    Ok(found)
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

#[cfg(all(test, target_os = "linux"))]
mod recovery_tests {
    use super::*;
    #[test]
    fn corrupt_and_missing_pid_files_recover_the_exact_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vm.pid");
        let marker = dir.path().join("specific-vm.sock").display().to_string();
        let mut child = std::process::Command::new("bash")
            .args(["-c", "exec -a \"$1\" sleep 20", "test", &marker])
            .spawn()
            .unwrap();
        let result = || {
            std::fs::write(&path, b"truncated").unwrap();
            assert_eq!(
                recover_pid(path.to_str().unwrap(), std::slice::from_ref(&marker)).unwrap(),
                Some(child.id())
            );
            std::fs::remove_file(&path).unwrap();
            assert_eq!(
                recover_pid(path.to_str().unwrap(), std::slice::from_ref(&marker)).unwrap(),
                Some(child.id())
            );
            terminate(child.id(), &marker).unwrap();
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(result));
        let _ = child.kill();
        let _ = child.wait();
        outcome.unwrap();
        assert!(
            recover_pid(path.to_str().unwrap(), &[marker])
                .unwrap()
                .is_none()
        );
    }
}
