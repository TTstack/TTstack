//! QEMU/KVM engine implementation.
//!
//! Launches VMs via `qemu-system-x86_64` with KVM acceleration.
//! Each VM gets its own tap device connected to the host bridge.

use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use ruc::*;
use std::path::Path;
use std::process::Command;

pub struct QemuEngine;

impl Default for QemuEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl QemuEngine {
    pub fn new() -> Self {
        Self
    }

    fn build_cmd(&self, vm: &Vm, disk_path: &str, disk_format: &str) -> Result<Command> {
        let tap = crate::net::tap_name(&vm.id);
        let mut cmd = Command::new("qemu-system-x86_64");
        cmd.args(["-enable-kvm", "-daemonize"])
            .args(["-name", &vm.id])
            .args(["-m", &format!("{}M", vm.mem)])
            .args(["-smp", &vm.cpu.to_string()])
            .args([
                "-drive",
                &format!("file={disk_path},format={disk_format},if=virtio"),
            ])
            .args([
                "-netdev",
                &format!("tap,id=net0,ifname={tap},script=no,downscript=no"),
            ])
            .args([
                "-device",
                &format!(
                    "virtio-net-pci,netdev=net0,mac={}",
                    Self::mac_address(&vm.ip)?
                ),
            ])
            .args(["-pidfile", &self.pid_path(vm)])
            .args([
                "-monitor",
                &format!("unix:{},server,nowait", self.monitor_path(vm)),
            ])
            .args(["-vnc", "none"]);

        // Attach cloud-init seed ISO if it exists (for cloud images)
        let seed = self.seed_path(vm);
        if Path::new(&seed).exists() {
            cmd.args([
                "-drive",
                &format!("file={seed},format=raw,if=virtio,readonly=on"),
            ]);
        }

        Ok(cmd)
    }

    // The allocated IPv4 address is stable and unique on this host's bridge.
    // Use a locally administered unicast MAC instead of QEMU's shared default.
    fn mac_address(ip: &str) -> Result<String> {
        let [a, b, c, d] = ip
            .parse::<std::net::Ipv4Addr>()
            .c(d!("invalid guest IP"))?
            .octets();
        Ok(format!("02:54:{a:02x}:{b:02x}:{c:02x}:{d:02x}"))
    }

    fn network_config(vm: &Vm) -> Result<String> {
        Ok(format!(
            r#"version: 1
config:
  - type: physical
    name: eth0
    mac_address: "{mac}"
    subnets:
      - type: static
        address: {ip}/16
        gateway: 10.10.0.1
        dns_nameservers: [8.8.8.8, 1.1.1.1]
"#,
            mac = Self::mac_address(&vm.ip)?,
            ip = vm.ip,
        ))
    }

