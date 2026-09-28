# Lifecycle audit fixes — two-host validation, 2026-09-28

The targeted QEMU, Firecracker and Docker cases below passed on both explicitly
authorized Linux hosts, identified here as A and B. Tests used isolated services
and disposable guests; existing services were not upgraded.

## Tested source and local checks

Implementation revision: `0d45fc1`, comprising six focused fixes after `452bba6`.
The release binaries were built and tested before committing, from the exact
source subsequently committed. SHA-256 of
`git diff --binary 452bba6 0d45fc1 -- Cargo.toml Cargo.lock crates`:

`5c3c4ce873e357b1c39725d78fe135f3077d040cfd4ea149d02819d6f5460183`

Both hosts used identical, checksum-verified binaries:

| Binary | SHA-256 |
| --- | --- |
| `tt` | `9622a1f003468d7ed3160da3fbe3d6b6ac22917ac08f78d765fd1b1684c682a1` |
| `tt-agent` | `ce72013338ef73e9a5dd589048118dfd5e7eddd06b5561bcc9ada7c5ffdd6c35` |
| `tt-ctl` | `c92a800acc269c3aea741d571f5351d70146aa96ca685f316eb17977633bfc6b` |

Local validation passed 176 workspace tests: CLI 19, agent 37, controller 49 and
core 71. Formatting, workspace checking, Clippy for all targets with warnings
denied, Rust 1.88 checking and the locked release build passed. Ext4 tests used
their real external tools; no skipped case was counted as a pass. Each intermediate
commit's library/test targets also passed type compilation and formatting checks.

Focused regressions cover SSH-only configuration disks, completed and lost-reply
resize reconciliation, linked/ambiguous QEMU disks, runtime binding across database
reopen, unreadable legacy rows, network configuration changes, queued mutations
between VM probes, and unrelated slow hosts during controller operations.

## Hosts, isolation and resource bounds

Both hosts had 32 logical CPUs, 126380 MiB RAM and Linux 6.8.0-138-generic. Initial
load was approximately zero and available memory exceeded 120000 MiB. QEMU was
8.2.2 and Docker was 29.1.3. No existing TTstack guests or running containers were
present at preflight; existing SSH, Docker and containerd services were retained.

Each host used private databases, binaries, images, clone directories, credentials
and transient systemd services. A private bind mount isolated the fixed engine
runtime directory. Three network namespaces represented the agent, the outer SSH
gateway and an external client. They had no uplink to the host network. Firewall
changes and guest forwarding remained inside these namespaces.

The validated runs limited the service/QEMU/Docker slice to two CPUs,
1280 MiB RAM and zero swap. Firecracker uses a separate jailer cgroup: each guest
was limited to one or two CPUs and 384 or 512 MiB including VMM headroom, with zero
swap. At most one guest ran per host. Initial QEMU-only runs and image preparation
used a four-CPU, 2 GiB limit. These budgets, including the observed existing load
and small control processes, remained well below the authorized half-host ceiling.
No stress or throughput test was performed.

Observed service-slice memory peaks stayed below 450 MiB; sampled Firecracker
cgroup peaks stayed below 55 MiB. Firecracker metrics were accounted separately,
not assumed to belong to the service slice or summed repeatedly across old PIDs
referring to the same VM cgroup.

Docker used a separate daemon, socket and data/exec directories, while still using
the host containerd service. Initial attempts inherited its default `moby`
namespace; task-owned records were removed by exact container ID. The final run
used separate task and plugin namespaces, then removed both. The host's existing
containerd namespaces and Docker store were not reset.

## Guest fixtures

