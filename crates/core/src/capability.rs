//! Technical capability contracts, independent of the controller's build target.
//!
//! Design support, host prerequisites and per-instance eligibility are distinct.
//! Legacy strings remain a wire compatibility surface; canonical tags are scoped
//! to an engine/storage pair in a versioned report, never unioned across engines.
use crate::model::{Engine, Host, Storage};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const ENGINES: [Engine; 5] = [
    Engine::Qemu,
    Engine::Firecracker,
    Engine::Docker,
    Engine::Bhyve,
    Engine::Jail,
];
pub const STORAGE: [Storage; 2] = [Storage::File, Storage::Zvol];

pub mod legacy {
    pub const SSH: &str = "ssh_bootstrap";
    pub const CONFIG: &str = "guest_config";
    pub const ISOLATION: &str = "isolated_network";
    pub const BHYVE_EGRESS: &str = "bhyve_deny_outgoing";
    pub const QEMU_RESOURCES: &str = "qemu_resources";
    pub const FC_RESOURCES: &str = "firecracker_resources";
    pub const FC_DISK: &str = "firecracker_disk_resize";
    pub const FC_JAILER: &str = "firecracker_jailer";
    pub const FC_ZVOL: &str = "firecracker_zvol";
    pub const BACKUP: &str = "disk_backup_v1";
    pub const BACKUP_ZVOL: &str = "disk_backup_zvol_v1";
    pub const BACKUP_REFLINK: &str = "disk_backup_reflink_v1";
    pub const ALL: [&str; 12] = [
        SSH,
        CONFIG,
        ISOLATION,
        BHYVE_EGRESS,
        QEMU_RESOURCES,
        FC_RESOURCES,
        FC_DISK,
        FC_JAILER,
        FC_ZVOL,
        BACKUP,
        BACKUP_ZVOL,
        BACKUP_REFLINK,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    Run,
    Ports,
    SshBootstrap,
    RootKeys,
    GuestConfig,
    DenyOutgoing,
    Isolation,
    DiskSize,
    Resources,
    Backup,
}
impl Feature {
    pub const ALL: [Self; 10] = [
        Self::Run,
        Self::Ports,
        Self::SshBootstrap,
        Self::RootKeys,
        Self::GuestConfig,
        Self::DenyOutgoing,
        Self::Isolation,
        Self::DiskSize,
        Self::Resources,
        Self::Backup,
    ];
    pub fn tag(self) -> &'static str {
        match self {
            Self::Run => "vm.run",
            Self::Ports => "net.tcp.publish",
            Self::SshBootstrap => "guest.ssh.bootstrap",
            Self::RootKeys => "guest.ssh.root_keys",
            Self::GuestConfig => "guest.config.drive",
            Self::DenyOutgoing => "net.egress.deny",
            Self::Isolation => "net.isolation",
            Self::DiskSize => "disk.size.create",
            Self::Resources => "vm.resources.offline",
            Self::Backup => "disk.backup.point",
        }
    }
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Run => "Create and cold-start a workload with this engine and storage",
            Self::Ports => "Publish TCP guest ports on the agent host",
            Self::SshBootstrap => "Initialize an SSH account and report its access metadata",
            Self::RootKeys => "Inject root authorized keys without managed SSH bootstrap",
            Self::GuestConfig => "Deliver caller-supplied opaque files on a read-only guest drive",
            Self::DenyOutgoing => "Block new routed guest egress while preserving incoming replies",
            Self::Isolation => "Enforce guest isolation from host, peers and private networks",
            Self::DiskSize => "Request virtual root-disk capacity at creation",
            Self::Resources => "Update CPU, memory and root-disk capacity while stopped",
            Self::Backup => "Create a stopped root-disk recovery point using a qualified primitive",
        }
    }
    pub fn supported(self, engine: Engine, storage: Storage) -> bool {
        if engine == Engine::Jail && storage == Storage::Zvol {
            return false;
        }
        match self {
            Self::Run | Self::Ports => true,
            Self::RootKeys => engine == Engine::Jail,
            Self::GuestConfig => engine == Engine::Firecracker,
            Self::DenyOutgoing => {
                matches!(engine, Engine::Qemu | Engine::Firecracker | Engine::Bhyve)
            }
            Self::Backup => {
                matches!(engine, Engine::Qemu | Engine::Firecracker)
                    || (engine == Engine::Bhyve && storage == Storage::Zvol)
            }
            Self::SshBootstrap | Self::Isolation | Self::DiskSize | Self::Resources => {
                matches!(engine, Engine::Qemu | Engine::Firecracker)
            }
        }
    }
    pub fn limit(self, engine: Engine, storage: Storage) -> &'static str {
        match self {
            Self::SshBootstrap => {
                "Prepared guest image required; process state does not establish SSH readiness"
            }
            Self::RootKeys => {
                "Root public keys only; no custom account, sudo or managed SSH metadata"
            }
            Self::GuestConfig => {
                "Firecracker only; 32 files / 64 KiB, including the managed SSH seed"
            }
            Self::DenyOutgoing => {
                "Not host or peer isolation; Jail and Docker do not implement this contract"
            }
            Self::Isolation => "Linux QEMU/Firecracker only; independent of outgoing restrictions",
            Self::DiskSize | Self::Resources if engine == Engine::Qemu => {
                "No shrinking; guest partitions and filesystems must grow separately"
            }
            Self::DiskSize | Self::Resources => {
                "Stopped resource updates; Firecracker grows an unpartitioned ext4 root; no shrinking"
            }
            Self::Backup if storage == Storage::File => {
                "Linux strict reflink qualification required; no full-copy fallback; existing backup recovery is separate from admission"
            }
            Self::Backup => {
                "Stopped zvol only; verify ownership and capacity per operation; no memory or deleted-VM recovery"
            }
            Self::Run if engine == Engine::Jail && storage == Storage::Zvol => {
                "Jail requires file storage"
            }
            Self::Run if engine == Engine::Firecracker => {
                "Requires jailer and delegated cgroup controllers; zvol needs compatible agent support"
            }
            Self::Run if matches!(engine, Engine::Bhyve | Engine::Jail) => {
                "Experimental FreeBSD; manual host and guest setup required"
            }
            Self::Run => "Process readiness is not application readiness",
            Self::Ports => {
                "Reachability and reserved host port ranges remain deployment prerequisites"
            }
        }
    }
    pub fn require_design(self, engine: Engine, storage: Storage) -> Result<(), String> {
        if self.supported(engine, storage) {
            Ok(())
        } else {
            Err(format!(
                "capability {} unsupported by design for {engine}/{storage}: {}",
                self.tag(),
                self.limit(engine, storage)
            ))
        }
    }
}

