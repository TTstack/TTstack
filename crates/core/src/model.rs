//! Data models for TTstack.
//!
//! All persistent types are serde-serializable for use with SQLite storage
//! and JSON API communication.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

// ── Engine & Backend Enums ──────────────────────────────────────────

/// Supported hypervisor / container engines.
///
/// Platform availability:
/// - **Linux**: Qemu, Firecracker, Docker
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Qemu,
    Firecracker,
    Docker,
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Qemu => write!(f, "qemu"),
            Self::Firecracker => write!(f, "firecracker"),
            Self::Docker => write!(f, "docker"),
        }
    }
}

impl std::str::FromStr for Engine {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "qemu" | "kvm" => Ok(Self::Qemu),
            "firecracker" | "fc" => Ok(Self::Firecracker),
            "docker" | "podman" => Ok(Self::Docker),
            _ => Err(format!("unknown engine: {s}")),
        }
    }
}

/// Storage backend for guest disk images; Docker uses its own image store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Storage {
    /// Filesystem copies: QEMU qcow2 files or Firecracker kernel/rootfs directories.
    File,
    /// ZFS zvol — raw block devices backed by ZFS volumes.
    Zvol,
}

impl fmt::Display for Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File => write!(f, "file"),
            Self::Zvol => write!(f, "zvol"),
        }
    }
}

impl std::str::FromStr for Storage {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "file" => Ok(Self::File),
            "zvol" => Ok(Self::Zvol),
            _ => Err(format!("unknown storage backend: {s}")),
        }
    }
}

// ── State Enums ─────────────────────────────────────────────────────

/// Runtime state of a VM or container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmState {
    Running,
    Stopped,
    Paused,
    Creating,
    Failed,
    Deleting,
}

impl fmt::Display for VmState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Stopped => write!(f, "stopped"),
            Self::Paused => write!(f, "paused"),
            Self::Creating => write!(f, "creating"),
            Self::Failed => write!(f, "failed"),
            Self::Deleting => write!(f, "deleting"),
        }
    }
}

/// State of an environment (group of VMs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvState {
    Active,
    Stopped,
    Creating,
    Deleting,
    Failed,
}

/// Online status of a physical host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostState {
    Online,
    Offline,
}

// ── Resource Tracking ───────────────────────────────────────────────

/// Host scheduling capacities and reservations, not measured resource usage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Resource {
    pub cpu_total: u32,
    pub cpu_used: u32,
    /// Total memory in MiB.
    pub mem_total: u32,
    /// Reserved memory in MiB.
    pub mem_used: u32,
    /// Total disk in MiB.
    pub disk_total: u32,
    /// Reserved disk capacity in MiB, including stopped guests.
    pub disk_used: u32,
    /// Number of tracked VMs / containers, including stopped and failed records.
    pub vm_count: u32,
}

impl Resource {
    pub fn cpu_free(&self) -> u32 {
        self.cpu_total.saturating_sub(self.cpu_used)
    }

    pub fn mem_free(&self) -> u32 {
        self.mem_total.saturating_sub(self.mem_used)
    }

    pub fn disk_free(&self) -> u32 {
        self.disk_total.saturating_sub(self.disk_used)
    }

    /// Check whether the host can accommodate the given requirement.
    pub fn can_fit(&self, cpu: u32, mem: u32, disk: u32) -> bool {
        self.cpu_free() >= cpu && self.mem_free() >= mem && self.disk_free() >= disk
    }
}

// ── Core Entities ───────────────────────────────────────────────────

/// A physical host in the fleet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub id: String,
    /// Agent listen address, e.g. "10.0.0.1:9100".
    pub addr: String,
    pub resource: Resource,
    pub state: HostState,
    /// Engines available on this host.
    pub engines: Vec<Engine>,
    #[serde(default)]
    pub images: Vec<String>,
    /// Storage backend used on this host.
    pub storage: Storage,
    pub registered_at: u64,
}

