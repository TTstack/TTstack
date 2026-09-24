# Compatibility and validation scope

Linux x86_64 is the supported host implementation and CI target, with QEMU/KVM,
Firecracker and Docker/Podman. The agent also includes experimental FreeBSD
Bhyve/Jail paths; this is outside the Linux support commitment and CI scope.

## Implementation versus live verification

| Workflow | Implementation | Current live evidence |
|---|---|---|
| Linux QEMU/KVM + file storage | Full VMs, cloud-init, TCP forwarding, disk growth | Alpine 3.21.7 and Debian 13 guests on Ubuntu 24.04.4 hosts |
| Linux Docker | Container lifecycle and native port publishing | Temporary HTTP workload on two Ubuntu 24.04.4 hosts |
| Linux Firecracker + file storage | Jailer, config drive, orderly shutdown, opt-in network isolation | Alpine fixture: config/read-only access, retained data, isolation, restart and cleanup on Ubuntu 24.04 |
| Linux Firecracker + zvol | Snapshot clones, jailed block device, ext4 growth, retained disks | [Dedicated ZFS host validation](firecracker-zvol-validation-2026-09-24.md) |
| Podman | Alternate runtime selected when Docker binary is absent | Not covered by this run |
| QEMU + zvol | Raw ZFS volumes and snapshot clones | Not covered by this run |
| Ubuntu cloud guest | Built-in QEMU recipe | Not covered by this run |
| Linux/systemd deployment | Local and distributed service generation | Temporary systemd services exercised; not every deploy configuration |
| Linux/OpenRC, musl binaries | Distributed deployment support | Not covered by this run |
| FreeBSD Bhyve/Jail/PF | Restored experimental implementation, manual setup | No new native build or live verification; see [limits below](#experimental-freebsd-restoration) |
| Other host platforms | No validated agent deployment path | No support commitment |

The [2026-09-24 validation record](live-validation-2026-09-24.md) identifies the
tested revision, load limits, corrections and results. Its evidence is limited to
those combinations. Unit tests, engine detection and an image recipe's presence
are not substitutes for guest boot and access tests.

The [Firecracker follow-up record](firecracker-validation-2026-09-24.md) covers
configuration drives, jailed execution, shutdown fallback and isolation. Its stated
limits include QEMU isolation and legacy unjailed-VMM upgrades, which remain untested
in that run.

The [Linux host upgrade record](linux-host-upgrade-validation-2026-09-24.md)
checks the retained engines across an agent/controller binary upgrade, new
provisioning, stop/start, access, persistence and cleanup. Refer to that record
for its exact fixture and validation limits.

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

Linux tests also need `mkfs.ext4` and `debugfs` from `e2fsprogs` to inspect a real
configuration disk without mounting it or requiring root. CI installs these tools.
Tests cover local logic and mock-agent HTTP workflows: interrupted creation,
partial deletion, expiry retry, failed stop, duplicate creation, input validation,
resource accounting and port reuse. They do not boot guests or modify host firewall
rules. `make doc` generates Rust API documentation, not the HTTP reference.

For changes affecting live behavior, validate the affected engine's create/access,
stop/start with retained data, agent/controller restart and final cleanup on a
suitable host. Host reboot on ZFS file datasets has a separate
[restart record](firecracker-restart-validation-2026-09-24.md); zvol coverage and
its limits are recorded in the [zvol validation](firecracker-zvol-validation-2026-09-24.md). No performance or maximum-capacity test
has been performed by that run.

## SQLite engine assessment — 2026-09-24

TTstack retains upstream SQLite through bundled `rusqlite`. The dependency refresh
updates all 12 direct registry dependencies to the latest non-yanked stable releases
and refreshes the lockfile within upstream constraints. In particular, Axum 0.8.9
requires exactly `matchit = 0.8.4`; forcing a newer release would require changing
that upstream dependency. `Cargo.toml` and `Cargo.lock` record the selected versions.

[Turso 0.7.2](https://github.com/tursodatabase/turso/tree/v0.7.2) is a Rust SQLite
reimplementation with reported production users, so it is a credible candidate.
Its release documentation still marks multi-process WAL as experimental and excludes
mixed SQLite/Turso multi-process access from its
[compatibility guarantees](https://github.com/tursodatabase/turso/blob/v0.7.2/COMPAT.md).
[prsqlite](https://github.com/kawasin73/prsqlite) describes itself as an unstable
work-in-progress. [libSQL](https://github.com/tursodatabase/libsql) is a SQLite fork,
not a pure Rust replacement for its engine.

An isolated local probe of Turso 0.7.2's Rust API passed independent-reader isolation,
dropped-transaction rollback and write rejection with `query_only=1`. It also found
that `execute_batch("PRAGMA journal_mode=WAL")` returns an unexpected-row error and
`PRAGMA query_only=ON` is rejected while `query_only=1` works. These are API/SQL
compatibility differences, not evidence of data loss; a passing smoke test is also
not a durability certification. The engine can be used with adaptations, but our
assessment is that switching TTstack's recovery-critical state store now adds more
compatibility and operational work than it removes. No alternate database backend
or migration path is introduced by this dependency refresh.

Validation of the retained backend passed 118 workspace tests, Clippy, Rust 1.88
checks and a locked release build. A local process-level upgrade check used the
previous controller binary to create state, killed it with an outstanding WAL,
then verified recovery, authenticated API/CLI access, writes and another restart
with the new binary. SQLite integrity checking and reopening with the previous
binary also passed. This check did not boot guests or exercise the remote hosts.

## Product boundaries

- One controller, with configured limits of 50 hosts and 1000 tracked VMs. These
  are guardrails, not benchmarked capacity. Mutations are serialized and reads
  generally serve cached/persisted state.
- Environments group lifecycle operations; they do not provide cross-host private
  networking or tenant authorization. Linux QEMU/Firecracker offer opt-in guest
  network isolation; Firecracker also uses jailer and resource limits. The API key
  remains a shared administrator credential. User identity, entitlements, application
  installation, connection leases and idle-stop policy belong in callers.
- No automatic guest restart after host reboot, migration, HA, distributed storage,
  image distribution or application/database provisioning.
- Docker does not support managed SSH keys, disk quotas or outgoing restrictions.
  Firecracker supports creation-time ext4 growth, but not managed SSH keys,
  interactive consoles or resizing existing VMs.
- Runtime and guest dependencies remain the operator's responsibility. Prefer
  QEMU cloud images for full VMs and prepared long-running Docker images for services.

See the [image guide](guest-images.md) for engine-specific access and storage, and
the [API reference](rest-api.md#lifecycle-and-recovery) for state and recovery semantics.