QEMU used the built-in pinned Alpine 3.21.7 NoCloud image, with its repository
SHA-512 verified before transfer. Firecracker/jailer were the official
[v1.17.0 release](https://github.com/firecracker-microvm/firecracker/releases/tag/v1.17.0).
The release archive SHA-256 was
`06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558`.

The Firecracker fixture used the previously validated 6.1.186 kernel
(`586a02db8ea1fd331d45efa6ebf502fff21ceddfedf010ef70ba389b8e095523`)
and a prepared 256 MiB ext4 Alpine rootfs with SSH bootstrap version 1.
It contained OpenSSH server 9.9_p2-r0 and sudo 1.9.17_p1-r0. APK signatures were
verified during preparation; package repositories were not content pinned.
The final rootfs, copied identically to both hosts, had SHA-256
`d95047c69ba0c0e26a893370570d5fb9a9b4c4c3f3c2637245b2bebb4c97a68d`.

Docker imported the pinned Alpine minirootfs into its private store and ran
`/bin/sleep infinity`; a synced file served as the persistence marker.

## Observed results

Every row passed on A and B using the same TTstack binaries, across isolated runs.
Only affected cases were repeated after correcting test-harness or fixture issues.

| Case | Observed result |
| --- | --- |
| QEMU linked directory image | Agent rejected the symbolic-link disk before creating a VM record or clone; the shared base-image checksum was unchanged. |
| QEMU lifecycle and resources | SSH worked directly and through the outer gateway. Root disk grew from 2048 to 3072 MiB; CPU/RAM changed from 1/256 to 2/384, then CPU decreased to 1 with the same disk. Repeated targets succeeded and the synced marker survived cold boots. |
| Firecracker SSH-only creation | Empty caller `guest_config` plus SSH options produced a readable, read-only `/dev/vdb` configuration disk and successful SSH. Total disk reservation was 260 MiB for a 256 MiB root disk. |
| Firecracker resize and retry | Root disk grew to 320 MiB while total reservation remained root plus 4 MiB. CPU/RAM changes and same-target retries completed without stuck pending intent. Guest block capacity and CPU count matched the target. |
| Agent/controller restart | Running guests remained accessible and their markers and provisioned SSH host identities were retained. Tests waited for the controller's host snapshot to become online before subsequent operations. |
| Lost VMM process recovery | With the isolated agent stopped, only the task's verified VMM PID was forcibly terminated. Restarting the agent observed a stopped guest with no stale network-recovery error. Explicit start retained the disk marker and SSH identity. |
| Network configuration binding | Changing the port range or removing ingress while a VM was tracked was rejected; persistent bindings were unchanged and the original configuration recovered access. |
| Docker lifecycle and binding | Real create, stop/start, agent/controller restart, retained-file access and delete passed. The stored runtime remained Docker. An explicit switch to Podman while the container existed was rejected before changing the binding. |
| API deletion | Agent/controller inventories and CPU, memory, disk and VM reservations returned to zero. VM clone paths and UUID-owned outer SSH rules were removed. Repeated QEMU/Firecracker environment deletion succeeded. |

## Shutdown and test limits

Normal stop and process-loss recovery are different observations. QEMU stop calls
took approximately 2.6–10.3 seconds; timing alone does not establish whether each
stop completed orderly shutdown or used termination fallback. Firecracker executed
the fixture's shutdown marker hook. The Docker sleeper required forced termination
after its ten-second stop grace period, as recorded by its daemon. All persistence
claims concern explicitly synced data, not guest memory or arbitrary applications.

The recovery fault used SIGKILL on task-owned VMMs, checked by PID and process start
time. Physical hosts were not rebooted. An initial harness attempt used guest
`poweroff`, which did not reliably terminate QEMU; the explicit process-loss fault
made the intended recovery condition deterministic. Other harness corrections
handled transient systemd unit collection and controller heartbeat readiness.
The prepared Firecracker init was corrected to tolerate devices and addresses
already initialized by the kernel. These changes did not alter the tested TTstack
source or release binaries.

This run adds no live Podman, ZFS/zvol, OpenRC, real host-power-cycle, maximum-capacity
or performance claim. Dual-runtime ownership, legacy binding migration, corrupted
records, delayed probes and lost resize replies have local regression coverage;
the live runtime-switch check used an existing Docker container, not a Podman
installation or an in-place production upgrade.

## Cleanup and maintained contracts

All task VMs, containers, transient services, namespace links, owned firewall
rules, VM cgroups, private Docker/containerd state, credentials and remote staging
directories were removed. Final comparisons matched host addresses, routes,
firewall structure, running services, Docker inventory and the original engine
runtime-directory state. SSH, Docker and containerd were active after cleanup.
An intermediate B snapshot observed PackageKit exit successfully (`daemon quit`);
the task neither stopped nor restarted that unrelated service.

Maintained behavior and upgrade requirements remain in the
[lifecycle API](../rest-api.md#lifecycle-and-recovery),
[deployment schema guidance](../deployment.md#resource-update-schema-gate),
[image guide](../guest-images.md) and [SSH contract](../ssh.md).
