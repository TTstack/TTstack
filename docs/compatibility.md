# Compatibility and validation scope

Linux x86_64 is the supported host implementation and CI target, with QEMU/KVM,
Firecracker and Docker/Podman. The agent also includes experimental FreeBSD
Bhyve/Jail paths; this is outside the Linux support commitment and CI scope.

## Implementation versus live verification

Evidence below is tied to each report's revision, fixture and limits. A later
code change does not inherit a fresh live-validation claim from an older report.

| Workflow | Implementation | Recorded live evidence |
| --- | --- | --- |
| Linux QEMU/KVM + file storage | Cloud-init, initial SSH accounts/keys, TCP forwarding and offline resource updates | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): SSH, resize, backup/restore and cleanup; [initial SSH acceptance](validation/expert-ssh-2026-09-28.md) covers ordinary-user SSH/SCP and sudo |
| Linux Docker | Container lifecycle, persisted runtime binding and native port publishing | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): published HTTP, retained-container restart, unsupported requests and automatic expiry on an isolated runtime |
| Linux Firecracker + file storage | Jailer, configuration drive, prepared-image SSH, offline ext4 growth, shutdown and opt-in isolation | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): SSH, read-only configuration, isolation, backup/restore, resource updates and prerequisite withdrawal |
| Linux Firecracker + zvol | Snapshot clones, jailed block devices, prepared-image SSH and ext4 growth | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): SSH/sudo, configuration, resource updates, backup/restore and deletion |
| Linux QEMU + zvol | Raw ZFS volumes, snapshot clones and offline resource updates | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): SSH, isolation, offline-agent recovery, backup/restore and resource updates |
| Stopped VM disk backup | QEMU/Firecracker root-disk recovery points on zvol or qualified reflink storage; experimental bhyve on zvol | [Two-host backup validation](validation/disk-backup-validation-2026-09-29.md) and [mixed fleet](validation/capability-tags-mixed-fleet-2026-09-29.md), 2026-09-29: restore/replay, unsupported disks, admission withdrawal and cleanup; no online or host-power-loss claim |
| Mixed Linux/FreeBSD capability admission | Shared engine/storage matrix, scoped host reports and explicit legacy fallback | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): one environment across three hosts, authoritative denials, legacy compatibility and offline report retention |
| Podman | Alternate container runtime bound to the agent inventory | [Native probes, 2026-09-26](validation/engine-capability-probes-2026-09-26.md); these did not validate the complete TTstack lifecycle or enable container resource/network-policy APIs |
| Ubuntu cloud guest | Built-in QEMU recipe | Not covered by the listed guest lifecycle runs |
| Linux/systemd deployment | Local/distributed service generation and isolated service restart | [Linux upgrade, 2026-09-24](validation/linux-host-upgrade-validation-2026-09-24.md) and later isolated runs; not every deploy configuration |
| Agent database initialization | Current native schema, identity and runtime/network bindings | [Process-level checks, 2026-09-28](validation/native-agent-schema-2026-09-28.md): fresh initialization, reopen and non-mutating rejection of incompatible fixtures; no production data migration |
| Offline native v5 agent conversion | Explicit converter writes a separate v6 database while preserving validated VM metadata; no automatic daemon migration | [Conversion rehearsal, 2026-09-29](validation/agent-v5-conversion-2026-09-29.md): rejection fixtures and five-VM metadata/storage preservation; not every legacy format or a new deployment acceptance run |
| Linux/OpenRC, musl binaries | Distributed deployment support | No complete live deployment coverage in the listed reports |
| FreeBSD bhyve/Jail/PF | Experimental implementation, manual setup | [Mixed fleet, 2026-09-29](validation/capability-tags-mixed-fleet-2026-09-29.md): native FreeBSD 15.1 bhyve/zvol and Jail/file lifecycle, SSH, capability rejection, backup eligibility and cleanup |
| Other host platforms | No validated agent deployment path | No support commitment |

