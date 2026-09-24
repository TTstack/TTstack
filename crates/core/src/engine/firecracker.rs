//! Firecracker with a jailed, resource-limited VMM and persistent guest disks.
use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use ruc::*;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

mod sandbox;
use sandbox::{Sandbox, private_write, remove_file};

pub struct FirecrackerEngine;
impl Default for FirecrackerEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn configuration(vm: &Vm, mac: &str) -> serde_json::Value {
    let mut drives = vec![serde_json::json!({
        "drive_id": "rootfs", "path_on_host": "/rootfs.ext4",
        "is_root_device": true, "is_read_only": false
    })];
    if vm.options.guest_config_digest.is_some() {
        drives.push(serde_json::json!({
            "drive_id": "config", "path_on_host": "/guest-config.ext4",
            "is_root_device": false, "is_read_only": true
        }));
    }
    serde_json::json!({
        "boot-source": {
            "kernel_image_path": "/vmlinux",
            "boot_args": format!("console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/init ip={}::10.10.0.1:255.255.0.0::eth0:off", vm.ip)
        },
        "drives": drives,
        // Cold guests need a source of entropy before applications can safely
        // use getrandom(); waiting for incidental device activity can take minutes.
        "entropy": { "rate_limiter": { "bandwidth": { "size": 4096, "refill_time": 100 } } },
        "machine-config": { "vcpu_count": vm.cpu, "mem_size_mib": vm.mem },
        "network-interfaces": [{ "iface_id": "eth0", "host_dev_name": crate::net::tap_name(&vm.id), "guest_mac": mac }]
    })
}

