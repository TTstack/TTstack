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
/// Uses a stable 48-bit hash of the VM ID, including long IDs.
/// Result is always <= 15 chars (IFNAMSIZ).
pub fn tap_name(vm_id: &str) -> String {
    // Freeze the original SipHash-1-3 (zero keys, UTF-8 plus str's 0xff sentinel).
    // Changing the algorithm would orphan existing taps, jails and firewall tables.
    let mut bytes = vm_id.as_bytes().to_vec();
    bytes.push(0xff);
    let mut v = [
        0x736f6d6570736575u64,
        0x646f72616e646f6du64,
        0x6c7967656e657261u64,
        0x7465646279746573u64,
    ];
    fn round(v: &mut [u64; 4]) {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13);
        v[1] ^= v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16);
        v[3] ^= v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21);
        v[3] ^= v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17);
        v[1] ^= v[2];
        v[2] = v[2].rotate_left(32);
    }
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        let mut word = [0; 8];
        word.copy_from_slice(chunk);
        let m = u64::from_le_bytes(word);
        v[3] ^= m;
        round(&mut v);
        v[0] ^= m;
    }
    let mut last = (bytes.len() as u64) << 56;
    for (i, byte) in chunks.remainder().iter().enumerate() {
        last |= u64::from(*byte) << (8 * i);
    }
    v[3] ^= last;
    round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..3 {
        round(&mut v);
    }
    let hash = v[0] ^ v[1] ^ v[2] ^ v[3];
    format!("tt-{:012x}", hash & 0xFFFF_FFFF_FFFF)
}

// ═══════════════════════════════════════════════════════════════════
// Linux implementation
// ═══════════════════════════════════════════════════════════════════

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub fn setup_bridge() -> Result<()> {
        if !bridge_exists()? {
            run(&["ip", "link", "add", BRIDGE_NAME, "type", "bridge"])?;
        }
        let output = Command::new("ip")
            .args(["-j", "-d", "link", "show", "dev", BRIDGE_NAME])
            .bounded_output()
            .c(d!("inspect bridge"))?;
        let links: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).c(d!("bridge metadata"))?;
        if !output.status.success() || !links.iter().any(|v| v["linkinfo"]["info_kind"] == "bridge")
        {
            return Err(eg!("tt0 exists but is not a bridge"));
        }
        run(&["ip", "addr", "replace", BRIDGE_CIDR, "dev", BRIDGE_NAME])?;
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

    pub fn set_tap_owner(vm_id: &str, uid: u32) -> Result<()> {
        use nix::libc;
        use std::os::fd::AsRawFd;
        let tap = tap_name(vm_id);
        if !link_exists(&tap)? {
            return create_tap_owned(vm_id, Some(uid));
        }
        let tun = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/net/tun")
            .c(d!("open stopped TAP"))?;
        // SAFETY: zero is valid for ifreq; the bounded name is NUL-terminated.
        let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
        for (out, byte) in request.ifr_name.iter_mut().zip(tap.bytes()) {
            *out = byte as libc::c_char;
        }
        request.ifr_ifru.ifru_flags = (libc::IFF_TAP | libc::IFF_NO_PI) as libc::c_short;
        // ip tuntap add uses IFF_TUN_EXCL, so it cannot reopen a persistent TAP.
        // Only stopped VMs use this path; the kernel rejects a queue still in use.
        // SAFETY: fd and ifreq pointer remain valid for the ioctl, with Linux TAP flags.
        if unsafe { libc::ioctl(tun.as_raw_fd(), libc::TUNSETIFF, &mut request) } < 0 {
            return Err(std::io::Error::last_os_error()).c(d!("open persistent TAP queue"));
        }
        // SAFETY: TUNSETOWNER takes an integer uid; this does not delete the device.
        if unsafe { libc::ioctl(tun.as_raw_fd(), libc::TUNSETOWNER, uid) } < 0 {
            return Err(std::io::Error::last_os_error()).c(d!("set TAP owner"));
        }
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
        // One transaction; preserve per-VM DNAT and denylist elements across restarts.
        nft(&format!(
            r#"
add table ip {NFT_TABLE}
add chain ip {NFT_TABLE} prerouting {{ type nat hook prerouting priority -100; policy accept; }}
add chain ip {NFT_TABLE} postrouting {{ type nat hook postrouting priority 100; policy accept; }}
add set ip {NFT_TABLE} denylist {{ type ipv4_addr; }}
add chain ip {NFT_TABLE} forward {{ type filter hook forward priority 0; policy accept; }}
flush chain ip {NFT_TABLE} postrouting
flush chain ip {NFT_TABLE} forward
add rule ip {NFT_TABLE} forward ct direction reply accept
add rule ip {NFT_TABLE} forward ip saddr @denylist drop
add rule ip {NFT_TABLE} postrouting ip saddr 10.10.0.0/16 masquerade
"#
        ))
    }

    pub fn add_port_forward(host_port: u16, vm_ip_addr: &str, guest_port: u16) -> Result<()> {
        nft(&format!(
            "add rule ip {NFT_TABLE} prerouting tcp dport {host_port} dnat to {vm_ip_addr}:{guest_port}"
        ))
    }

    fn object_exists(kind: &str, name: &str) -> Result<bool> {
        let output = Command::new("nft")
            .args(["-j", "list", "ruleset"])
            .output_timeout(std::time::Duration::from_secs(10))
            .c(d!("list firewall objects"))?;
        if !output.status.success() {
            return Err(eg!("cannot inspect firewall objects"));
        }
        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).c(d!("firewall objects"))?;
        Ok(json["nftables"].as_array().is_some_and(|entries| {
            entries.iter().any(|e| {
                e[kind]["family"] == "ip"
                    && e[kind]["table"] == NFT_TABLE
                    && e[kind]["name"] == name
            })
        }))
    }

    pub fn remove_port_forwards(vm_ip_addr: &str) -> Result<()> {
        if !object_exists("chain", "prerouting")? {
            return Ok(());
        }
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
        if !object_exists("set", "denylist")? {
            return Ok(());
        }
        if is_denied(vm_ip_addr)? {
            nft(&format!(
                "delete element ip {NFT_TABLE} denylist {{ {vm_ip_addr} }}"
            ))?;
        }
        Ok(())
    }

    pub(super) fn link_exists(name: &str) -> Result<bool> {
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

#[cfg(target_os = "linux")]
pub fn tap_exists(vm_id: &str) -> Result<bool> {
    platform::link_exists(&tap_name(vm_id))
}

/// Called only before launching a stopped Firecracker, never during live recovery.
#[cfg(target_os = "linux")]
pub fn prepare_jailed_tap(vm_id: &str, uid: u32) -> Result<()> {
    // Reattach the unused persistent tap to change its owner without deleting it.
    platform::set_tap_owner(vm_id, uid)
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
    fn tap_names_preserve_pre_upgrade_host_objects() {
        for (id, expected) in [
            ("", "tt-6ea523c53def"),
            ("vm1", "tt-e9e453b4d5a5"),
            ("vm2", "tt-fc7c4d3fbec3"),
            ("83b7e95-test-vm", "tt-ab2cb9caf2f1"),
            ("long-id-abcdefghijklmnopqrstuvwxyz", "tt-75e5d7552606"),
        ] {
            assert_eq!(tap_name(id), expected);
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
