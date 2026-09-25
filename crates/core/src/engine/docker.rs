//! Docker / Podman container engine implementation.
//!
//! Auto-detects whether `docker` or `podman` is available and uses
//! whichever is found (preferring Docker).

use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{Vm, VmState};
use ruc::*;
use std::process::Command;
use std::sync::LazyLock;

/// Cached path to the container runtime binary.
///
/// Prefer `docker` when available since it has wider ecosystem
/// compatibility; fall back to `podman` for rootless operation.
static RUNTIME: LazyLock<&'static str> = LazyLock::new(|| {
    if Command::new("docker")
        .arg("--version")
        .bounded_output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        "docker"
    } else {
        "podman"
    }
});

pub struct DockerEngine;

impl Default for DockerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerEngine {
    pub fn new() -> Self {
        Self
    }

    fn runtime() -> &'static str {
        &RUNTIME
    }

    fn inspect(&self, vm: &Vm) -> Result<Option<VmState>> {
        let output = Command::new(Self::runtime())
            .args([
                "inspect",
                "-f",
                "{{.State.Status}}",
                &Self::container_name(vm),
            ])
            .output_timeout(std::time::Duration::from_secs(10))
            .c(d!("inspect container"))?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            let lower = error.to_lowercase();
            if lower.contains("no such object")
                || lower.contains("no such container")
                || lower.contains("no container with name or id")
            {
                return Ok(None);
            }
            return Err(eg!(format!("container inspect failed: {error}")));
        }
        Ok(Some(match String::from_utf8_lossy(&output.stdout).trim() {
            "running" => VmState::Running,
            "paused" => VmState::Paused,
            "exited" | "created" => VmState::Stopped,
            _ => VmState::Failed,
        }))
    }

    /// Container name derived from VM id.
    fn container_name(vm: &Vm) -> String {
        format!("tt-{}", vm.id)
    }
}

impl VmEngine for DockerEngine {
    fn create(
        &self,
        vm: &Vm,
        _image_path: &str,
        _disk_format: &str,
        _ssh_keys: &[String],
    ) -> Result<()> {
        let name = Self::container_name(vm);
        let rt = Self::runtime();

        let mut cmd = Command::new(rt);
        cmd.args(["run", "-d", "--name", &name])
            .args(["--cpus", &vm.cpu.to_string()])
            .args(["--memory", &format!("{}m", vm.mem)])
            .args(["--memory-swap", &format!("{}m", vm.mem)]);

        // Publish port mappings
        for (&guest, &host) in &vm.port_map {
            cmd.args(["-p", &format!("{host}:{guest}")]);
        }

        // The image name is used directly as the container image reference
        cmd.arg(&vm.image);

        let output = cmd.bounded_output().c(d!("failed to spawn container"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("{} run failed: {}", rt, stderr));
        }

        if self.state(vm)? != VmState::Running {
            return Err(eg!(
                "container exited during creation; use an image with a long-running command"
            ));
        }
        Ok(())
    }

    fn start(&self, vm: &Vm) -> Result<()> {
        let name = Self::container_name(vm);
        let action = if self.state(vm)? == VmState::Paused {
            "unpause"
        } else {
            "start"
        };
        let output = Command::new(Self::runtime())
            .args([action, &name])
            .bounded_output()
            .c(d!())?;

        if !output.status.success() {
            return Err(eg!(
                "container {action} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        if self.state(vm)? != VmState::Running {
            return Err(eg!(
                "container exited during startup; check {} logs {name}",
                Self::runtime()
            ));
        }
        Ok(())
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        let name = Self::container_name(vm);
        let output = Command::new(Self::runtime())
            .args(["stop", "-t", "10", &name])
            .bounded_output()
            .c(d!())?;

        if !output.status.success() {
            return Err(eg!("container stop failed"));
        }
        Ok(())
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        if self.inspect(vm)?.is_none() {
            return Ok(());
        }
        let name = Self::container_name(vm);
        // Force remove the container
        let output = Command::new(Self::runtime())
            .args(["rm", "-f", &name])
            .bounded_output()
            .c(d!())?;

        if !output.status.success() {
            return Err(eg!("container remove failed"));
        }
        Ok(())
    }

    fn state(&self, vm: &Vm) -> Result<VmState> {
        Ok(self.inspect(vm)?.unwrap_or(VmState::Failed))
    }

    fn name(&self) -> &'static str {
        "docker"
    }
}