impl FirecrackerEngine {
    pub fn new() -> Self {
        Self
    }
    fn pid_path(vm: &Vm) -> String {
        format!("{RUN_DIR}/fc-{}.pid", vm.id)
    }
    fn identity(vm: &Vm) -> Result<(String, String)> {
        if let Some(sandbox) = Sandbox::load(vm)? {
            return Ok((sandbox.socket(), sandbox.marker));
        }
        let path = format!("{RUN_DIR}/fc-{}.sock", vm.id);
        Ok((path.clone(), path))
    }
    fn read_pid(vm: &Vm) -> Result<Option<u32>> {
        match std::fs::read_to_string(Self::pid_path(vm)) {
            Ok(s) => Ok(Some(
                s.trim().parse::<u32>().c(d!("invalid Firecracker pid"))?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).c(d!("read Firecracker pid")),
        }
    }
    fn request(socket: &str, method: &str, path: &str, body: Option<&str>) -> Result<Vec<u8>> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "--unix-socket",
            socket,
            "--fail",
            "--max-time",
            "5",
            "-sS",
            "-X",
            method,
        ])
        .arg(format!("http://localhost{path}"));
        if let Some(body) = body {
            cmd.args(["-H", "Content-Type: application/json", "-d", body]);
        }
        let output = cmd
            .output_timeout(Duration::from_secs(6))
            .c(d!("Firecracker API"))?;
        if !output.status.success() {
            return Err(eg!(
                "Firecracker API {method} {path} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(output.stdout)
    }
    fn wait_exit(pid: u32, marker: &str, timeout: Duration) -> Result<bool> {
        let start = std::time::Instant::now();
        loop {
            if !super::process_matches(pid, marker)? {
                return Ok(true);
            }
            if start.elapsed() >= timeout {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl VmEngine for FirecrackerEngine {
    fn create(
        &self,
        vm: &Vm,
        image_path: &str,
        _disk_format: &str,
        _ssh_keys: &[String],
    ) -> Result<()> {
        std::fs::create_dir_all(RUN_DIR).c(d!("create runtime dir"))?;
        if let Some(pid) = Self::read_pid(vm)? {
            let (_, marker) = Self::identity(vm)?;
            if super::process_matches(pid, &marker)? {
                return Err(eg!(
                    "Firecracker is still running; refusing duplicate launch"
                ));
            }
        }
        if let Some(old) = Sandbox::load(vm)? {
            old.cleanup()?;
        }
        let (sandbox, mut command) = Sandbox::prepare(vm, Path::new(image_path))?;
        let log_path = format!("{RUN_DIR}/fc-{}.log", vm.id);
        private_write(Path::new(&log_path), b"")?;
        let log = std::fs::OpenOptions::new()
            .append(true)
            .open(log_path)
            .c(d!("console log"))?;
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stderr(log.try_clone().c(d!("console stderr"))?)
            .stdout(log)
            .spawn()
            .c(d!("spawn Firecracker jailer"))?;
        let pid = child.id();
        if let Err(e) = private_write(Path::new(&Self::pid_path(vm)), pid.to_string().as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let result = wait_for_boot(&mut child, Duration::from_secs(10), || {
            Path::new(&sandbox.socket()).exists() && matches!(self.state(vm), Ok(VmState::Running))
        });
        // Reap the owned child on every outcome, including a readiness timeout.
        // A timeout leaves the VMM and its disks available for inspection.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        result
    }

    fn start(&self, vm: &Vm) -> Result<()> {
        let (socket, _) = Self::identity(vm)?;
        Self::request(&socket, "PATCH", "/vm", Some(r#"{"state":"Resumed"}"#)).map(|_| ())
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        let Some(pid) = Self::read_pid(vm)? else {
            return Ok(());
        };
        let (socket, marker) = Self::identity(vm)?;
        if !super::process_matches(pid, &marker)? {
            return Ok(());
        }
        if matches!(self.state(vm), Ok(VmState::Paused)) {
            self.start(vm)?;
        }
        #[cfg(target_arch = "x86_64")]
        if Self::request(
            &socket,
            "PUT",
            "/actions",
            Some(r#"{"action_type":"SendCtrlAltDel"}"#),
        )
        .is_ok()
            && Self::wait_exit(pid, &marker, Duration::from_secs(30))?
        {
            return Ok(());
        }
        eprintln!(
            "[firecracker] {}: graceful shutdown unavailable or timed out; forcing stop",
            vm.id
        );
        super::terminate(pid, &marker)
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        self.stop(vm)?;
        if let Some(sandbox) = Sandbox::load(vm)? {
            sandbox.cleanup()?;
        }
        for path in [
            Sandbox::path(vm),
            Self::pid_path(vm),
            format!("{RUN_DIR}/fc-{}.sock", vm.id),
            format!("{RUN_DIR}/fc-{}.json", vm.id),
            format!("{RUN_DIR}/fc-{}.log", vm.id),
        ] {
            remove_file(Path::new(&path))?;
        }
        Ok(())
    }

    fn state(&self, vm: &Vm) -> Result<VmState> {
        let Some(pid) = Self::read_pid(vm)? else {
            return Ok(VmState::Stopped);
        };
        let (socket, marker) = Self::identity(vm)?;
        if !super::process_matches(pid, &marker)? {
            return Ok(VmState::Stopped);
        }
        let body: serde_json::Value =
            serde_json::from_slice(&Self::request(&socket, "GET", "/", None)?)
                .c(d!("Firecracker status"))?;
        match body["state"].as_str() {
            Some("Running") => Ok(VmState::Running),
            Some("Paused") => Ok(VmState::Paused),
            Some("Not started") => Ok(VmState::Creating),
            _ => Err(eg!("unexpected Firecracker state")),
        }
    }
    fn name(&self) -> &'static str {
        "firecracker"
    }
}

// Jailer execs into Firecracker. /proc/PID/cmdline can be temporarily empty
// during exec; a missing identity marker is not evidence that our child exited.
fn wait_for_boot(
    child: &mut std::process::Child,
    timeout: Duration,
    mut ready: impl FnMut() -> bool,
) -> Result<()> {
    let started = std::time::Instant::now();
    while started.elapsed() < timeout {
        if child
            .try_wait()
            .c(d!("inspect Firecracker child"))?
            .is_some()
        {
            return Err(eg!(
                "Firecracker jailer exited during boot; inspect the VM console log"
            ));
        }
        if ready() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(eg!(
        "Firecracker did not become ready; resources retained for inspection"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_waits_for_readiness_without_treating_missing_marker_as_exit() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        assert!(!super::super::process_matches(child.id(), "/not-yet-visible.sock").unwrap());
        let mut probes = 0;
        let result = wait_for_boot(&mut child, Duration::from_secs(2), || {
            probes += 1;
            probes >= 2
        });
        let alive = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(result.is_ok());
        assert!(alive);
        assert_eq!(probes, 2);
    }

    #[test]
    fn startup_reports_actual_exit_before_accepting_readiness() {
        let mut child = Command::new("sh").args(["-c", "exit 7"]).spawn().unwrap();
        child.wait().unwrap();
        let result = wait_for_boot(&mut child, Duration::from_secs(1), || true);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exited during boot")
        );
    }

    #[test]
    fn startup_timeout_retains_a_live_child_for_inspection() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let result = wait_for_boot(&mut child, Duration::from_millis(30), || false);
        let alive = child.try_wait().unwrap().is_none();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("resources retained")
        );
        assert!(alive);
    }
}
