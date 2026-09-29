//! Docker / Podman container engine implementation.
//!
//! The agent binds one runtime to its persistent inventory; lifecycle calls never switch it.

#[cfg(target_os = "linux")]
use super::VmEngine;
#[cfg(target_os = "linux")]
use crate::command::CommandExt;
#[cfg(target_os = "linux")]
use crate::model::{Vm, VmState};
#[cfg(target_os = "linux")]
use ruc::*;
#[cfg(target_os = "linux")]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Docker,
    Podman,
}
impl ContainerRuntime {
    pub fn command(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
    pub fn available(self) -> bool {
        #[cfg(target_os = "linux")]
        {
            Command::new(self.command())
                .arg("--version")
                .output_timeout(std::time::Duration::from_secs(5))
                .is_ok_and(|output| output.status.success())
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}
impl std::str::FromStr for ContainerRuntime {
    type Err = String;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            _ => Err("container runtime must be docker or podman".into()),
        }
    }
}

#[cfg(target_os = "linux")]
pub struct DockerEngine {
    runtime: ContainerRuntime,
}

#[cfg(target_os = "linux")]
impl DockerEngine {
    pub fn new(runtime: ContainerRuntime) -> Self {
        Self { runtime }
    }

    fn runtime(&self) -> &'static str {
        self.runtime.command()
    }

    fn inspect(&self, vm: &Vm) -> Result<Option<VmState>> {
        let output = Command::new(self.runtime())
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

#[cfg(target_os = "linux")]
impl VmEngine for DockerEngine {
    fn create(
        &self,
        vm: &Vm,
        _image_path: &str,
        _disk_format: &str,
        _ssh_keys: &[String],
    ) -> Result<()> {
        let name = Self::container_name(vm);
        let rt = self.runtime();

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
        let output = Command::new(self.runtime())
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
                self.runtime()
            ));
        }
        Ok(())
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        // A failed pull can leave a durable VM record without a container.
        // Only confirmed absence is success; daemon/query failures still propagate.
        if self.inspect(vm)?.is_none() {
            return Ok(());
        }
        let name = Self::container_name(vm);
        let output = Command::new(self.runtime())
            .args(["stop", "-t", "10", &name])
            .bounded_output()
            .c(d!())?;

        if !output.status.success() {
            // The container may also disappear between inspect and stop.
            if self.inspect(vm)?.is_none() {
                return Ok(());
            }
            return Err(eg!(
                "container stop failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        if self.inspect(vm)?.is_none() {
            return Ok(());
        }
        let name = Self::container_name(vm);
        // Force remove the container
        let output = Command::new(self.runtime())
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn bound_runtime_never_deletes_from_another_container_store() {
        const CHILD: &str = "TT_RUNTIME_BINDING_TEST_DIR";
        if let Some(path) = std::env::var_os(CHILD) {
            let dir = std::path::PathBuf::from(path);
            let vm: Vm = serde_json::from_value(serde_json::json!({
                "id":"owned", "env_id":"test", "host_id":"test", "image":"test",
                "engine":"docker", "cpu":1, "mem":128, "disk":0, "ip":"",
                "port_map":{}, "state":"running", "created_at":0
            }))
            .unwrap();
            assert!(ContainerRuntime::Docker.available());
            let engine = DockerEngine::new(ContainerRuntime::Podman);
            engine.stop(&vm).unwrap();
            engine.destroy(&vm).unwrap();
            assert!(!dir.join("podman-owned").exists());
            assert!(dir.join("docker-owned").exists());
            std::fs::remove_file(dir.join("podman")).unwrap();
            assert!(engine.stop(&vm).is_err());
            assert!(engine.destroy(&vm).is_err());
            assert!(dir.join("docker-owned").exists());
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        for runtime in ["docker", "podman"] {
            let script = dir.path().join(runtime);
            std::fs::write(dir.path().join(format!("{runtime}-owned")), b"running").unwrap();
            std::fs::write(&script, format!(r#"#!/bin/sh
case "$1" in
  --version) exit 0 ;;
  inspect) if [ -e "$TT_RUNTIME_BINDING_TEST_DIR/{runtime}-owned" ]; then echo running; else echo 'No such container' >&2; exit 1; fi ;;
  stop) exit 0 ;;
  rm) /bin/rm "$TT_RUNTIME_BINDING_TEST_DIR/{runtime}-owned" ;;
  *) exit 2 ;;
esac
"#)).unwrap();
            std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "engine::docker::tests::bound_runtime_never_deletes_from_another_container_store",
                "--nocapture",
            ])
            .env(CHILD, dir.path())
            .env("PATH", dir.path())
            .output_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn missing_container_cleanup_does_not_hide_daemon_or_stop_errors() {
        const CHILD: &str = "TT_DOCKER_CLEANUP_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let mut vm: Vm = serde_json::from_value(serde_json::json!({
                "id": "missing", "env_id": "test", "host_id": "test",
                "image": "missing:latest", "engine": "docker",
                "cpu": 1, "mem": 128, "disk": 0, "ip": "",
                "port_map": {}, "state": "failed", "created_at": 0
            }))
            .unwrap();
            let engine = DockerEngine::new(ContainerRuntime::Docker);
            for _ in 0..2 {
                engine.stop(&vm).unwrap();
                engine.destroy(&vm).unwrap();
            }
            vm.id = "unavailable".into();
            assert!(engine.stop(&vm).is_err());
            assert!(engine.destroy(&vm).is_err());
            vm.id = "running".into();
            assert!(engine.stop(&vm).is_err());
            return;
        }

        // Run the real command boundary in a child, without changing process-wide PATH.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let docker = dir.path().join("docker");
        std::fs::write(
            &docker,
            r#"#!/bin/sh
if [ "$1" = --version ]; then exit 0; fi
for arg; do name="$arg"; done
case "$1:$name" in
  inspect:tt-missing) echo 'No such container: tt-missing' >&2; exit 1 ;;
  inspect:tt-unavailable) echo 'Cannot connect to the Docker daemon' >&2; exit 1 ;;
  inspect:tt-running) echo running; exit 0 ;;
  stop:tt-running) echo 'permission denied' >&2; exit 1 ;;
  *) echo 'unexpected mutating command' >&2; exit 2 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "engine::docker::tests::missing_container_cleanup_does_not_hide_daemon_or_stop_errors", "--nocapture"])
            .env(CHILD, "1")
            .env("PATH", dir.path())
            .output_timeout(std::time::Duration::from_secs(10)).unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
