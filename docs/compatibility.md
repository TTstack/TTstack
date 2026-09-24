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
formatting, Clippy, tests and a release build. SQLite is bundled; native TLS needs
the platform's OpenSSL development files. See [deployment prerequisites](deployment.md#prerequisites).

Tests cover local logic and mock-agent HTTP workflows: interrupted creation,
partial deletion, expiry retry, failed stop, duplicate creation, input validation,
resource accounting and port reuse. They do not boot guests or modify host firewall
rules. `make doc` generates Rust API documentation, not the HTTP reference.

For changes affecting live behavior, validate the affected engine's create/access,
stop/start with retained data, agent/controller restart and final cleanup on a
suitable host. Host reboot and ZFS-backed storage still require separate validation;
neither was tested in the current record. No performance or maximum-capacity test
has been performed by that run.

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
