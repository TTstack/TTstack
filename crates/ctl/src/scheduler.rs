//! VM placement scheduler.
//!
//! Decides which host should run each VM based on available resources,
//! supported engines, image availability, and a simple best-fit strategy.

use ruc::*;
use std::collections::{HashMap, HashSet};
use ttcore::api::VmSpec;
use ttcore::model::*;

/// Result of scheduling: VM spec + chosen host.
#[derive(Debug)]
pub struct Placement {
    pub host_id: String,
    pub host_addr: String,
}

// Unspecified Firecracker rootfs size is checked by the agent, which owns the image.
fn disk_reservation(spec: &VmSpec) -> u32 {
    spec.disk
        .unwrap_or(spec.engine.default_disk())
        .saturating_add(
            if spec.engine == Engine::Firecracker && !spec.guest_config.is_empty() {
                ttcore::guest_config::CONFIG_DISK_MIB
            } else {
                0
            },
        )
}

/// Choose the best host for a VM spec using a best-fit strategy.
///
/// Prefers the host with the least free resources that can still
/// accommodate the VM, to pack hosts densely and leave larger hosts
/// available for bigger workloads.
///
/// `host_images` maps host_id → set of available image names.
/// If the map is empty, image validation is skipped (for backward compat).
pub fn place_vm(
    hosts: &[Host],
    spec: &VmSpec,
    host_images: &HashMap<String, HashSet<String>>,
) -> Result<Placement> {
    let cpu = spec.cpu.unwrap_or(VM_CPU_DEFAULT);
    let mem = spec
        .engine
        .memory_reservation(spec.mem.unwrap_or(VM_MEM_DEFAULT));
    let disk = disk_reservation(spec);

    // Docker images are managed by Docker, not by the image directory
    let check_images = !host_images.is_empty() && spec.engine != Engine::Docker;
    let supports = |h: &Host| {
        let has = |cap: &str| h.capabilities.iter().any(|c| c == cap);
        (!spec.isolated_network || has("isolated_network"))
            && (spec.guest_config.is_empty() || has("guest_config"))
            && (spec.engine != Engine::Firecracker || has("firecracker_jailer"))
            && (spec.engine != Engine::Firecracker
                || spec.disk.is_none()
                || has("firecracker_disk_resize"))
    };

    let mut candidates: Vec<&Host> = hosts
        .iter()
        .filter(|h| {
            h.state == HostState::Online
                && supports(h)
                && h.engines.contains(&spec.engine)
                && !(h.storage == Storage::Zvol && spec.engine == Engine::Firecracker)
                && h.resource.can_fit(cpu, mem, disk)
                && (!check_images
                    || host_images
                        .get(&h.id)
                        .is_some_and(|imgs| imgs.contains(&spec.image)))
        })
        .collect();

    if candidates.is_empty() {
        if hosts.iter().any(|h| h.state == HostState::Online)
            && !hosts
                .iter()
                .any(|h| h.state == HostState::Online && supports(h))
        {
            return Err(eg!(
                "no online agent supports the requested capabilities; upgrade agents before using guest_config, isolated_network, jailed Firecracker or Firecracker disk sizing"
            ));
        }
        // Provide a more helpful error message
        let online = hosts
            .iter()
            .filter(|h| h.state == HostState::Online)
            .count();
        let with_engine = hosts
            .iter()
            .filter(|h| h.state == HostState::Online && h.engines.contains(&spec.engine))
            .count();
        let with_resource = hosts
            .iter()
            .filter(|h| {
                h.state == HostState::Online
                    && h.engines.contains(&spec.engine)
                    && !(h.storage == Storage::Zvol && spec.engine == Engine::Firecracker)
                    && h.resource.can_fit(cpu, mem, disk)
            })
            .count();

        if online == 0 {
            return Err(eg!("no online hosts available"));
        } else if with_engine == 0 {
            return Err(eg!("no online host supports engine={}", spec.engine,));
        } else if with_resource == 0 {
            return Err(eg!(
                "no host has enough resources for engine={}, cpu={}, mem={}MB, disk={}MB",
                spec.engine,
                cpu,
                mem,
                disk,
            ));
        } else {
            return Err(eg!(
                "no host has image '{}' for engine={}",
                spec.image,
                spec.engine,
            ));
        }
    }

    // Sort by free memory ascending (best-fit)
    candidates.sort_by_key(|h| h.resource.mem_free());

    let host = candidates[0];
    Ok(Placement {
        host_id: host.id.clone(),
        host_addr: host.addr.clone(),
    })
}

