# Compatibility and validation scope

Linux x86_64 is the supported host implementation and CI target, with QEMU/KVM,
Firecracker and Docker/Podman. The agent also includes experimental FreeBSD
Bhyve/Jail paths; this is outside the Linux support commitment and CI scope.

## Implementation versus live verification

Evidence below is tied to each report's revision, fixture and limits. A later
code change does not inherit a fresh live-validation claim from an older report.

| Workflow | Implementation | Recorded live evidence |
| --- | --- | --- |
| Linux QEMU/KVM + file storage | Cloud-init, initial SSH accounts/keys, TCP forwarding and offline resource updates | [Two-host audit fixes, 2026-09-28](validation/audit-fixes-2026-09-28.md): Alpine 3.21.7 SSH, resize, restart recovery and cleanup; [initial SSH acceptance](validation/expert-ssh-2026-09-28.md) covers ordinary-user SSH/SCP and sudo |
| Linux Docker | Container lifecycle, persisted runtime binding and native port publishing | [Two-host audit fixes, 2026-09-28](validation/audit-fixes-2026-09-28.md): retained-container lifecycle and binding; [2026-09-24 lifecycle](validation/live-validation-2026-09-24.md) covers a published HTTP workload |
| Linux Firecracker + file storage | Jailer, configuration drive, prepared-image SSH, offline ext4 growth, shutdown and opt-in isolation | [Two-host audit fixes, 2026-09-28](validation/audit-fixes-2026-09-28.md): SSH-only configuration disks, resource accounting and recovery; [2026-09-24 follow-up](validation/firecracker-validation-2026-09-24.md) covers isolation and shutdown cases |
| Linux Firecracker + zvol | Snapshot clones, jailed block devices, prepared-image SSH and ext4 growth | [Zvol lifecycle, 2026-09-24](validation/firecracker-zvol-validation-2026-09-24.md); [subsequent SSH deployment](validation/expert-ssh-2026-09-28.md#subsequent-firecrackerzfs-deployment) |
| QEMU + zvol | Raw ZFS volumes, snapshot clones and offline resource updates | [Alpine resize and recovery, 2026-09-26](validation/qemu-resize-validation-2026-09-26.md) on file storage and dedicated ZFS pools |
| Podman | Alternate container runtime bound to the agent inventory | [Native probes, 2026-09-26](validation/engine-capability-probes-2026-09-26.md); these did not validate the complete TTstack lifecycle or enable container resource/network-policy APIs |
| Ubuntu cloud guest | Built-in QEMU recipe | Not covered by the listed guest lifecycle runs |
| Linux/systemd deployment | Local/distributed service generation and isolated service restart | [Linux upgrade, 2026-09-24](validation/linux-host-upgrade-validation-2026-09-24.md) and later isolated runs; not every deploy configuration |
| Agent database initialization | Current native schema, identity and runtime/network bindings | [Process-level checks, 2026-09-28](validation/native-agent-schema-2026-09-28.md): fresh initialization, reopen and non-mutating rejection of incompatible fixtures; no production data migration |
| Linux/OpenRC, musl binaries | Distributed deployment support | No complete live deployment coverage in the listed reports |
| FreeBSD Bhyve/Jail/PF | Restored experimental implementation, manual setup | No new native build or live verification; see [limits below](#experimental-freebsd-restoration) |
| Other host platforms | No validated agent deployment path | No support commitment |

The [validation index](README.md#validation-evidence) retains earlier engine,
storage, networking and caller evidence. Unit tests, prerequisite detection and
a recipe's presence do not establish guest boot or application readiness.

## Experimental FreeBSD restoration

This branch extracts the FreeBSD code removed by `8775fdb` and patches it onto
`83b7e95`, retaining the later Linux Firecracker, ZFS and lifecycle fixes. It
restores the `bhyve` and `jail` API values, engine detection, `sysctl hw.physmem`
memory detection, ifconfig/PF networking, the `freebsd-base` recipe and CLI/UI
entries. These paths remain experimental. The core library and its test targets
pass cross-target checking for `x86_64-unknown-freebsd` on Linux; no native
FreeBSD agent/controller build or guest boot was performed.

Use native FreeBSD binaries and manual service setup; automated deployment still
requires Linux. Bhyve invokes `bhyveload`, `bhyve` and `bhyvectl`, so its disk must
already be bootable by that loader. There is no Bhyve image recipe, SSH injection
or disk resizing. Zvol readiness accepts FreeBSD character devices, matching
the [OpenZFS FreeBSD implementation](https://github.com/openzfs/zfs/blob/master/module/os/freebsd/zfs/zvol_os.c);
Linux still requires block devices. Jail expects a copied root directory with file storage; zvols
are rejected for Jail by both scheduler and agent. Its SSH-key support only writes
`root/.ssh/authorized_keys`; the restored code does not start guest services or
configure sshd. Jail CPU/memory values are scheduling reservations, not enforced
host limits. Linux guest configuration drives and network isolation are rejected
for both FreeBSD engines.

The extraction preserves known limitations of the old implementation:

- Bhyve rejects in-place restart. Jail stop removes the jail, while start tries
  to modify an existing jail; do not assume Linux stop/start guarantees apply.
- TAP creation is not idempotent, and the Jail path uses shared host networking.
  Network recovery and repeated operations need native lifecycle validation.
- PF requires operator configuration in `/etc/pf.conf`. The restored rule writer
  reloads a shared anchor per rule; multiple forwards or guests can overwrite
  earlier rules. Cleanup errors can be ignored by the legacy engine/network code.
- `freebsd-base` uses the host major release with a fixed `.3-RELEASE` suffix
  (fallback `14.3-RELEASE`). Download availability is unverified, and a failed
  extraction can leave a directory that later attempts treat as complete.

The Linux-only release cannot deserialize restored engine names in persisted
VMs or cached host lists. Upgrade controller and agents together when trying this
branch. Before returning to Linux-only binaries, use this branch to remove all
FreeBSD workloads and unregister their hosts; keep consistent state/disk backups.
Existing dated Linux reports below retain their original tested revisions and scope.

## Build and CI

The workspace uses Rust **1.88+**, edition 2024, with committed `Cargo.lock` and
locked builds. CI checks the minimum toolchain separately from stable and runs
formatting, Clippy, tests and a release build. SQLite is bundled; HTTP client TLS
uses rustls/AWS-LC rather than OpenSSL. Native dependencies still need a C/C++
compiler. See [deployment prerequisites](deployment.md#prerequisites).

Linux tests also need `mkfs.ext4`, `debugfs`, `dumpe2fs`, `e2fsck` and `resize2fs` from `e2fsprogs` to inspect a real
configuration disk without mounting it or requiring root. CI installs these tools.
Tests cover local logic and mock-agent HTTP workflows: interrupted creation,
partial deletion, expiry retry, failed stop, duplicate creation, input validation,
resource accounting, configuration-disk overhead, runtime/network bindings,
database-format rejection and port reuse. They do not boot guests or modify host
firewall rules. `make doc` generates Rust API documentation, not the HTTP reference.

For changes affecting live behavior, validate the affected engine's create/access,
stop/start with retained data, agent/controller restart and final cleanup on a
suitable host. Host reboot on ZFS file datasets has a separate
[restart record](validation/firecracker-restart-validation-2026-09-24.md); zvol coverage and
its limits are recorded in the [zvol validation](validation/firecracker-zvol-validation-2026-09-24.md). No performance or maximum-capacity test
has been performed by that run.

## SQLite engine assessment — 2026-09-24

The historical database-engine comparison and dependency-refresh probes are
archived in the [dated assessment](validation/sqlite-engine-assessment-2026-09-24.md).
They describe that decision and its limits, not current dependency freshness or
an upgrade policy for future data.

## Product boundaries

- One controller, with configured limits of 50 hosts and 1000 tracked VMs. These
  are guardrails, not benchmarked capacity. Mutations serialize per environment and reads
  generally serve cached/persisted state.
- Environments group lifecycle operations; they do not provide cross-host private
  networking or tenant authorization. Linux QEMU/Firecracker offer opt-in guest
  network isolation; Firecracker also uses jailer and resource limits. The API key
  remains a shared administrator credential. User identity, entitlements, application
  installation, connection leases and idle-stop policy belong in callers.
- No automatic guest restart after host reboot, guest migration, HA, distributed storage,
  image distribution or application/database provisioning.
- QEMU and Firecracker support stopped-VM CPU/RAM updates and disk growth on file
  and ZFS storage. QEMU requires `qemu_resources` and expands only the virtual disk;
  the guest must grow its partitions/filesystems. See the
  [offline resource API](rest-api.md#offline-resource-updates).
- Docker does not support managed SSH keys, disk quotas or outgoing restrictions.
  Firecracker supports creation-time ext4 growth and explicit stopped-VM resource
  updates (`firecracker_resources`) and initial SSH bootstrap on prepared images
  (`ssh_bootstrap`), but not interactive consoles or resizing a running VM;
  CPU/RAM changes need a cold boot. See [SSH support and limits](ssh.md).
- Runtime and guest dependencies remain the operator's responsibility. Prefer
  QEMU cloud images for full VMs and prepared long-running Docker images for services.

See the [image guide](guest-images.md) for engine-specific access and storage, and
the [API reference](rest-api.md#lifecycle-and-recovery) for state and recovery semantics.