/// A VM or container instance managed by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vm {
    pub id: String,
    pub env_id: String,
    pub host_id: String,
    pub image: String,
    pub engine: Engine,
    /// Number of vCPUs.
    pub cpu: u32,
    /// Memory in MiB.
    pub mem: u32,
    /// Disk in MiB.
    pub disk: u32,
    /// Internal IP (on the host bridge).
    pub ip: String,
    /// guest_port → host_port mapping.
    pub port_map: BTreeMap<u16, u16>,
    #[serde(default)]
    pub options: VmOptions,
    #[serde(default)]
    pub error: Option<String>,
    pub state: VmState,
    pub created_at: u64,
}

/// Creation options retained for idempotency, restart and firewall recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmOptions {
    pub ports: Vec<u16>,
    pub ssh_keys: Vec<String>,
    pub deny_outgoing: bool,
    pub requested_disk: u32,
    #[serde(default)]
    pub isolated_network: bool,
    /// Only a digest is exposed/persisted here; configuration contents stay on the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_config_digest: Option<String>,
}

/// An environment — a logical group of related VMs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Env {
    pub id: String,
    pub owner: String,
    pub vm_ids: Vec<String>,
    pub created_at: u64,
    /// Unix timestamp after which the env auto-expires (0 = never).
    pub expires_at: u64,
    #[serde(default)]
    pub error: Option<String>,
    pub state: EnvState,
}

// ── Default VM Sizing ───────────────────────────────────────────────

/// Default number of vCPUs per VM.
pub const VM_CPU_DEFAULT: u32 = 2;
/// Default memory per VM in MiB (1 GiB).
pub const VM_MEM_DEFAULT: u32 = 1024;
/// Default QEMU virtual disk size in MiB (40 GiB).
pub const VM_DISK_DEFAULT: u32 = 40 * 1024;
/// Default environment lifetime in seconds (6 hours); explicit zero means no expiry.
pub const DEFAULT_LIFETIME: u64 = 6 * 3600;
/// Maximum hosts in the fleet.
pub const MAX_HOSTS: usize = 50;
/// Maximum total VM instances across the fleet.
pub const MAX_VMS: usize = 1000;

/// Directory for engine PID files, sockets, and other runtime state.
pub const RUN_DIR: &str = "/home/ttstack/run";

// ── Input Validation ────────────────────────────────────────────────

