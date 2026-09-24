# Compatibility and validation scope

Linux x86_64 is the primary host implementation and CI target. **All FreeBSD
support is experimental**, including Bhyve, Jail, PF networking and manual service
setup. It is outside the primary reliability/fix scope and has no guarantee of
complete lifecycle or guest compatibility.

## Implementation versus live verification

| Workflow | Implementation | Current live evidence |
|---|---|---|
| Linux QEMU/KVM + file storage | Full VMs, cloud-init, TCP forwarding, disk growth | Alpine 3.21.7 and Debian 13 guests on Ubuntu 24.04.4 hosts |
| Linux Docker | Container lifecycle and native port publishing | Temporary HTTP workload on two Ubuntu 24.04.4 hosts |
| Linux Firecracker + file storage | Prepared kernel/rootfs, TAP networking | Built-in idle Alpine guest on Ubuntu 24.04.4 |
| Podman | Alternate runtime selected when Docker binary is absent | Not covered by this run |
| QEMU + zvol | Raw ZFS volumes and snapshot clones | Not covered by this run |
| Ubuntu cloud guest | Built-in QEMU recipe | Not covered by this run |
| Linux/systemd deployment | Local and distributed service generation | Temporary systemd services exercised; not every deploy configuration |
| Linux/OpenRC, musl binaries | Distributed deployment support | Not covered by this run |
| FreeBSD Bhyve/Jail/PF | Experimental, manual setup; Bhyve rejects in-place restart | Not covered by this run |
| Other host platforms | No validated agent deployment path | No support commitment |

The [2026-09-24 validation record](live-validation-2026-09-24.md) identifies the
tested revision, load limits, corrections and results. Its evidence is limited to
those combinations. Unit tests, engine detection and an image recipe's presence
are not substitutes for guest boot and access tests.

## Build and CI

The workspace uses Rust **1.88+**, edition 2024, with committed `Cargo.lock` and
locked builds. CI checks the minimum toolchain separately from stable and runs
formatting, Clippy, tests and a release build. SQLite is bundled; HTTP client TLS
uses rustls/AWS-LC rather than OpenSSL. Native dependencies still need a C/C++
compiler. See [deployment prerequisites](deployment.md#prerequisites).

Tests cover local logic and mock-agent HTTP workflows: interrupted creation,
partial deletion, expiry retry, failed stop, duplicate creation, input validation,
resource accounting and port reuse. They do not boot guests or modify host firewall
rules. `make doc` generates Rust API documentation, not the HTTP reference.

For changes affecting live behavior, validate the affected engine's create/access,
stop/start with retained data, agent/controller restart and final cleanup on a
suitable host. Host reboot and ZFS-backed storage still require separate validation;
neither was tested in the current record. No performance or maximum-capacity test
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
  networking or tenant isolation. The API key is a shared administrator credential.
- No automatic guest restart after host reboot, migration, HA, distributed storage,
  image distribution or application/database provisioning.
- Docker does not support managed SSH keys, disk quotas or outgoing restrictions.
  Firecracker does not support managed SSH keys, interactive consoles or disk resizing.
- Runtime and guest dependencies remain the operator's responsibility. Prefer
  QEMU cloud images for full VMs and prepared long-running Docker images for services.

See the [image guide](guest-images.md) for engine-specific access and storage, and
the [API reference](rest-api.md#lifecycle-and-recovery) for state and recovery semantics.