    /// Generate a cloud-init NoCloud seed ISO for the VM.
    ///
    /// This allows cloud images (Alpine, Debian, Ubuntu) to auto-configure
    /// on first boot: configure networking, inject SSH keys, enable sshd.
    fn generate_seed_iso(&self, vm: &Vm, ssh_keys: &[String]) -> Result<()> {
        let seed_dir = format!("{RUN_DIR}/seed-{}", vm.id);
        std::fs::create_dir_all(&seed_dir).c(d!("create seed dir"))?;

        // meta-data
        let meta_data = format!("instance-id: {}\nlocal-hostname: {}\n", vm.id, vm.id);
        std::fs::write(format!("{seed_dir}/meta-data"), meta_data).c(d!("write meta-data"))?;

        // V1 works with both Alpine's ENI renderer and netplan-based cloud images.
        // Match the explicit NIC MAC; driver matching only works with networkd.
        let network_config = Self::network_config(vm)?;
        std::fs::write(format!("{seed_dir}/network-config"), network_config)
            .c(d!("write network-config"))?;

        // user-data — inject SSH keys, disable password login
        let mut user_data = String::from(
            "#cloud-config\n\
             disable_root: false\n\
             ssh_pwauth: false\n",
        );

        if !ssh_keys.is_empty() {
            // Alpine's non-PAM sshd rejects locked accounts even with a valid key.
            // An impossible password hash permits key login without a usable password.
            user_data.push_str(
                "users:\n  - name: root\n    lock_passwd: false\n    hashed_passwd: '*'\n    ssh_authorized_keys:\n",
            );
            for key in ssh_keys {
                let quoted = serde_json::to_string(key).c(d!("quote SSH key"))?;
                user_data.push_str(&format!("      - {quoted}\n"));
            }
        }

        user_data.push_str(
            "runcmd:\n  \
             - sed -i 's/^#*PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config\n  \
             - sed -i 's/^#*PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config\n  \
             - systemctl restart sshd 2>/dev/null || service sshd restart 2>/dev/null || rc-service sshd restart 2>/dev/null || true\n",
        );

        std::fs::write(format!("{seed_dir}/user-data"), user_data).c(d!("write user-data"))?;

        // Generate ISO using genisoimage or mkisofs
        let seed_iso = self.seed_path(vm);
        let meta = format!("{seed_dir}/meta-data");
        let user = format!("{seed_dir}/user-data");
        let netcfg = format!("{seed_dir}/network-config");

        let output = if Path::new("/usr/bin/genisoimage").exists() {
            Command::new("genisoimage")
                .args([
                    "-output", &seed_iso, "-volid", "cidata", "-joliet", "-rock", "-quiet",
                ])
                .args([&meta, &user, &netcfg])
                .bounded_output()
                .c(d!("generate seed ISO"))?
        } else {
            Command::new("mkisofs")
                .args(["-o", &seed_iso, "-V", "cidata", "-J", "-R", "-quiet"])
                .args([&meta, &user, &netcfg])
                .bounded_output()
                .c(d!("generate seed ISO"))?
        };

        // Clean up temp dir
        let _ = std::fs::remove_dir_all(&seed_dir);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("seed ISO creation failed: {}", stderr));
        }

        Ok(())
    }

    fn pid_path(&self, vm: &Vm) -> String {
        format!("{RUN_DIR}/qemu-{}.pid", vm.id)
    }

    fn monitor_path(&self, vm: &Vm) -> String {
        format!("{RUN_DIR}/qemu-{}.sock", vm.id)
    }

    fn seed_path(&self, vm: &Vm) -> String {
        format!("{RUN_DIR}/seed-{}.iso", vm.id)
    }

    fn existing_pid(&self, vm: &Vm) -> Result<Option<u32>> {
        match std::fs::read_to_string(self.pid_path(vm)) {
            Ok(s) => Ok(Some(s.trim().parse::<u32>().c(d!("invalid QEMU PID"))?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).c(d!("read QEMU PID")),
        }
    }

    fn monitor(&self, vm: &Vm, command: &str) -> Result<String> {
        use std::io::{Read, Write};
        let mut stream =
            std::os::unix::net::UnixStream::connect(self.monitor_path(vm)).c(d!("QEMU monitor"))?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .c(d!())?;
        stream
            .set_write_timeout(Some(std::time::Duration::from_secs(2)))
            .c(d!())?;
        let read_prompt = |stream: &mut std::os::unix::net::UnixStream| -> Result<String> {
            let mut reply = String::new();
            let mut buf = [0; 1024];
            while reply.len() < 65536 {
                let n = stream.read(&mut buf).c(d!("read QEMU monitor"))?;
                if n == 0 {
                    return Err(eg!("QEMU monitor closed"));
                }
                reply.push_str(&String::from_utf8_lossy(&buf[..n]));
                if reply.contains("(qemu)") {
                    return Ok(reply);
                }
            }
            Err(eg!("QEMU monitor response too large"))
        };
        read_prompt(&mut stream)?;
        writeln!(stream, "{command}").c(d!("write QEMU monitor"))?;
        read_prompt(&mut stream)
    }
}

impl VmEngine for QemuEngine {
    fn create(
        &self,
        vm: &Vm,
        image_path: &str,
        disk_format: &str,
        ssh_keys: &[String],
    ) -> Result<()> {
        std::fs::create_dir_all(RUN_DIR).c(d!("create runtime dir"))?;

        // A broken seed means the guest may be unreachable: fail before booting.
        self.generate_seed_iso(vm, ssh_keys)?;
        let _ = std::fs::remove_file(self.monitor_path(vm));
        let _ = std::fs::remove_file(self.pid_path(vm));

        let output = self
            .build_cmd(vm, image_path, disk_format)?
            .bounded_output()
            .c(d!("spawn qemu"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("qemu launch failed: {}", stderr));
        }

        Ok(())
    }