The [validation index](README.md#validation-evidence) retains earlier engine,
storage, networking and caller evidence. Unit tests, prerequisite detection and
a recipe's presence do not establish guest boot or application readiness.

## Experimental FreeBSD restoration

The `bhyve` and `jail` engines remain experimental. Native binaries and manual
service/network setup are required; automated deployment and primary CI remain
Linux-only. Live FreeBSD validation covers **FreeBSD 15.1-RELEASE-p3 (amd64) only**;
other FreeBSD versions have not been live-tested. The
[mixed-fleet report](validation/capability-tags-mixed-fleet-2026-09-29.md) records
the latest tested revision, native test execution and Linux/FreeBSD lifecycle
cases. It does not establish native FreeBSD release-build or MSRV coverage.

Host implementations are selected with Rust `cfg(target_os)`, not additive Cargo
features: Linux includes QEMU, Firecracker and Docker/Podman; FreeBSD includes
bhyve and Jail. Linux nftables SSH namespace ingress is excluded from FreeBSD.
Shared engine names, models and validation remain available on both targets so
a controller can manage a mixed fleet. No Cargo feature enables another OS's
host implementation; an incompatible engine is rejected by the local factory.

- Bhyve uses `bhyveload`, `bhyve`, `bhyvectl` and `/dev/vmmctl`. Supply a raw disk
  that `bhyveload` can boot, with its own guest networking and credentials. There
  is no Bhyve image recipe, SSH injection, guest configuration or resource resize.
  Renamed interfaces retain a separately recorded `/dev/tapN` device. Startup
  failures are reported with a runtime log; process identity is checked before
  termination. A leftover VMM device alone does not mean a VM is running.
- Jail uses file storage with a copied FreeBSD root. The `freebsd-base` recipe
  requires a RELEASE host, uses its exact major/minor release (without the patch
  suffix), downloads with the native `fetch` utility, and publishes only a
  completely extracted/configured root. Existing incomplete/unmanaged recipe
  directories require inspection and moving aside before retry. Custom roots
  must provide `/etc/rc`, `/etc/rc.shutdown` and their desired services. Jail root
  paths must be UTF-8 without whitespace.
- Jails share the agent's IP stack, receive an alias on `tt0`, and run their rc
  scripts. Public keys enable root key authentication; the built-in recipe starts
  sshd. This is not VNET isolation. Jail rejects `deny_outgoing`; retained Jail
  records with that option report a recovery error. Bhyve supports this option
  through PF, gated by scoped `net.egress.deny` support (`bhyve_deny_outgoing`
  for legacy agents without a report). See [capability compatibility](capabilities.md#compatibility).
- Stop/start cold-boots a retained bhyve disk or Jail root. Jail stop invokes
  `rc.shutdown`; bhyve stop requests ACPI shutdown with SIGTERM and can fall back
  to SIGKILL. Deletion waits for confirmed termination, and a failed devfs
  unmount retains the Jail root. Guest memory is not retained.
- Jail CPU/memory and disk usage are not enforced host limits. Jail disk
  accounting remains zero. bhyve reserves the full raw file or zvol capacity,
  rounded up to MiB; placement requires an agent that reports image sizes and
  admission rechecks capacity before cloning. Disk sizing/resizing is still
  unsupported. CPU/memory reservations are used for placement. Jail zvol storage is rejected. Bhyve zvols accept FreeBSD
  character devices; see the [native zvol follow-up](validation/freebsd-zvol-validation-2026-09-28.md)
  for clone lifecycle and retry coverage.
- bhyve on zvol storage supports the shared opt-in [disk backup protocol](disk-backup.md),
  including stopped-only admission, ownership checks, retry and deferred cleanup.
  FreeBSD file storage has no strict reflink backend; Jail roots are ineligible.
  Unsupported backup requests never fall back to a full copy.
- Both FreeBSD engines reject Linux guest configuration and `isolated_network`.
  Keep mutually untrusted tenants on a separately validated isolation setup.

The agent uses the same native database format and persistent network ownership
checks as Linux. Its configured `--port-start`/`--port-end` range governs PF
mappings and cannot change while VM records remain. Reconciliation probes one
VM at a time, allowing queued lifecycle operations between probes. Linux outer
SSH namespace options are rejected on FreeBSD; Jail root-key access continues
through its ordinary mapped port without managed SSH endpoint metadata.

PF must already be enabled and the operator must install the following hooks
in the active root ruleset, with the filter hook before broader pass rules:

```pf
# Place any operator-owned outbound NAT here, before rdr rules.
rdr-anchor "ttstack/*"
anchor "ttstack/*"
```

Use the wildcard filter hook **without `quick`**; the native PF implementation
otherwise stops visiting subsequent child anchors. TTstack checks this prerequisite
and does not change the root ruleset. Individual guest rules use `quick`.
Outbound NAT and the remaining firewall policy stay operator-owned.

Each published TCP port has an independent translation/filter anchor. Separate
interface-bound states cover the external and guest-facing legs, preserving
replies to mapped connections when bhyve's deny anchor blocks new routed IPv4
initiation before source NAT. Host-local access remains allowed. This is an
IPv4 egress restriction, not anti-spoofing, peer or IPv6 isolation. Existing
connections can retain their PF states. Create/restart/delete preserve sibling
anchors; deletion removes both translation and filter rules.

When the agent itself runs in a VNET jail, its parent must delegate child-jail
creation and devfs mounting (`children.max`, `allow.mount`, `allow.mount.devfs`,
`enforce_statfs < 2`), and expose the needed PF/TAP/VMM devices. For bhyve this includes
`/dev/vmmctl`, `/dev/vmm/*` and `/dev/vmm.io/*`, as well as the TAP devices;
bhyve additionally requires `allow.vmm`. Nested Jail guests also need permission to mount devfs with
ruleset 4; the agent's VMM/ZFS devices can use a separate devfs mount ruleset.
Use a dedicated test jail; changing a gateway jail's
permissions or firewall is not required by TTstack.

Older Linux-only binaries cannot deserialize bhyve/Jail engine names in persisted
VMs or cached host lists. Upgrade controller and agents together. Before returning
to those binaries, use a compatible FreeBSD-aware version to remove all FreeBSD
workloads and unregister their hosts; also respect the
[database schema gates](deployment.md#resource-update-schema-gate) and retain
consistent state/disk backups. Current Linux binaries retain the shared models
needed to manage FreeBSD agents. Historical reports keep their tested revisions
and scope.

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

The [architecture guide](architecture.md) explains which service owns each
decision. The following limits define the supported product scope.

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
- Optional [disk backup](disk-backup.md) supports stopped QEMU/Firecracker root
  disks on zvol or qualified reflink storage, and experimental bhyve root zvols.
  No full-copy fallback, memory snapshot,
  online backup, container backup, or deleted-VM recovery is provided. Filesystem
  and image eligibility must be checked; a host capability alone does not validate
  every disk image or underlying storage configuration.
- QEMU and Firecracker support stopped-VM CPU/RAM updates and disk growth on file
  and ZFS storage, subject to scoped `vm.resources.offline` admission. QEMU expands
  only the virtual disk; the guest must grow its partitions/filesystems. See the
  [offline resource API](rest-api.md#offline-resource-updates).
- Docker does not support managed SSH keys, disk quotas or outgoing restrictions.
  Firecracker supports creation-time ext4 growth and explicit stopped-VM resource
  updates and initial SSH bootstrap on prepared images, but not interactive consoles
  or resizing a running VM; CPU/RAM changes need a cold boot. See [SSH support and limits](ssh.md).
- Runtime and guest dependencies remain the operator's responsibility. Prefer
  QEMU cloud images for full VMs and prepared long-running Docker images for services.

See the [image guide](guest-images.md) for engine-specific access and storage, and
the [API reference](rest-api.md#lifecycle-and-recovery) for state and recovery semantics.
