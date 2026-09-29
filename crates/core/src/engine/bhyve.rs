//! Experimental FreeBSD bhyve with cold restart and verified process cleanup.
use super::VmEngine;
use crate::command::CommandExt;
use crate::model::{RUN_DIR, Vm, VmState};
use crate::net;
use ruc::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Default)]
pub struct BhyveEngine;
impl BhyveEngine {
    pub fn new() -> Self {
        Self
    }
    fn artifact(vm: &Vm, suffix: &str) -> PathBuf {
        Path::new(RUN_DIR).join(format!("bhyve-{}.{suffix}", vm.id))
    }
    fn device(vm: &Vm) -> PathBuf {
        Path::new("/dev/vmm").join(&vm.id)
    }
    fn pid(vm: &Vm) -> Result<Option<i32>> {
        // bhyve changes its process title. Never signal a PID solely from a stale file.
        let found = find_process(&vm.id, &process_listing()?)?;
        if found.is_some() && !Self::artifact(vm, "disk").is_file() {
            return Err(eg!("bhyve ownership metadata missing; retain resources"));
        }
        Ok(found)
    }
    fn remove_device(vm: &Vm) -> Result<()> {
        if Self::device(vm)
            .try_exists()
            .c(d!("inspect bhyve device"))?
        {
            if !Self::artifact(vm, "disk").is_file() {
                return Err(eg!("unowned bhyve device; retain resources"));
            }
            let out = Command::new("bhyvectl")
                .args(["--destroy", "--vm", &vm.id])
                .bounded_output()
                .c(d!("destroy bhyve device"))?;
            if !out.status.success() {
                return Err(eg!(
                    "bhyvectl destroy failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
        }
        Ok(())
    }
}
impl VmEngine for BhyveEngine {
    fn create(&self, vm: &Vm, image_path: &str, _: &str, _: &[String]) -> Result<()> {
        if Self::pid(vm)?.is_some() {
            return Err(eg!("bhyve is still running"));
        }
        Self::remove_device(vm)?;
        std::fs::create_dir_all(RUN_DIR).c(d!())?;
        let disk = Path::new(image_path)
            .canonicalize()
            .c(d!("resolve bhyve disk"))?;
        std::fs::write(
            Self::artifact(vm, "disk"),
            disk.as_os_str().as_encoded_bytes(),
        )
        .c(d!("record bhyve disk"))?;
        let out = Command::new("bhyveload")
            .args([
                "-m",
                &format!("{}M", vm.mem),
                "-e",
                "autoboot_delay=0",
                "-d",
            ])
            .arg(&disk)
            .arg(&vm.id)
            .stdin(Stdio::null())
            .bounded_output()
            .c(d!("run bhyveload"))?;
        if !out.status.success() {
            return Err(eg!(
                "bhyveload failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let log = std::fs::File::create(Self::artifact(vm, "log")).c(d!("create bhyve log"))?;
        let mut child = Command::new("bhyve")
            .args([
                "-A",
                "-H",
                "-P",
                "-c",
                &vm.cpu.to_string(),
                "-m",
                &format!("{}M", vm.mem),
            ])
            .args([
                "-s",
                "0:0,hostbridge",
                "-s",
                &format!("3:0,virtio-blk,{}", disk.display()),
            ])
            .args([
                "-s",
                &format!("4:0,virtio-net,{}", net::bhyve_tap_device(&vm.id)?),
                "-s",
                "31,lpc",
                "-l",
                "com1,stdio",
            ])
            .arg(&vm.id)
            .stdin(Stdio::null())
            .stdout(log.try_clone().c(d!())?)
            .stderr(log)
            .spawn()
            .c(d!("spawn bhyve"))?;
        // Keep a failed live launch discoverable; the agent retains the VM record.
        std::fs::write(Self::artifact(vm, "pid"), child.id().to_string())
            .c(d!("record bhyve PID"))?;
        for _ in 0..10 {
            if let Some(status) = child.try_wait().c(d!("check bhyve startup"))? {
                return Err(eg!(format!(
                    "bhyve exited during startup ({status}); inspect its runtime log"
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Reap a later exit while the agent remains alive.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    fn start(&self, _: &Vm) -> Result<()> {
        Err(eg!(
            "stopped bhyve VMs require cold creation from their retained disk"
        ))
    }
    fn stop(&self, vm: &Vm) -> Result<()> {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        for signal in [Signal::SIGTERM, Signal::SIGKILL] {
            let Some(pid) = Self::pid(vm)? else {
                break;
            };
            if signal == Signal::SIGKILL {
                eprintln!(
                    "[bhyve] {} did not stop after ACPI request; forcing termination",
                    vm.id
                );
            }
            match kill(Pid::from_raw(pid), signal) {
                Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
                Err(e) => return Err(eg!(e)),
            }
            for _ in 0..100 {
                if Self::pid(vm)?.is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        if Self::pid(vm)?.is_some() {
            return Err(eg!("bhyve process did not exit; retain disk"));
        }
        Self::remove_device(vm)
    }
    fn destroy(&self, vm: &Vm) -> Result<()> {
        self.stop(vm)?;
        for suffix in ["pid", "disk", "log"] {
            match std::fs::remove_file(Self::artifact(vm, suffix)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(eg!(e)),
            }
        }
        Ok(())
    }
    fn state(&self, vm: &Vm) -> Result<VmState> {
        Ok(if Self::pid(vm)?.is_some() {
            VmState::Running
        } else {
            VmState::Stopped
        })
    }
    fn name(&self) -> &'static str {
        "bhyve"
    }
}

fn process_listing() -> Result<String> {
    // FreeBSD treats commas after an empty column header as part of that header.
    // Separate -o arguments are required; -ww keeps long VM identities intact.
    let out = Command::new("ps")
        .args([
            "-axww", "-o", "pid=", "-o", "stat=", "-o", "comm=", "-o", "args=",
        ])
        .bounded_output()
        .c(d!("inspect bhyve processes"))?;
    if !out.status.success() {
        return Err(eg!("cannot query bhyve processes"));
    }
    String::from_utf8(out.stdout).c(d!("invalid process inventory encoding"))
}

fn find_process(id: &str, listing: &str) -> Result<Option<i32>> {
    if listing.trim().is_empty() {
        return Err(eg!("empty process inventory; retain resources"));
    }
    let mut found = None;
    for line in listing.lines().filter(|line| !line.trim().is_empty()) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 3 || fields[0].parse::<i32>().is_err() {
            return Err(eg!("invalid process inventory; retain resources"));
        }
        if fields[1].starts_with('Z') || fields[2] != "bhyve" {
            continue;
        }
        if fields.len() < 4 {
            return Err(eg!("unreadable bhyve process identity; retain resources"));
        }
        let args = &fields[3..];
        if (args.first() == Some(&"bhyve:") && args.get(1) == Some(&id))
            || (args.last() == Some(&id) && !args[0].ends_with(':'))
        {
            if found.is_some() {
                return Err(eg!("multiple bhyve processes match; retain resources"));
            }
            let pid = fields[0].parse::<i32>().c(d!("parse bhyve PID"))?;
            if pid <= 0 {
                return Err(eg!("invalid bhyve PID"));
            }
            found = Some(pid);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_ps_inventory_has_separate_columns() {
        let listing = process_listing().unwrap();
        let own_pid = std::process::id().to_string();
        assert!(
            listing
                .lines()
                .any(|line| line.split_whitespace().next() == Some(own_pid.as_str()))
        );
        assert!(find_process("ttstack-nonexistent-test-vm", &listing).is_ok());
        assert!(find_process("guest", ",stat=,comm=,args=\n123\n").is_err());
        assert!(find_process("guest", "").is_err());
    }

    #[test]
    fn bhyve_identity_uses_exact_live_process_titles_instead_of_stale_pids() {
        let listing = "10 S sleep sleep guest\n11 Z bhyve bhyve: guest (bhyve)\n12 S bhyve bhyve: guest-other (bhyve)\n13 ICJ bhyve bhyve: guest (bhyve)\n";
        assert_eq!(find_process("guest", listing).unwrap(), Some(13));
        assert_eq!(find_process("absent", listing).unwrap(), None);
        assert_eq!(
            find_process("guest", "14 S bhyve bhyve -c 1 guest").unwrap(),
            Some(14)
        );
        assert!(
            find_process(
                "guest",
                &(listing.to_string() + "15 S bhyve bhyve: guest (bhyve)\n")
            )
            .is_err()
        );
    }
}
