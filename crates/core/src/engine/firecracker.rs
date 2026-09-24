//! Firecracker microVM engine implementation.
//!
//! Uses the Firecracker VMM for lightweight, fast-booting microVMs.
//! Communicates with the Firecracker process via its REST API socket.

use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use ruc::*;
use std::path::Path;
use std::process::Command;

pub struct FirecrackerEngine;

impl Default for FirecrackerEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl FirecrackerEngine {
    pub fn new() -> Self {
        Self
    }

    fn socket_path(vm: &Vm) -> String {
        format!("{RUN_DIR}/fc-{}.sock", vm.id)
    }

    fn pid_path(vm: &Vm) -> String {
        format!("{RUN_DIR}/fc-{}.pid", vm.id)
    }

    fn config_path(vm: &Vm) -> String {
        format!("{RUN_DIR}/fc-{}.json", vm.id)
    }

    fn read_pid(vm: &Vm) -> Result<u32> {
        let path = Self::pid_path(vm);
        let content = std::fs::read_to_string(&path).c(d!("read fc pid"))?;
        content.trim().parse::<u32>().c(d!("invalid pid"))
    }

    fn write_config(&self, vm: &Vm, image_path: &str) -> Result<()> {
        let tap = crate::net::tap_name(&vm.id);
        let config = serde_json::json!({
            "boot-source": {
                "kernel_image_path": format!("{image_path}/vmlinux"),
                "boot_args": format!("console=ttyS0 reboot=k panic=1 pci=off root=/dev/vda rw init=/sbin/init ip={}::10.10.0.1:255.255.0.0::eth0:off", vm.ip)
            },
            "drives": [{
                "drive_id": "rootfs",
                "path_on_host": format!("{image_path}/rootfs.ext4"),
                "is_root_device": true,
                "is_read_only": false
            }],
            "machine-config": {
                "vcpu_count": vm.cpu,
                "mem_size_mib": vm.mem,
            },
            "network-interfaces": [{
                "iface_id": "eth0",
                "host_dev_name": tap,
            }]
        });

        let path = Self::config_path(vm);
        let json = serde_json::to_string_pretty(&config).c(d!("serialize config"))?;
        std::fs::write(&path, json).c(d!("write config"))
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

        self.write_config(vm, image_path)?;

        let sock = Self::socket_path(vm);
        let config = Self::config_path(vm);

        let _ = std::fs::remove_file(&sock);
        let console = std::fs::File::create(format!("{RUN_DIR}/fc-{}.log", vm.id))
            .c(d!("create console log"))?;
        let mut child = Command::new("firecracker")
            .args(["--api-sock", &sock])
            .args(["--config-file", &config])
            .stdin(std::process::Stdio::null())
            .stdout(console)
            .spawn()
            .c(d!("spawn firecracker"))?;

        let pid = child.id();
        if let Err(e) = std::fs::write(Self::pid_path(vm), pid.to_string()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e).c(d!("write Firecracker PID"));
        }

        // Spawn a reaper thread so the child process is wait()ed on,
        // preventing zombie processes if the Firecracker VM exits.
        std::thread::spawn(move || {
            let _ = child.wait();
        });

        for _ in 0..100 {
            if !super::process_matches(pid, &sock)? {
                return Err(eg!("Firecracker exited during boot; check agent logs"));
            }
            if Path::new(&sock).exists() && matches!(self.state(vm), Ok(VmState::Running)) {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err(eg!("Firecracker did not become ready"))
    }

    fn start(&self, vm: &Vm) -> Result<()> {
        let sock = Self::socket_path(vm);
        if !Path::new(&sock).exists() {
            return Err(eg!("firecracker socket not found for VM {}", vm.id));
        }

        // Resume a paused VM via the Firecracker API
        let output = Command::new("curl")
            .args([
                "--unix-socket",
                &sock,
                "--fail",
                "--max-time",
                "5",
                "-s",
                "-X",
                "PATCH",
                "http://localhost/vm",
                "-H",
                "Content-Type: application/json",
                "-d",
                r#"{"state": "Resumed"}"#,
            ])
            .bounded_output()
            .c(d!("resume firecracker"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("firecracker resume failed: {}", stderr));
        }
        Ok(())
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        match Self::read_pid(vm) {
            Ok(pid) => super::terminate(pid, &Self::socket_path(vm))?,
            Err(e) if Path::new(&Self::pid_path(vm)).exists() => return Err(e),
            Err(_) => {}
        }
        Ok(())
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        self.stop(vm)?;
        for path in [
            Self::socket_path(vm),
            Self::pid_path(vm),
            Self::config_path(vm),
            format!("{RUN_DIR}/fc-{}.log", vm.id),
        ] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).c(d!("remove Firecracker runtime file")),
            }
        }
        Ok(())
    }

    fn state(&self, vm: &Vm) -> Result<VmState> {
        let pid = match Self::read_pid(vm) {
            Ok(pid) => pid,
            Err(e) if Path::new(&Self::pid_path(vm)).exists() => return Err(e),
            Err(_) => return Ok(VmState::Stopped),
        };
        let sock = Self::socket_path(vm);
        if !super::process_matches(pid, &sock)? {
            return Ok(VmState::Stopped);
        }
        let output = Command::new("curl")
            .args([
                "--unix-socket",
                &sock,
                "--fail",
                "--max-time",
                "5",
                "-s",
                "http://localhost/",
            ])
            .bounded_output()
            .c(d!("query Firecracker"))?;
        if !output.status.success() {
            return Err(eg!("cannot query Firecracker status"));
        }
        let body: serde_json::Value =
            serde_json::from_slice(&output.stdout).c(d!("Firecracker status"))?;
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
