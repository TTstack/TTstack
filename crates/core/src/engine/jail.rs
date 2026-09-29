//! Experimental FreeBSD shared-IP jails with retained roots and cold restart.
use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use ruc::*;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
pub struct JailEngine;

impl JailEngine {
    pub fn new() -> Self {
        Self
    }
    fn jail_name(vm: &Vm) -> String {
        format!("tt-{}", vm.id)
    }
    fn config_path(vm: &Vm) -> PathBuf {
        Path::new(RUN_DIR).join(format!("jail-{}.conf", vm.id))
    }
    fn root_path(vm: &Vm) -> PathBuf {
        Path::new(RUN_DIR).join(format!("jail-{}.root", vm.id))
    }

    fn query(vm: &Vm) -> Result<Option<String>> {
        // Enumerate successfully, including dying jails. A query failure is not absence.
        let out = Command::new("jls")
            .args(["--libxo", "json", "-dn", "name", "path"])
            .bounded_output()
            .c(d!("query jails"))?;
        if !out.status.success() {
            return Err(eg!("jls failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).c(d!("parse jls"))?;
        let jails = value["jail-information"]["jail"]
            .as_array()
            .ok_or_else(|| eg!("missing jail inventory"))?;
        let name = Self::jail_name(vm);
        for jail in jails {
            if jail["name"]
                .as_str()
                .is_some_and(|v| v.rsplit('.').next() == Some(&name))
            {
                let root = jail["path"]
                    .as_str()
                    .ok_or_else(|| eg!("missing jail path"))?;
                let expected =
                    std::fs::read_to_string(Self::root_path(vm)).c(d!("read owned jail root"))?;
                if root != expected {
                    return Err(eg!("jail name belongs to a different root"));
                }
                return Ok(Some(root.into()));
            }
        }
        Ok(None)
    }

    fn unmount_devfs(vm: &Vm) -> Result<()> {
        let root = match std::fs::read_to_string(Self::root_path(vm)) {
            Ok(root) => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(eg!(e)),
        };
        let dev = format!("{root}/dev");
        let out = Command::new("mount")
            .arg("-p")
            .bounded_output()
            .c(d!("list mounts"))?;
        if !out.status.success() {
            return Err(eg!("cannot inspect jail mounts"));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.get(1) == Some(&dev.as_str()) {
                if fields.get(2) != Some(&"devfs") {
                    return Err(eg!("unexpected filesystem on jail dev directory"));
                }
                let out = Command::new("umount")
                    .arg(&dev)
                    .bounded_output()
                    .c(d!("unmount jail devfs"))?;
                if !out.status.success() {
                    return Err(eg!(
                        "cannot unmount jail devfs: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ));
                }
            }
        }
        Ok(())
    }
}

impl VmEngine for JailEngine {
    fn create(&self, vm: &Vm, image_path: &str, _: &str, ssh_keys: &[String]) -> Result<()> {
        let root = Path::new(image_path)
            .canonicalize()
            .c(d!("canonicalize jail root"))?;
        let root_str = root.to_str().ok_or_else(|| eg!("invalid jail root path"))?;
        if root_str.chars().any(char::is_whitespace) {
            return Err(eg!("jail root path must not contain whitespace"));
        }
        if Self::query(vm)?.is_some() {
            return Err(eg!("jail already exists; reconcile before retrying"));
        }
        if !ssh_keys.is_empty() {
            use std::os::unix::fs::PermissionsExt;
            let ssh = root.join("root/.ssh");
            std::fs::create_dir_all(&ssh).c(d!("create jail .ssh"))?;
            std::fs::write(ssh.join("authorized_keys"), ssh_keys.join("\n") + "\n")
                .c(d!("write jail keys"))?;
            std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).c(d!())?;
            std::fs::set_permissions(
                ssh.join("authorized_keys"),
                std::fs::Permissions::from_mode(0o600),
            )
            .c(d!())?;
            // Root access remains public-key-only, including the standard FreeBSD base.
            let sshd = root.join("etc/ssh/sshd_config");
            let old = std::fs::read_to_string(&sshd).c(d!("read jail sshd config"))?;
            std::fs::write(sshd, format!("PermitRootLogin prohibit-password\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n{old}")).c(d!("configure jail sshd"))?;
        }
        std::fs::create_dir_all(RUN_DIR).c(d!())?;
        std::fs::write(Self::root_path(vm), root_str).c(d!("record jail root"))?;
        let config = format!(
            "{} {{\n path = {};\n host.hostname = \"ttstack\";\n ip4.addr = {};\n interface = \"tt0\";\n persist;\n mount.devfs;\n exec.start = \"/bin/sh /etc/rc\";\n exec.stop = \"/bin/sh /etc/rc.shutdown\";\n exec.timeout = 30;\n stop.timeout = 10;\n}}\n",
            Self::jail_name(vm),
            serde_json::to_string(root_str).c(d!())?,
            vm.ip
        );
        std::fs::write(Self::config_path(vm), config).c(d!("write jail configuration"))?;
        let out = Command::new("jail")
            .arg("-f")
            .arg(Self::config_path(vm))
            .args(["-c", &Self::jail_name(vm)])
            .bounded_output()
            .c(d!("create jail"))?;
        if !out.status.success() {
            return Err(eg!(
                "jail create failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        if Self::query(vm)?.is_none() {
            return Err(eg!("jail exited during startup"));
        }
        Ok(())
    }
    fn start(&self, _: &Vm) -> Result<()> {
        Err(eg!(
            "stopped jails require cold creation from their retained root"
        ))
    }
    fn stop(&self, vm: &Vm) -> Result<()> {
        if Self::query(vm)?.is_some() {
            let out = Command::new("jail")
                .arg("-f")
                .arg(Self::config_path(vm))
                .args(["-r", &Self::jail_name(vm)])
                .bounded_output()
                .c(d!("stop jail"))?;
            if !out.status.success() {
                return Err(eg!(
                    "jail stop failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            for _ in 0..100 {
                if Self::query(vm)?.is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if Self::query(vm)?.is_some() {
                return Err(eg!("jail is still running or dying"));
            }
        }
        Self::unmount_devfs(vm)
    }
    fn destroy(&self, vm: &Vm) -> Result<()> {
        self.stop(vm)?;
        for path in [Self::config_path(vm), Self::root_path(vm)] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(eg!(e)),
            }
        }
        Ok(())
    }
    fn state(&self, vm: &Vm) -> Result<VmState> {
        Ok(if Self::query(vm)?.is_some() {
            VmState::Running
        } else {
            VmState::Stopped
        })
    }
    fn name(&self) -> &'static str {
        "jail"
    }
}
