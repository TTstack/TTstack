//! Network utilities for TTstack.
//!
//! Manages the virtual network infrastructure on each host:
//! - A bridge device for VM connectivity
//! - TAP devices for individual VMs
//! - Firewall NAT rules for port forwarding
//!
//! **Linux**: uses `ip`, `nftables`

use crate::command::CommandExt;
#[cfg(target_os = "linux")]
mod isolation;
#[cfg(target_os = "linux")]
pub use isolation::{isolate, remove_isolation};
#[cfg(target_os = "linux")]
use ruc::*;
#[cfg(target_os = "linux")]
use std::process::Command;

/// Default bridge name on each host.
pub const BRIDGE_NAME: &str = "tt0";
/// Bridge IP address (gateway for VMs).
pub const BRIDGE_ADDR: &str = "10.10.0.1";
/// Bridge subnet mask.
pub const BRIDGE_CIDR: &str = "10.10.0.1/16";
/// nftables table name (Linux).
#[cfg(target_os = "linux")]
pub const NFT_TABLE: &str = "tt-nat";

/// Derive an IP address for a VM from a sequential index (0..65534).
///
/// Produces addresses in the 10.10.x.y range, skipping .0 and .255.
pub fn vm_ip(index: u32) -> String {
    let index = index + 1; // skip .0.0
    let hi = (index / 254) & 0xFF;
    let lo = (index % 254) + 1;
    format!("10.10.{hi}.{lo}")
}

/// TAP device name for a VM.
///
/// Uses a hash of the VM ID to guarantee uniqueness even for long IDs.
/// Result is always <= 15 chars (IFNAMSIZ).
pub fn tap_name(vm_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    vm_id.hash(&mut h);
    let hash = h.finish();
    // "tt-" + 12 hex chars = 15 chars exactly
    format!("tt-{:012x}", hash & 0xFFFF_FFFF_FFFF)
}