/// Validate that a name (env, host, image) is safe.
///
/// Rejects path traversal (`..`), shell metacharacters, and excessive length.
pub fn validate_name(name: &str, label: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    if name.len() > 128 {
        return Err(format!("{label} too long (max 128 chars)"));
    }
    // Only allow alphanumeric, hyphen, underscore, and dot.
    // This prevents path traversal, shell injection, and filesystem issues.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(format!(
            "{label} contains invalid characters (only a-z, A-Z, 0-9, '-', '_', '.' allowed)"
        ));
    }
    if name.starts_with('.') || name.contains("..") {
        return Err(format!("{label} must not start with '.' or contain '..'"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Engine ──────────────────────────────────────────────────────

    #[test]
    fn engine_display_roundtrip() {
        for e in [Engine::Qemu, Engine::Firecracker, Engine::Docker] {
            let s = e.to_string();
            let parsed: Engine = s.parse().unwrap();
            assert_eq!(e, parsed);
        }
    }

    #[test]
    fn engine_aliases() {
        assert_eq!("kvm".parse::<Engine>().unwrap(), Engine::Qemu);
        assert_eq!("fc".parse::<Engine>().unwrap(), Engine::Firecracker);
        assert_eq!("podman".parse::<Engine>().unwrap(), Engine::Docker);
        assert_eq!("QEMU".parse::<Engine>().unwrap(), Engine::Qemu);
    }

    #[test]
    fn engine_unknown() {
        assert!("foobar".parse::<Engine>().is_err());
    }

    #[test]
    fn engine_serde_json() {
        for (engine, name) in [
            (Engine::Qemu, "qemu"),
            (Engine::Firecracker, "firecracker"),
            (Engine::Docker, "docker"),
        ] {
            let json = serde_json::to_string(&engine).unwrap();
            assert_eq!(json, format!("\"{name}\""));
            assert_eq!(serde_json::from_str::<Engine>(&json).unwrap(), engine);
        }
        // JSON requires canonical names; CLI aliases do not change persisted formats.
        for name in ["unknown-engine", "kvm", "fc", "podman", "QEMU", ""] {
            assert!(serde_json::from_value::<Engine>(serde_json::json!(name)).is_err());
        }
    }

    // ── Storage ─────────────────────────────────────────────────────

    #[test]
    fn storage_display_roundtrip() {
        for s in [Storage::File, Storage::Zvol] {
            let text = s.to_string();
            let parsed: Storage = text.parse().unwrap();
            assert_eq!(s, parsed);
        }
    }

    #[test]
    fn storage_unknown() {
        assert!("ntfs".parse::<Storage>().is_err());
    }

    // ── VmState ─────────────────────────────────────────────────────

    #[test]
    fn vmstate_display() {
        assert_eq!(VmState::Running.to_string(), "running");
        assert_eq!(VmState::Stopped.to_string(), "stopped");
        assert_eq!(VmState::Paused.to_string(), "paused");
        assert_eq!(VmState::Creating.to_string(), "creating");
        assert_eq!(VmState::Failed.to_string(), "failed");
    }

    // ── Resource ────────────────────────────────────────────────────

    #[test]
    fn resource_free_values() {
        let r = Resource {
            cpu_total: 16,
            cpu_used: 6,
            mem_total: 32768,
            mem_used: 8192,
            disk_total: 500_000,
            disk_used: 100_000,
            vm_count: 3,
        };
        assert_eq!(r.cpu_free(), 10);
        assert_eq!(r.mem_free(), 24576);
        assert_eq!(r.disk_free(), 400_000);
    }

    #[test]
    fn resource_free_saturates() {
        let r = Resource {
            cpu_total: 4,
            cpu_used: 10, // over-committed
            ..Default::default()
        };
        assert_eq!(r.cpu_free(), 0); // saturates, no panic
    }

    #[test]
    fn resource_can_fit() {
        let r = Resource {
            cpu_total: 8,
            cpu_used: 4,
            mem_total: 16384,
            mem_used: 8192,
            disk_total: 200_000,
            disk_used: 100_000,
            vm_count: 2,
        };
        assert!(r.can_fit(4, 8192, 100_000)); // exact fit
        assert!(r.can_fit(1, 1, 1)); // plenty of room
        assert!(!r.can_fit(5, 1, 1)); // cpu insufficient
        assert!(!r.can_fit(1, 9000, 1)); // mem insufficient
        assert!(!r.can_fit(1, 1, 200_000)); // disk insufficient
    }

    #[test]
    fn resource_default_is_zero() {
        let r = Resource::default();
        assert_eq!(r.cpu_total, 0);
        assert_eq!(r.vm_count, 0);
        assert!(!r.can_fit(1, 1, 1));
    }

    // ── Constants ───────────────────────────────────────────────────

    // ── Validation ──────────────────────────────────────────────────

    #[test]
    fn validate_name_ok() {
        assert!(validate_name("ubuntu-22.04", "image").is_ok());
        assert!(validate_name("my-env", "env").is_ok());
        assert!(validate_name("a", "x").is_ok());
    }

    #[test]
    fn validate_name_rejects_traversal() {
        assert!(validate_name("../etc/passwd", "image").is_err());
        assert!(validate_name("foo/../bar", "image").is_err());
        assert!(validate_name("foo/bar", "image").is_err());
    }

    #[test]
    fn validate_name_rejects_empty_and_long() {
        assert!(validate_name("", "env").is_err());
        let long = "a".repeat(200);
        assert!(validate_name(&long, "env").is_err());
    }

    #[test]
    fn validate_name_rejects_null() {
        assert!(validate_name("foo\0bar", "id").is_err());
    }

    #[test]
    fn validate_name_rejects_spaces_and_special() {
        assert!(validate_name("bad name", "env").is_err());
        assert!(validate_name("bad!", "env").is_err());
        assert!(validate_name("bad@name", "env").is_err());
        assert!(validate_name(".hidden", "env").is_err());
    }
}

/// Container references are not filesystem paths; allow registry, tag and digest syntax.
pub fn validate_image(image: &str, engine: Engine) -> std::result::Result<(), String> {
    if engine != Engine::Docker {
        return validate_name(image, "image");
    }
    if image.is_empty()
        || image.len() > 512
        || !image.as_bytes()[0].is_ascii_alphanumeric()
        || !image
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-/:@".contains(&c))
    {
        return Err("invalid container image reference".into());
    }
    Ok(())
}

pub fn validate_vm_options(
    engine: Engine,
    disk: Option<u32>,
    deny_outgoing: bool,
    ssh_keys: &[String],
    ports: &[u16],
) -> std::result::Result<(), String> {
    if disk.is_some() && !matches!(engine, Engine::Qemu | Engine::Firecracker) {
        return Err(
            "--disk is supported only by QEMU and Firecracker (Docker has no disk quota)".into(),
        );
    }
    if deny_outgoing && engine == Engine::Docker {
        return Err("--deny-outgoing is not supported by Docker".into());
    }
    if !ssh_keys.is_empty() && matches!(engine, Engine::Docker | Engine::Firecracker) {
        return Err(format!("SSH key injection is not supported by {engine}"));
    }
    if ports.contains(&0) {
        return Err("port must be between 1 and 65535".into());
    }
    for key in ssh_keys {
        if key.contains(['\n', '\r'])
            || !key
                .split_whitespace()
                .next()
                .is_some_and(|k| k.starts_with("ssh-") || k.starts_with("ecdsa-"))
            || key.split_whitespace().nth(1).is_none()
        {
            return Err("invalid SSH public key; provide one OpenSSH public key per entry".into());
        }
    }
    Ok(())
}

impl Engine {
    /// Reserve VMM headroom as well as guest RAM for jailed Firecracker.
    pub fn memory_reservation(self, guest_mib: u32) -> u32 {
        guest_mib.saturating_add(if self == Self::Firecracker { 128 } else { 0 })
    }
    pub fn default_disk(self) -> u32 {
        if self == Self::Qemu {
            VM_DISK_DEFAULT
        } else {
            0
        }
    }
}

impl Resource {
    /// Disk and instance slots remain reserved even when a VM is stopped.
    pub fn account(&mut self, vm: &Vm) {
        self.disk_used = self.disk_used.saturating_add(vm.disk);
        self.vm_count = self.vm_count.saturating_add(1);
        if vm.state != VmState::Stopped {
            self.cpu_used = self.cpu_used.saturating_add(vm.cpu);
            self.mem_used = self
                .mem_used
                .saturating_add(vm.engine.memory_reservation(vm.mem));
        }
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    #[test]
    fn image_names_respect_backend_format() {
        for name in [
            "ubuntu:24.04",
            "registry.example:5000/team/app:v1",
            "alpine@sha256:abcdef",
        ] {
            assert!(validate_image(name, Engine::Docker).is_ok());
            assert!(validate_image(name, Engine::Qemu).is_err());
        }
        assert!(validate_image("--privileged", Engine::Docker).is_err());
        assert!(validate_image("../image", Engine::Qemu).is_err());
    }
    #[test]
    fn unsupported_options_are_rejected() {
        assert!(validate_vm_options(Engine::Docker, None, true, &[], &[]).is_err());
        assert!(validate_vm_options(Engine::Firecracker, Some(512), false, &[], &[]).is_ok());
        assert!(validate_vm_options(Engine::Docker, Some(512), false, &[], &[]).is_err());
        assert!(
            validate_vm_options(
                Engine::Qemu,
                Some(512),
                false,
                &["/missing/key.pub".into()],
                &[22]
            )
            .is_err()
        );
        assert!(
            validate_vm_options(
                Engine::Qemu,
                Some(512),
                true,
                &["ssh-ed25519 AAAA user".into()],
                &[22]
            )
            .is_ok()
        );
    }
}