    fn start(&self, vm: &Vm) -> Result<()> {
        self.monitor(vm, "cont")?;
        Ok(())
    }

    fn stop(&self, vm: &Vm) -> Result<()> {
        if let Some(pid) = self.existing_pid(vm)? {
            // Give a cooperative guest a chance to shut down before terminating the VMM.
            let _ = self.monitor(vm, "system_powerdown");
            for _ in 0..100 {
                if !super::process_matches(pid, &vm.id)? {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            super::terminate(pid, &vm.id)?;
        }
        Ok(())
    }

    fn destroy(&self, vm: &Vm) -> Result<()> {
        if let Some(pid) = self.existing_pid(vm)? {
            super::terminate(pid, &vm.id)?;
        }
        for path in [self.pid_path(vm), self.monitor_path(vm), self.seed_path(vm)] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).c(d!("remove QEMU runtime file")),
            }
        }
        Ok(())
    }

    fn state(&self, vm: &Vm) -> Result<VmState> {
        let Some(pid) = self.existing_pid(vm)? else {
            return Ok(VmState::Stopped);
        };
        if !super::process_matches(pid, &vm.id)? {
            return Ok(VmState::Stopped);
        }
        let status = self.monitor(vm, "info status")?;
        if status.contains("paused") {
            Ok(VmState::Paused)
        } else if status.contains("running") {
            Ok(VmState::Running)
        } else {
            Err(eg!(format!("unexpected QEMU status: {status}")))
        }
    }

    fn name(&self) -> &'static str {
        "qemu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_macs_are_unique_across_the_address_pool() {
        let mut macs = std::collections::HashSet::new();
        for index in 0..65000 {
            let ip = crate::net::vm_ip(index);
            let mac = QemuEngine::mac_address(&ip).unwrap();
            assert!(mac.starts_with("02:")); // locally administered, unicast
            assert!(macs.insert(mac), "duplicate MAC for {ip}");
        }
        // Stable across restarts and toolchain upgrades; reject invalid persisted addresses.
        assert_eq!(
            QemuEngine::mac_address("10.10.0.2").unwrap(),
            "02:54:0a:0a:00:02"
        );
        assert!(QemuEngine::mac_address("not-an-ip").is_err());
    }

    #[test]
    fn build_cmd_uses_disk_format() {
        // Smoke test: ensure disk_format ends up in the -drive arg
        let eng = QemuEngine::new();
        let vm = Vm {
            id: "test-vm".into(),
            env_id: "e1".into(),
            host_id: "h1".into(),
            image: "img".into(),
            engine: crate::model::Engine::Qemu,
            cpu: 2,
            mem: 1024,
            disk: 10240,
            ip: "10.10.0.2".into(),
            port_map: Default::default(),
            options: crate::model::VmOptions::default(),
            error: None,
            state: VmState::Creating,
            created_at: 0,
        };

        let cmd = eng.build_cmd(&vm, "/dev/zvol/tank/clone-1", "raw").unwrap();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let drive_arg = args.iter().find(|a| a.starts_with("file=")).unwrap();
        assert!(drive_arg.contains("format=raw"));
        assert!(drive_arg.contains("/dev/zvol/tank/clone-1"));

        let cmd2 = eng.build_cmd(&vm, "/tmp/disk.qcow2", "qcow2").unwrap();
        let args2: Vec<_> = cmd2
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let drive_arg2 = args2.iter().find(|a| a.starts_with("file=")).unwrap();
        assert!(drive_arg2.contains("format=qcow2"));
        let nic = args2
            .iter()
            .find(|a| a.starts_with("virtio-net-pci,"))
            .unwrap();
        let mac = nic.split("mac=").nth(1).unwrap();
        assert!(
            QemuEngine::network_config(&vm)
                .unwrap()
                .contains(&format!("mac_address: \"{mac}\""))
        );
    }
}
