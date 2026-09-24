//! API request and response types shared between all components.

use crate::model::*;
use serde::{Deserialize, Serialize};

// ── Agent API (controller → agent) ─────────────────────────────────

/// Request to create a VM on an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVmReq {
    pub vm_id: String,
    pub env_id: String,
    pub image: String,
    pub engine: Engine,
    pub cpu: u32,
    pub mem: u32,
    pub disk: u32,
    pub ports: Vec<u16>,
    pub deny_outgoing: bool,
    /// Block guest access to peers, host services and private networks (Linux VMs).
    #[serde(default)]
    pub isolated_network: bool,
    /// UTF-8 files on a read-only configuration drive (Firecracker only).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub guest_config: crate::guest_config::GuestConfig,
    /// Root SSH public keys for QEMU cloud-init.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,
}

/// Response from agent after creating a VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVmResp {
    pub vm: Vm,
}

/// Information reported by an agent about itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub host_id: String,
    pub resource: Resource,
    pub engines: Vec<Engine>,
    pub storage: Storage,
    pub images: Vec<String>,
}

// ── Controller API (CLI → controller) ──────────────────────────────

/// Specification for a single VM to be created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSpec {
    pub image: String,
    #[serde(default = "default_engine")]
    pub engine: Engine,
    pub cpu: Option<u32>,
    pub mem: Option<u32>,
    pub disk: Option<u32>,
    #[serde(default)]
    pub ports: Vec<u16>,
    #[serde(default)]
    pub deny_outgoing: bool,
    #[serde(default)]
    pub isolated_network: bool,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub guest_config: crate::guest_config::GuestConfig,
    /// Per-VM SSH keys (merged with env-level keys).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,
}

fn default_engine() -> Engine {
    Engine::Qemu
}

/// Request to create an environment with one or more VMs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEnvReq {
    pub id: String,
    #[serde(default)]
    pub owner: String,
    pub vms: Vec<VmSpec>,
    /// Lifetime in seconds; `None` means use server default.
    pub lifetime: Option<u64>,
    /// SSH public keys applied to all VMs in this environment.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,
}

/// Full environment details returned to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvDetail {
    pub env: Env,
    pub vms: Vec<Vm>,
    /// Operation diagnostics, including VM failures; inspect states and errors too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Explicit host registration request from a controller client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterHostReq {
    pub addr: String,
}

/// File/zvol image reported by an online host; excludes Docker image stores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub name: String,
    pub host_id: String,
}

/// Global status of the fleet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetStatus {
    pub hosts: u32,
    pub hosts_online: u32,
    pub total_vms: u32,
    pub total_envs: u32,
    pub cpu_total: u32,
    pub cpu_used: u32,
    pub mem_total: u32,
    pub mem_used: u32,
    pub disk_total: u32,
    pub disk_used: u32,
}

// ── Generic API Wrapper ────────────────────────────────────────────

/// Standard API response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: Serialize",
    deserialize = "T: serde::de::DeserializeOwned"
))]
pub struct ApiResp<T> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T> ApiResp<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Convenience for responses with no payload.
pub type ApiRespEmpty = ApiResp<()>;

impl ApiRespEmpty {
    pub fn ok() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_resp_success_roundtrip() {
        let resp = ApiResp::success("hello".to_string());
        assert!(resp.ok);
        assert_eq!(resp.data.as_deref(), Some("hello"));
        assert!(resp.error.is_none());

        let json = serde_json::to_string(&resp).unwrap();
        let back: ApiResp<String> = serde_json::from_str(&json).unwrap();
        assert!(back.ok);
        assert_eq!(back.data, resp.data);
    }

    #[test]
    fn api_resp_error() {
        let resp = ApiResp::<String>::err("boom");
        assert!(!resp.ok);
        assert!(resp.data.is_none());
        assert_eq!(resp.error.as_deref(), Some("boom"));
    }

    #[test]
    fn api_resp_empty_ok() {
        let resp = ApiRespEmpty::ok();
        assert!(resp.ok);
        assert!(resp.data.is_none());
        assert!(resp.error.is_none());

        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""ok":true"#));
        assert!(!json.contains("data"));
        assert!(!json.contains("error"));
    }

    #[test]
    fn api_resp_skip_none_fields() {
        let resp = ApiResp::success(42u32);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("error"));

        let resp = ApiResp::<u32>::err("fail");
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("data"));
    }

    #[test]
    fn vm_spec_defaults() {
        let json = r#"{"image": "ubuntu"}"#;
        let spec: VmSpec = serde_json::from_str(json).unwrap();
        assert_eq!(spec.image, "ubuntu");
        assert_eq!(spec.engine, Engine::Qemu); // default
        assert!(spec.ports.is_empty());
        assert!(!spec.deny_outgoing);
    }
}