/// Schedule an entire environment's VMs across the fleet.
///
/// Returns a list of (VmSpec, Placement) pairs.
pub fn schedule_env(
    hosts: &[Host],
    specs: &[VmSpec],
    host_images: &HashMap<String, HashSet<String>>,
) -> Result<Vec<(VmSpec, Placement)>> {
    let mut result = Vec::with_capacity(specs.len());

    // Work with a mutable copy of host resources for multi-VM scheduling
    let mut shadow: Vec<Host> = hosts.to_vec();

    for spec in specs {
        let placement = place_vm(&shadow, spec, host_images)?;

        // Update shadow resources to account for this allocation
        if let Some(h) = shadow.iter_mut().find(|h| h.id == placement.host_id) {
            let cpu = spec.cpu.unwrap_or(VM_CPU_DEFAULT);
            let mem = spec
                .engine
                .memory_reservation(spec.mem.unwrap_or(VM_MEM_DEFAULT));
            let disk = disk_reservation(spec);
            h.resource.cpu_used += cpu;
            h.resource.mem_used += mem;
            h.resource.disk_used += disk;
            h.resource.vm_count += 1;
        }

        result.push((spec.clone(), placement));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_host(id: &str, cpu: u32, mem: u32, engines: Vec<Engine>) -> Host {
        Host {
            capabilities: vec![
                "guest_config".into(),
                "isolated_network".into(),
                "firecracker_jailer".into(),
                "firecracker_disk_resize".into(),
            ],
            id: id.into(),
            addr: format!("{id}:9100"),
            resource: Resource {
                cpu_total: cpu,
                cpu_used: 0,
                mem_total: mem,
                mem_used: 0,
                disk_total: 500_000,
                disk_used: 0,
                vm_count: 0,
            },
            state: HostState::Online,
            engines,
            storage: Storage::File,
            images: vec![],
            registered_at: 0,
        }
    }

    fn make_spec() -> VmSpec {
        VmSpec {
            isolated_network: false,
            guest_config: Default::default(),
            image: "ubuntu".into(),
            engine: Engine::Qemu,
            cpu: Some(2),
            mem: Some(1024),
            disk: Some(40960),
            ports: vec![22],
            deny_outgoing: false,
            ssh_keys: vec![],
        }
    }

    #[test]
    fn isolation_and_configuration_are_never_scheduled_on_legacy_agents() {
        let mut host = make_host("old", 8, 4096, vec![Engine::Qemu, Engine::Firecracker]);
        host.capabilities.clear();
        let mut spec = make_spec();
        spec.isolated_network = true;
        assert!(
            place_vm(std::slice::from_ref(&host), &spec, &HashMap::new())
                .unwrap_err()
                .to_string()
                .contains("capabilities")
        );
        spec.isolated_network = false;
        assert!(place_vm(std::slice::from_ref(&host), &spec, &HashMap::new()).is_ok());
        spec.engine = Engine::Firecracker;
        spec.disk = None;
        spec.guest_config
            .insert("settings.json".into(), "{}".into());
        assert!(place_vm(&[host], &spec, &HashMap::new()).is_err());
    }

    #[test]
    fn firecracker_disk_sizing_requires_capability_and_reserves_config_drive() {
        let mut host = make_host("disk-host", 8, 8192, vec![Engine::Firecracker]);
        let mut spec = make_spec();
        spec.engine = Engine::Firecracker;
        spec.disk = Some(8192);
        spec.guest_config
            .insert("settings.json".into(), "{}".into());
        host.capabilities.retain(|c| c != "firecracker_disk_resize");
        assert!(place_vm(std::slice::from_ref(&host), &spec, &HashMap::new()).is_err());
        host.capabilities.push("firecracker_disk_resize".into());
        host.resource.disk_total = 8192;
        assert!(place_vm(std::slice::from_ref(&host), &spec, &HashMap::new()).is_err());
        host.resource.disk_total = 8196;
        assert!(place_vm(std::slice::from_ref(&host), &spec, &HashMap::new()).is_ok());
        host.resource.disk_total = 16388; // Two rootfs disks fit, their two config drives do not.
        assert!(schedule_env(&[host], &[spec.clone(), spec], &HashMap::new()).is_err());
    }

    #[test]
    fn firecracker_reserves_vmm_memory_headroom() {
        let host = make_host("small", 2, 256, vec![Engine::Firecracker]);
        let mut spec = make_spec();
        spec.engine = Engine::Firecracker;
        spec.mem = Some(256);
        spec.disk = None;
        assert!(place_vm(std::slice::from_ref(&host), &spec, &HashMap::new()).is_err());
        spec.mem = Some(128);
        assert!(place_vm(&[host], &spec, &HashMap::new()).is_ok());
    }

    fn empty_images() -> HashMap<String, HashSet<String>> {
        HashMap::new()
    }

    fn images_for(host_id: &str, imgs: &[&str]) -> HashMap<String, HashSet<String>> {
        let mut m = HashMap::new();
        m.insert(host_id.into(), imgs.iter().map(|s| s.to_string()).collect());
        m
    }

    #[test]
    fn place_vm_picks_online_host() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Qemu])];
        let p = place_vm(&hosts, &make_spec(), &empty_images()).unwrap();
        assert_eq!(p.host_id, "h1");
    }

    #[test]
    fn place_vm_skips_offline_host() {
        let mut h = make_host("h1", 8, 16384, vec![Engine::Qemu]);
        h.state = HostState::Offline;
        let hosts = vec![h];
        assert!(place_vm(&hosts, &make_spec(), &empty_images()).is_err());
    }

    #[test]
    fn place_vm_skips_wrong_engine() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Docker])];
        let spec = make_spec(); // wants Qemu
        assert!(place_vm(&hosts, &spec, &empty_images()).is_err());
    }

    #[test]
    fn place_vm_skips_insufficient_resources() {
        let hosts = vec![make_host("h1", 1, 512, vec![Engine::Qemu])];
        let spec = make_spec(); // needs 2 CPU, 1024 mem
        assert!(place_vm(&hosts, &spec, &empty_images()).is_err());
    }

    #[test]
    fn place_vm_best_fit_prefers_smaller() {
        // h1 has more room, h2 has less but enough
        let hosts = vec![
            make_host("h1", 16, 32768, vec![Engine::Qemu]),
            make_host("h2", 4, 4096, vec![Engine::Qemu]),
        ];
        let p = place_vm(&hosts, &make_spec(), &empty_images()).unwrap();
        assert_eq!(p.host_id, "h2"); // best-fit picks smaller
    }

    #[test]
    fn place_vm_checks_image_availability() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Qemu])];
        let imgs = images_for("h1", &["alpine"]);
        let spec = make_spec(); // wants "ubuntu"
        let result = place_vm(&hosts, &spec, &imgs);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("image"));
    }

    #[test]
    fn place_vm_with_matching_image() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Qemu])];
        let imgs = images_for("h1", &["ubuntu", "alpine"]);
        let p = place_vm(&hosts, &make_spec(), &imgs).unwrap();
        assert_eq!(p.host_id, "h1");
    }

    #[test]
    fn place_vm_skips_image_check_for_docker() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Docker])];
        let imgs = images_for("h1", &["alpine"]); // no "ubuntu"
        let mut spec = make_spec();
        spec.engine = Engine::Docker;
        // Should succeed — Docker images are not checked against host_images
        let p = place_vm(&hosts, &spec, &imgs).unwrap();
        assert_eq!(p.host_id, "h1");
    }

    #[test]
    fn schedule_env_distributes_when_full() {
        let hosts = vec![
            make_host("h1", 4, 4096, vec![Engine::Qemu]),
            make_host("h2", 4, 4096, vec![Engine::Qemu]),
        ];

        // 3 VMs each needing 2 CPU: h1 takes 2 (filling up), h2 takes 1
        let specs: Vec<VmSpec> = (0..3).map(|_| make_spec()).collect();
        let placements = schedule_env(&hosts, &specs, &empty_images()).unwrap();
        assert_eq!(placements.len(), 3);

        let on_h1 = placements.iter().filter(|(_, p)| p.host_id == "h1").count();
        let on_h2 = placements.iter().filter(|(_, p)| p.host_id == "h2").count();
        assert_eq!(on_h1, 2);
        assert_eq!(on_h2, 1);
    }

    #[test]
    fn schedule_env_fails_if_no_capacity() {
        let hosts = vec![make_host("h1", 2, 2048, vec![Engine::Qemu])];
        let specs: Vec<VmSpec> = (0..2).map(|_| make_spec()).collect();
        assert!(schedule_env(&hosts, &specs, &empty_images()).is_err());
    }

    #[test]
    fn schedule_env_empty_specs_ok() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Qemu])];
        let placements = schedule_env(&hosts, &[], &empty_images()).unwrap();
        assert!(placements.is_empty());
    }

    #[test]
    fn error_message_no_online() {
        let mut h = make_host("h1", 8, 16384, vec![Engine::Qemu]);
        h.state = HostState::Offline;
        let err = place_vm(&[h], &make_spec(), &empty_images())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no online hosts"));
    }

    #[test]
    fn error_message_no_engine() {
        let hosts = vec![make_host("h1", 8, 16384, vec![Engine::Docker])];
        let err = place_vm(&hosts, &make_spec(), &empty_images())
            .unwrap_err()
            .to_string();
        assert!(err.contains("engine=qemu"));
    }

    #[test]
    fn error_message_no_resource() {
        let hosts = vec![make_host("h1", 1, 512, vec![Engine::Qemu])];
        let err = place_vm(&hosts, &make_spec(), &empty_images())
            .unwrap_err()
            .to_string();
        assert!(err.contains("resources"));
    }
}