// ═══════════════════════════════════════════════════════════════════
// Linux implementation
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub fn setup_bridge() -> Result<()> {
        if bridge_exists()? {
            std::fs::write("/proc/sys/net/ipv4/ip_forward", "1").c(d!("enable ip_forward"))?;
            return Ok(());
        }

        run(&["ip", "link", "add", BRIDGE_NAME, "type", "bridge"])?;
        run(&["ip", "addr", "add", BRIDGE_CIDR, "dev", BRIDGE_NAME])?;
        run(&["ip", "link", "set", BRIDGE_NAME, "up"])?;

        // Enable IP forwarding
        std::fs::write("/proc/sys/net/ipv4/ip_forward", "1").c(d!("enable ip_forward"))?;

        Ok(())
    }

    pub fn bridge_exists() -> Result<bool> {
        let output = Command::new("ip")
            .args(["link", "show", BRIDGE_NAME])
            .bounded_output()
            .c(d!())?;
        Ok(output.status.success())
    }

    pub fn create_tap(vm_id: &str) -> Result<()> {
        create_tap_owned(vm_id, None)
    }

    pub fn create_tap_owned(vm_id: &str, uid: Option<u32>) -> Result<()> {
        let tap = tap_name(vm_id);

        if !link_exists(&tap)? {
            let owner = uid.unwrap_or(0).to_string();
            run(&[
                "ip", "tuntap", "add", "dev", &tap, "mode", "tap", "user", &owner,
            ])?;
        }
        run(&["ip", "link", "set", &tap, "master", BRIDGE_NAME])?;
        run(&["ip", "link", "set", &tap, "up"])?;

        Ok(())
    }

    pub fn destroy_tap(vm_id: &str) -> Result<()> {
        let tap = tap_name(vm_id);
        if link_exists(&tap)? {
            run(&["ip", "link", "del", &tap])?;
        }
        Ok(())
    }

    pub fn setup_nat() -> Result<()> {
        nft(&format!("add table ip {NFT_TABLE}"))?;

        nft(&format!(
            "add chain ip {NFT_TABLE} prerouting {{ type nat hook prerouting priority -100; policy accept; }}"
        ))?;

        nft(&format!(
            "add chain ip {NFT_TABLE} postrouting {{ type nat hook postrouting priority 100; policy accept; }}"
        ))?;

        // Flush both chains on startup to avoid duplicate/stale rules.
        // Per-VM port forwards in prerouting will be restored from the
        // database by the agent's recovery loop.
        nft(&format!("flush chain ip {NFT_TABLE} postrouting"))?;
        nft(&format!("flush chain ip {NFT_TABLE} prerouting"))?;
        nft(&format!(
            "add set ip {NFT_TABLE} denylist {{ type ipv4_addr; }}"
        ))?;
        nft(&format!(
            "add chain ip {NFT_TABLE} forward {{ type filter hook forward priority 0; policy accept; }}"
        ))?;
        nft(&format!("flush chain ip {NFT_TABLE} forward"))?;
        nft(&format!(
            "add rule ip {NFT_TABLE} forward ct direction reply accept"
        ))?;
        nft(&format!(
            "add rule ip {NFT_TABLE} forward ip saddr @denylist drop"
        ))?;

        nft(&format!(
            "add rule ip {NFT_TABLE} postrouting ip saddr 10.10.0.0/16 masquerade"
        ))?;

        Ok(())
    }

    pub fn add_port_forward(host_port: u16, vm_ip_addr: &str, guest_port: u16) -> Result<()> {
        nft(&format!(
            "add rule ip {NFT_TABLE} prerouting tcp dport {host_port} dnat to {vm_ip_addr}:{guest_port}"
        ))
    }

    pub fn remove_port_forwards(vm_ip_addr: &str) -> Result<()> {
        let output = Command::new("nft")
            .args(["-a", "list", "chain", "ip", NFT_TABLE, "prerouting"])
            .bounded_output()
            .c(d!())?;

        if !output.status.success() {
            return Err(eg!(
                "cannot list NAT rules: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let listing = String::from_utf8_lossy(&output.stdout);
        for line in listing.lines() {
            if line
                .split_whitespace()
                .any(|part| part.split(':').next() == Some(vm_ip_addr))
                && let Some(handle) = line
                    .rsplit("handle ")
                    .next()
                    .and_then(|h| h.trim().parse::<u64>().ok())
            {
                nft(&format!(
                    "delete rule ip {NFT_TABLE} prerouting handle {handle}"
                ))?;
            }
        }

        Ok(())
    }

    fn is_denied(vm_ip_addr: &str) -> Result<bool> {
        let output = Command::new("nft")
            .args(["list", "set", "ip", NFT_TABLE, "denylist"])
            .output_timeout(std::time::Duration::from_secs(10))
            .c(d!("list denylist"))?;
        if !output.status.success() {
            return Err(eg!("cannot read outgoing firewall rules"));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .split(|c: char| c.is_whitespace() || ",{}".contains(c))
            .any(|s| s == vm_ip_addr))
    }
    pub fn deny_outgoing(vm_ip_addr: &str) -> Result<()> {
        if !is_denied(vm_ip_addr)? {
            nft(&format!(
                "add element ip {NFT_TABLE} denylist {{ {vm_ip_addr} }}"
            ))?;
        }
        Ok(())
    }
    pub fn allow_outgoing(vm_ip_addr: &str) -> Result<()> {
        if is_denied(vm_ip_addr)? {
            nft(&format!(
                "delete element ip {NFT_TABLE} denylist {{ {vm_ip_addr} }}"
            ))?;
        }
        Ok(())
    }

    fn link_exists(name: &str) -> Result<bool> {
        let output = Command::new("ip")
            .args(["-j", "link", "show"])
            .output_timeout(std::time::Duration::from_secs(10))
            .c(d!("list links"))?;
        if !output.status.success() {
            return Err(eg!("cannot list network interfaces"));
        }
        let links: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).c(d!("parse interfaces"))?;
        Ok(links
            .iter()
            .any(|link| link["ifname"].as_str() == Some(name)))
    }

    pub(super) fn nft(rule: &str) -> Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut input = tempfile::tempfile().c(d!("nft input"))?;
        writeln!(input, "{rule}").c(d!("write nft input"))?;
        input.seek(SeekFrom::Start(0)).c(d!())?;
        let output = Command::new("nft")
            .args(["-f", "-"])
            .stdin(input)
            .output_timeout(std::time::Duration::from_secs(10))
            .c(d!("nft"))?;
        if !output.status.success() {
            return Err(eg!(
                "nft {}: {}",
                rule,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn run(args: &[&str]) -> Result<()> {
        let output = Command::new(args[0])
            .args(&args[1..])
            .bounded_output()
            .c(d!(args.join(" ")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("{}: {}", args.join(" "), stderr));
        }

        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════
// Public re-exports (dispatches to platform module)
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
pub fn setup_bridge() -> Result<()> {
    platform::setup_bridge()
}

#[cfg(target_os = "linux")]
pub fn setup_nat() -> Result<()> {
    platform::setup_nat()
}

#[cfg(target_os = "linux")]
pub fn create_tap(vm_id: &str, _vm_ip_addr: &str) -> Result<()> {
    platform::create_tap(vm_id)
}

#[cfg(target_os = "linux")]
pub fn destroy_tap(vm_id: &str) -> Result<()> {
    platform::destroy_tap(vm_id)
}

/// Called only before launching a stopped Firecracker, never during live recovery.
#[cfg(target_os = "linux")]
pub fn prepare_jailed_tap(vm_id: &str, uid: u32) -> Result<()> {
    platform::destroy_tap(vm_id)?;
    platform::create_tap_owned(vm_id, Some(uid))
}

#[cfg(target_os = "linux")]
pub fn add_port_forward(host_port: u16, vm_ip_addr: &str, guest_port: u16) -> Result<()> {
    platform::add_port_forward(host_port, vm_ip_addr, guest_port)
}

#[cfg(target_os = "linux")]
pub fn remove_port_forwards(vm_ip_addr: &str) -> Result<()> {
    platform::remove_port_forwards(vm_ip_addr)
}

#[cfg(target_os = "linux")]
pub fn deny_outgoing(vm_ip_addr: &str) -> Result<()> {
    platform::deny_outgoing(vm_ip_addr)
}

#[cfg(target_os = "linux")]
pub fn allow_outgoing(vm_ip_addr: &str) -> Result<()> {
    platform::allow_outgoing(vm_ip_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_ip_first() {
        // index=0 → internal=1 → hi=0, lo=2 → 10.10.0.2
        assert_eq!(vm_ip(0), "10.10.0.2");
    }

    #[test]
    fn vm_ip_sequential() {
        assert_eq!(vm_ip(1), "10.10.0.3");
        // index=252 → internal=253 → hi=0, lo=254 → 10.10.0.254
        assert_eq!(vm_ip(252), "10.10.0.254");
    }

    #[test]
    fn vm_ip_wraps_to_next_octet() {
        // index=253 → internal=254 → hi=1, lo=254%254+1=1 → 10.10.1.1
        assert_eq!(vm_ip(253), "10.10.1.1");
    }

    #[test]
    fn vm_ip_unique_and_valid() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for i in 0..1000 {
            let ip = vm_ip(i);
            assert!(seen.insert(ip.clone()), "duplicate IP at index {i}: {ip}");
            // Verify no .0 or .255 in last octet
            let lo: u32 = ip.rsplit('.').next().unwrap().parse().unwrap();
            assert!(
                (1..=254).contains(&lo),
                "invalid lo octet {lo} at index {i}"
            );
        }
    }

    #[test]
    fn tap_name_fits_ifnamsiz() {
        assert!(tap_name("abc").len() <= 15);
        assert!(tap_name("a".repeat(200).as_str()).len() <= 15);
    }

    #[test]
    fn tap_name_deterministic() {
        assert_eq!(tap_name("vm1"), tap_name("vm1"));
    }

    #[test]
    fn tap_name_unique_for_different_ids() {
        assert_ne!(tap_name("vm1"), tap_name("vm2"));
        // Long IDs that used to collide via truncation are now unique
        assert_ne!(
            tap_name("very_long_vm_name_1"),
            tap_name("very_long_vm_name_2")
        );
    }
}