/// The inputs that change technical requirements; values/content are validated separately.
#[derive(Default)]
pub struct Create {
    pub disk: bool,
    pub ports: bool,
    pub keys: bool,
    pub ssh: bool,
    pub guest_config: bool,
    pub deny_outgoing: bool,
    pub isolated: bool,
}
impl Create {
    pub fn requirements(&self, engine: Engine) -> Vec<Feature> {
        let mut result = vec![Feature::Run];
        for (needed, feature) in [
            (self.disk || engine == Engine::Qemu, Feature::DiskSize),
            (
                self.ports
                    || matches!(engine, Engine::Qemu | Engine::Bhyve | Engine::Jail)
                    || self.ssh
                    || self.keys,
                Feature::Ports,
            ),
            (
                self.ssh || (self.keys && engine != Engine::Jail),
                Feature::SshBootstrap,
            ),
            (self.keys && engine == Engine::Jail, Feature::RootKeys),
            (
                self.guest_config || ((self.ssh || self.keys) && engine == Engine::Firecracker),
                Feature::GuestConfig,
            ),
            (self.deny_outgoing, Feature::DenyOutgoing),
            (self.isolated, Feature::Isolation),
        ] {
            if needed {
                result.push(feature);
            }
        }
        result
    }
}
impl TryFrom<&crate::api::VmSpec> for Create {
    type Error = String;
    fn try_from(spec: &crate::api::VmSpec) -> Result<Self, Self::Error> {
        Ok(Self {
            disk: spec.disk.is_some(),
            ports: !spec.ports.is_empty(),
            keys: !spec.ssh_keys.is_empty(),
            ssh: crate::ssh::resolve_options(spec.engine, spec.ssh.as_ref(), &spec.ssh_keys)?
                .is_some(),
            guest_config: !spec.guest_config.is_empty(),
            deny_outgoing: spec.deny_outgoing,
            isolated: spec.isolated_network,
        })
    }
}
impl TryFrom<&crate::api::CreateVmReq> for Create {
    type Error = String;
    fn try_from(req: &crate::api::CreateVmReq) -> Result<Self, Self::Error> {
        Ok(Self {
            disk: req.disk > 0,
            ports: !req.ports.is_empty(),
            keys: !req.ssh_keys.is_empty(),
            ssh: crate::ssh::resolve_options(req.engine, req.ssh.as_ref(), &req.ssh_keys)?
                .is_some(),
            guest_config: !req.guest_config.is_empty(),
            deny_outgoing: req.deny_outgoing,
            isolated: req.isolated_network,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Definition {
    pub tag: String,
    pub meaning: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignSupport {
    pub tag: String,
    pub supported: bool,
    pub limit: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Combination {
    pub engine: Engine,
    pub storage: Storage,
    pub platform: String,
    pub experimental: bool,
    pub features: Vec<DesignSupport>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Matrix {
    pub version: u32,
    pub definitions: Vec<Definition>,
    pub create_fields: std::collections::BTreeMap<String, Vec<String>>,
    pub combinations: Vec<Combination>,
}
pub fn matrix() -> Matrix {
    Matrix {
        version: VERSION,
        create_fields: [
            ("disk", vec![Feature::DiskSize]),
            ("ssh_keys", vec![Feature::SshBootstrap, Feature::RootKeys]),
            ("guest_config", vec![Feature::GuestConfig]),
            ("deny_outgoing", vec![Feature::DenyOutgoing]),
            ("isolated_network", vec![Feature::Isolation]),
            ("ports", vec![Feature::Ports]),
        ]
        .into_iter()
        .map(|(field, features)| {
            (
                field.into(),
                features.into_iter().map(|f| f.tag().into()).collect(),
            )
        })
        .collect(),
        definitions: Feature::ALL
            .into_iter()
            .map(|f| Definition {
                tag: f.tag().into(),
                meaning: f.meaning().into(),
            })
            .collect(),
        combinations: ENGINES
            .into_iter()
            .flat_map(|engine| {
                STORAGE.into_iter().map(move |storage| {
                    let experimental = matches!(engine, Engine::Bhyve | Engine::Jail);
                    Combination {
                        engine,
                        storage,
                        experimental,
                        platform: if experimental { "freebsd" } else { "linux" }.into(),
                        features: Feature::ALL
                            .into_iter()
                            .map(|f| DesignSupport {
                                tag: f.tag().into(),
                                supported: f.supported(engine, storage),
                                limit: f.limit(engine, storage).into(),
                            })
                            .collect(),
                    }
                })
            })
            .collect(),
    }
}

/// Strings on the wire preserve unknown future tags; only typed local tags match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub tag: String,
    pub enabled: bool,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineReport {
    pub engine: Engine,
    pub features: Vec<Status>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub version: u32,
    pub storage: Storage,
    pub engines: Vec<EngineReport>,
    /// Admission configuration is separate from support and never fences old recovery work.
    pub backup_admission: bool,
}

fn missing_legacy(
    feature: Feature,
    engine: Engine,
    storage: Storage,
    tags: &[String],
) -> Option<&'static str> {
    let has = |tag: &str| tags.iter().any(|t| t == tag);
    let required: &[&str] = match (feature, engine, storage) {
        (Feature::Run, Engine::Firecracker, Storage::Zvol) => &[legacy::FC_JAILER, legacy::FC_ZVOL],
        (Feature::Run, Engine::Firecracker, _) => &[legacy::FC_JAILER],
        (Feature::SshBootstrap, _, _) => &[legacy::SSH],
        (Feature::GuestConfig, _, _) => &[legacy::CONFIG],
        (Feature::Isolation, _, _) => &[legacy::ISOLATION],
        (Feature::DenyOutgoing, Engine::Bhyve, _) => &[legacy::BHYVE_EGRESS],
        (Feature::DiskSize, Engine::Firecracker, _) => &[legacy::FC_DISK],
        (Feature::Resources, Engine::Qemu, _) => &[legacy::QEMU_RESOURCES],
        (Feature::Resources, Engine::Firecracker, _) => &[legacy::FC_RESOURCES],
        (Feature::Backup, _, _) => &[legacy::BACKUP],
        _ => &[],
    };
    required.iter().copied().find(|tag| !has(tag))
}
fn prerequisite_reason(tag: &str) -> &'static str {
    match tag {
        legacy::SSH => "ssh-keygen or ssh-keyscan is unavailable",
        legacy::CONFIG => "mkfs.ext4 is unavailable",
        legacy::ISOLATION => "nft or ip is unavailable",
        legacy::FC_JAILER => "delegated cgroup v2 cpu, memory or pids controllers are unavailable",
        legacy::FC_DISK | legacy::FC_RESOURCES => "e2fsck or resize2fs is unavailable",
        legacy::FC_ZVOL | legacy::QEMU_RESOURCES => {
            "engine or ZFS storage prerequisites are unavailable"
        }
        legacy::BHYVE_EGRESS => "compatible bhyve outgoing-filter support was not advertised",
        legacy::BACKUP => "the disk backup v1 protocol was not advertised",
        _ => "a required host prerequisite was not advertised",
    }
}

impl Report {
    /// Project the agent's existing probes into scoped canonical statuses.
    pub fn from_probes(
        storage: Storage,
        engines: &[Engine],
        tags: &[String],
        backup_admission: bool,
        backup_unsupported: Option<&str>,
    ) -> Self {
        Self {
            version: VERSION,
            storage,
            backup_admission,
            engines: ENGINES
                .into_iter()
                .map(|engine| EngineReport {
                    engine,
                    features: Feature::ALL
                        .into_iter()
                        .map(|feature| {
                            let reason = if !feature.supported(engine, storage) {
                                Some(format!(
                                    "unsupported by design: {}",
                                    feature.limit(engine, storage)
                                ))
                            } else if !engines.contains(&engine) {
                                Some("engine unavailable on this host".into())
                            } else if let Some(tag) = missing_legacy(feature, engine, storage, tags)
                            {
                                Some(format!("{} ({tag})", prerequisite_reason(tag)))
                            } else if feature == Feature::Backup {
                                backup_unsupported.map(str::to_owned)
                            } else {
                                None
                            };
                            let evidence = if reason.is_none() {
                                let source = match feature {
                                    Feature::SshBootstrap => "tools: ssh-keygen and ssh-keyscan found",
                                    Feature::GuestConfig => "tool: mkfs.ext4 probe succeeded",
                                    Feature::Isolation => "tools: nft and ip probes succeeded",
                                    Feature::DiskSize | Feature::Resources if engine == Engine::Firecracker => "tools: e2fsck and resize2fs probes succeeded",
                                    Feature::Backup if storage == Storage::File => "storage: runtime filesystem passed strict reflink qualification",
                                    Feature::Backup => "storage: ZFS tool probe succeeded; volume eligibility checked per request",
                                    _ => "implementation available; runtime and guest prerequisites checked during the operation",
                                };
                                vec![format!("engine detected: {engine}"), source.into()]
                            } else {
                                vec![]
                            };
                            Status {
                                tag: feature.tag().into(),
                                enabled: reason.is_none(),
                                reason,
                                evidence,
                            }
                        })
                        .collect(),
                })
                .collect(),
        }
    }
    pub fn require(
        &self,
        engine: Engine,
        storage: Storage,
        feature: Feature,
    ) -> Result<(), String> {
        feature.require_design(engine, storage)?;
        let denied = |reason: &str| {
            format!(
                "capability {} unavailable on host for {engine}/{storage}: {reason}",
                feature.tag()
            )
        };
        if self.version != VERSION {
            return Err(denied(
                "unknown capability report version; upgrade agent and controller",
            ));
        }
        if self.storage != storage {
            return Err(denied("capability report storage does not match host"));
        }
        let mut engines = self.engines.iter().filter(|entry| entry.engine == engine);
        let Some(entry) = engines.next() else {
            return Err(denied("engine report missing"));
        };
        if engines.next().is_some() {
            return Err(denied("ambiguous engine report"));
        }
        let mut features = entry
            .features
            .iter()
            .filter(|status| status.tag == feature.tag());
        let Some(status) = features.next() else {
            return Err(denied("capability missing from report"));
        };
        if features.next().is_some() {
            return Err(denied("ambiguous capability report"));
        }
        if !status.enabled || status.reason.is_some() {
            return Err(denied(
                status
                    .reason
                    .as_deref()
                    .unwrap_or("host did not enable this capability"),
            ));
        }
        Ok(())
    }
}

pub fn require_host(host: &Host, engine: Engine, features: &[Feature]) -> Result<(), String> {
    if features.contains(&Feature::Run) && !host.engines.contains(&engine) {
        return Err(format!("engine {engine} unavailable on host {}", host.id));
    }
    for &feature in features {
        feature.require_design(engine, host.storage)?;
        if let Some(report) = &host.capability_report {
            report.require(engine, host.storage, feature)?;
        } else if let Some(tag) = missing_legacy(feature, engine, host.storage, &host.capabilities)
        {
            return Err(format!(
                "capability {} unavailable on legacy host {}: missing {tag}; upgrade or configure the agent",
                feature.tag(),
                host.id
            ));
        }
    }
    Ok(())
}

/// Inspection and protocol replay stay available on unsupported/disabled backends.
pub fn backup_unsupported(
    engine: Engine,
    storage: Storage,
    backend_reason: Option<String>,
    report: Option<&Report>,
) -> Option<String> {
    Feature::Backup
        .require_design(engine, storage)
        .err()
        .map(|reason| format!("backup unsupported: {reason}"))
        .or(backend_reason)
        .or_else(|| {
            report
                .and_then(|r| r.require(engine, storage, Feature::Backup).err())
                .map(|reason| format!("backup unsupported: {reason}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags() -> Vec<String> {
        legacy::ALL.iter().map(|s| (*s).into()).collect()
    }
    fn host() -> Host {
        serde_json::from_value(serde_json::json!({
            "id":"host", "addr":"127.0.0.1:9100", "state":"online",
            "engines":["qemu","firecracker","bhyve","jail"], "images":[],
            "storage":"file", "registered_at":0,
            "resource":{"cpu_total":8,"cpu_used":0,"mem_total":8192,"mem_used":0,"disk_total":8192,"disk_used":0,"vm_count":0},
            "capabilities":tags()
        })).unwrap()
    }

    #[test]
    fn matrix_preserves_the_distinct_guest_and_network_contracts() {
        let matrix = matrix();
        assert_eq!(matrix.combinations.len(), 10);
        let unique: std::collections::HashSet<_> =
            matrix.definitions.iter().map(|d| &d.tag).collect();
        assert_eq!(unique.len(), Feature::ALL.len());
        for entry in &matrix.combinations {
            assert_eq!(entry.features.len(), Feature::ALL.len());
            assert_eq!(
                entry.experimental,
                matches!(entry.engine, Engine::Bhyve | Engine::Jail)
            );
        }
        assert!(!Feature::GuestConfig.supported(Engine::Qemu, Storage::File));
        assert!(Feature::DenyOutgoing.supported(Engine::Bhyve, Storage::Zvol));
        assert!(!Feature::Isolation.supported(Engine::Bhyve, Storage::Zvol));
        assert!(Feature::RootKeys.supported(Engine::Jail, Storage::File));
        assert!(!Feature::SshBootstrap.supported(Engine::Jail, Storage::File));
        for feature in Feature::ALL {
            assert!(!feature.supported(Engine::Jail, Storage::Zvol));
        }
        assert!(!Feature::Backup.supported(Engine::Bhyve, Storage::File));
        assert!(Feature::Backup.supported(Engine::Bhyve, Storage::Zvol));
    }

    #[test]
    fn requirements_distinguish_root_keys_bootstrap_egress_and_isolation() {
        let keys = Create {
            keys: true,
            ..Default::default()
        };
        let fc = keys.requirements(Engine::Firecracker);
        assert!(fc.contains(&Feature::SshBootstrap));
        assert!(fc.contains(&Feature::GuestConfig));
        assert!(fc.contains(&Feature::Ports));
        assert!(!fc.contains(&Feature::RootKeys));
        let jail = keys.requirements(Engine::Jail);
        assert!(jail.contains(&Feature::RootKeys));
        assert!(!jail.contains(&Feature::SshBootstrap));
        assert!(!jail.contains(&Feature::GuestConfig));
        let egress = Create {
            deny_outgoing: true,
            ..Default::default()
        }
        .requirements(Engine::Bhyve);
        assert!(egress.contains(&Feature::DenyOutgoing));
        assert!(!egress.contains(&Feature::Isolation));
        assert!(
            Create::default()
                .requirements(Engine::Qemu)
                .contains(&Feature::DiskSize)
        );
    }

    #[test]
    fn explicit_default_jail_ssh_normalizes_to_root_keys_on_both_api_paths() {
        let spec: crate::api::VmSpec = serde_json::from_value(serde_json::json!({
            "image":"freebsd-base", "engine":"jail", "ssh_keys":["ssh-ed25519 AAAA fixture"],
            "ssh":{"user":"root", "sudo":false}
        }))
        .unwrap();
        let req: crate::api::CreateVmReq = serde_json::from_value(serde_json::json!({
            "vm_id":"guest", "env_id":"env", "image":"freebsd-base", "engine":"jail",
            "cpu":1, "mem":128, "disk":0, "ports":[], "deny_outgoing":false,
            "ssh_keys":["ssh-ed25519 AAAA fixture"], "ssh":{"user":"root", "sudo":false}
        }))
        .unwrap();
        let controller = Create::try_from(&spec).unwrap().requirements(spec.engine);
        let agent = Create::try_from(&req).unwrap().requirements(req.engine);
        assert_eq!(controller, agent);
        assert!(agent.contains(&Feature::RootKeys));
        assert!(!agent.contains(&Feature::SshBootstrap));
        let mut invalid = req;
        invalid.ssh.as_mut().unwrap().sudo = true;
        assert!(Create::try_from(&invalid).is_err());
    }

    #[test]
    fn canonical_denial_is_authoritative_and_never_borrows_another_engines_support() {
        let mut host = host();
        let mut probes = tags();
        probes.retain(|t| t != legacy::QEMU_RESOURCES);
        host.capability_report = Some(Report::from_probes(
            Storage::File,
            &host.engines,
            &probes,
            false,
            None,
        ));
        // The retained global list still claims the old tag; the scoped report wins.
        assert!(require_host(&host, Engine::Qemu, &[Feature::Resources]).is_err());
        assert!(require_host(&host, Engine::Firecracker, &[Feature::Resources]).is_ok());
        host.capability_report = None;
        assert!(require_host(&host, Engine::Qemu, &[Feature::Resources]).is_ok());
    }

    #[test]
    fn unknown_missing_ambiguous_and_mismatched_reports_fail_closed() {
        let mut host = host();
        let valid = Report::from_probes(Storage::File, &host.engines, &tags(), false, None);
        for defect in 0..5 {
            let mut report = valid.clone();
            match defect {
                0 => report.version += 1,
                1 => report.storage = Storage::Zvol,
                2 => report.engines.retain(|e| e.engine != Engine::Qemu),
                3 => report.engines.push(report.engines[0].clone()),
                _ => {
                    let duplicate = report.engines[0].features[0].clone();
                    report.engines[0].features.push(duplicate);
                }
            }
            host.capability_report = Some(report);
            assert!(require_host(&host, Engine::Qemu, &[Feature::Run]).is_err());
        }
        let mut report = valid;
        let status = report.engines[0]
            .features
            .iter_mut()
            .find(|f| f.tag == Feature::SshBootstrap.tag())
            .unwrap();
        status.tag = "future.ssh.contract".into();
        let json = serde_json::to_string(&report).unwrap();
        let restored: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, report);
        assert!(json.contains("future.ssh.contract"));
        assert!(
            restored
                .require(Engine::Qemu, Storage::File, Feature::SshBootstrap)
                .is_err()
        );
    }

    #[test]
    fn legacy_gates_remain_engine_and_storage_specific() {
        let mut host = host();
        host.capabilities.clear();
        assert!(require_host(&host, Engine::Bhyve, &[Feature::Run]).is_ok());
        assert!(require_host(&host, Engine::Bhyve, &[Feature::DenyOutgoing]).is_err());
        host.capabilities.push(legacy::BHYVE_EGRESS.into());
        assert!(require_host(&host, Engine::Bhyve, &[Feature::DenyOutgoing]).is_ok());
        assert!(require_host(&host, Engine::Bhyve, &[Feature::Isolation]).is_err());
        host.capabilities = vec![Feature::Isolation.tag().into()];
        assert!(require_host(&host, Engine::Qemu, &[Feature::Isolation]).is_err());
        host.storage = Storage::Zvol;
        host.capabilities = vec![legacy::FC_JAILER.into()];
        assert!(require_host(&host, Engine::Firecracker, &[Feature::Run]).is_err());
        host.capabilities.push(legacy::FC_ZVOL.into());
        assert!(require_host(&host, Engine::Firecracker, &[Feature::Run]).is_ok());
        for (feature, engine, tag) in [
            (Feature::SshBootstrap, Engine::Qemu, legacy::SSH),
            (Feature::GuestConfig, Engine::Firecracker, legacy::CONFIG),
            (Feature::Isolation, Engine::Qemu, legacy::ISOLATION),
            (Feature::DiskSize, Engine::Firecracker, legacy::FC_DISK),
            (Feature::Resources, Engine::Qemu, legacy::QEMU_RESOURCES),
            (
                Feature::Resources,
                Engine::Firecracker,
                legacy::FC_RESOURCES,
            ),
            (Feature::Backup, Engine::Qemu, legacy::BACKUP),
        ] {
            host.capabilities = tags();
            assert!(require_host(&host, engine, &[feature]).is_ok());
            host.capabilities.retain(|t| t != tag);
            assert!(require_host(&host, engine, &[feature]).is_err());
        }
    }

    #[test]
    fn backup_qualification_is_separate_from_admission_and_runtime_eligibility() {
        let engines = [Engine::Bhyve];
        let report = Report::from_probes(Storage::Zvol, &engines, &tags(), false, None);
        assert!(!report.backup_admission);
        assert!(
            report
                .require(Engine::Bhyve, Storage::Zvol, Feature::Backup)
                .is_ok()
        );
        let unqualified = Report::from_probes(
            Storage::Zvol,
            &engines,
            &tags(),
            true,
            Some("ZFS tools unavailable"),
        );
        assert!(unqualified.backup_admission);
        let error = unqualified
            .require(Engine::Bhyve, Storage::Zvol, Feature::Backup)
            .unwrap_err();
        assert!(error.contains("unavailable on host"));
        assert!(error.contains("ZFS tools unavailable"));
        assert!(
            backup_unsupported(Engine::Jail, Storage::File, None, None)
                .unwrap()
                .contains("unsupported by design")
        );
    }
}
