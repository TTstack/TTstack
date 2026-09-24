//! Experimental FreeBSD support; outside the primary validation scope.
//! Bhyve engine implementation (FreeBSD only).
//!
//! Bhyve is the native hypervisor on FreeBSD. This module is only
//! compiled on FreeBSD targets via `#[cfg(target_os = "freebsd")]`.

use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use crate::net;
use ruc::*;
use std::process::Command;

pub struct BhyveEngine;

impl BhyveEngine {
    pub fn new() -> Self {
        Self
    }

    fn pid_path(vm: &Vm) -> String {
        format!("{RUN_DIR}/bhyve-{}.pid", vm.id)
    }

    fn read_pid(vm: &Vm) -> Option<i32> {
        std::fs::read_to_string(Self::pid_path(vm))
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }
}

impl VmEngine for BhyveEngine {
    fn create(
        &self,
        vm: &Vm,
        image_path: &str,
        _disk_format: &str,
        _ssh_keys: &[String],
    ) -> Result<()> {
        // Load the VM into bhyve via bhyveload
        let output = Command::new("bhyveload")
            .args(["-m", &format!("{}M", vm.mem)])
            .args(["-d", image_path])
            .arg(&vm.id)
            .bounded_output()
            .c(d!("failed to run bhyveload"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("bhyveload failed: {}", stderr));
        }

        // Launch the VM as a background daemon
        let tap = net::tap_name(&vm.id);
        let pid_path = format!("{RUN_DIR}/bhyve-{}.pid", vm.id);
        std::fs::create_dir_all(RUN_DIR).c(d!("create pid dir"))?;

        let child = Command::new("bhyve")
            .args(["-A", "-H", "-P"])
            .args(["-c", &vm.cpu.to_string()])
            .args(["-m", &format!("{}M", vm.mem)])
            .args(["-s", "0:0,hostbridge"])
            .args(["-s", &format!("3:0,virtio-blk,{image_path}")])
            .args(["-s", &format!("4:0,virtio-net,{tap}")])
            .args(["-s", "31,lpc"])
            .args(["-l", "com1,/dev/null"])
            .arg(&vm.id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .c(d!("failed to spawn bhyve"))?;

        // Record PID
        std::fs::write(&pid_path, child.id().to_string()).c(d!("write pid"))?;

        Ok(())
    }

    fn start(&self, _vm: &Vm) -> Result<()> {
        // Bhyve doesn't support pause/resume natively;
        // "start" after destroy requires re-create.
        Err(eg!(
            "bhyve does not support in-place restart; re-create the VM"
        ))
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        // Kill the bhyve process first, then clean up the VM device
        if let Some(pid) = Self::read_pid(vm) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
            // Give it a moment to exit gracefully
            std::thread::sleep(std::time::Duration::from_millis(500));
        }

        let output = Command::new("bhyvectl")
            .args(["--destroy", "--vm", &vm.id])
            .bounded_output()
            .c(d!("bhyvectl destroy"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Not fatal — the VM device may already be gone
            eprintln!(
                "[bhyve] WARN: bhyvectl --destroy failed for {}: {}",
                vm.id, stderr
            );
        }

        Ok(())
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        // Kill the bhyve process if still alive
        if let Some(pid) = Self::read_pid(vm) {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGKILL,
            );
        }

        // Clean up the bhyve VM device
        let _ = Command::new("bhyvectl")
            .args(["--destroy", "--vm", &vm.id])
            .bounded_output();

        let _ = std::fs::remove_file(Self::pid_path(vm));

        Ok(())
    }

    fn state(&self, vm: &Vm) -> Result<VmState> {
        let output = Command::new("bhyvectl")
            .args(["--get-lowmem", "--vm", &vm.id])
            .bounded_output();

        match output {
            Ok(o) if o.status.success() => Ok(VmState::Running),
            _ => Ok(VmState::Stopped),
        }
    }

    fn name(&self) -> &'static str {
        "bhyve"
    }
}
