# Compatibility and validation scope

Linux is the primary supported host platform. **All FreeBSD host support is
experimental**, including Bhyve, Jail, PF networking and platform-specific deployment.
Experimental support is not a promise of complete lifecycle or guest compatibility.

| Host | Status | Engines | Storage |
|---|---|---|---|
| Linux x86_64 | Primary implementation and CI target | QEMU/KVM, Docker/Podman, Firecracker | file; QEMU also supports zvol |
| FreeBSD | Experimental; manual setup | Bhyve, Jail | Experimental file/zvol combinations |
| Other hosts | No validated agent deployment path | No support commitment | — |

QEMU cloud images are the recommended full-VM workflow. Firecracker requires a
prepared Linux kernel and ext4 rootfs; it does not provide QEMU's cloud-init/SSH
workflow. Docker delegates images and networking to the container runtime.

## Host dependencies

- QEMU: `/dev/kvm`, `qemu-system-x86_64`, `qemu-img`, `genisoimage` or `mkisofs`.
- Host-managed networking: full `iproute2`, `nftables`, TAP support and root access.
- Firecracker: `/dev/kvm`, `firecracker`, `curl`, and a compatible guest kernel/rootfs.
- Docker: a working Docker daemon or Podman runtime; Docker is preferred if installed.
- Zvol: an existing ZFS pool and the `zfs` command; configure dataset names rather
  than filesystem directories. Firecracker and Jail do not use this backend.
- Local automated deployment: Linux/systemd. Distributed deployment supports
  Linux/systemd and OpenRC. FreeBSD requires manual service setup.

## Build and CI

Rust **1.88+**, edition 2024. `Cargo.lock` is versioned and CI builds with `--locked`.
The minimum toolchain is checked separately from stable. SQLite is bundled;
`reqwest` uses native TLS, so builds need the platform's OpenSSL development files.

CI runs unit tests and local mock-agent HTTP tests for interrupted creation,
partial deletion, expiry retry, failed stop, duplicate creation, input validation,
resource accounting and port reuse. These tests do not boot real guests or modify
host firewall rules.

Before a release, validate on a dedicated Linux host: QEMU cloud-image SSH access,
stop/start with disk preservation, host reboot and agent restart, Docker lifecycle,
Firecracker rootfs/network startup, and nftables cleanup. Zvol needs a separate
ZFS-backed host check. No live-host verification is implied by passing unit tests.

## Validation records

The [2026-09-24 Linux validation record](live-validation-2026-09-24.md) covers
low-load Docker, QEMU and Firecracker workflows on three hosts, including failures
found and corrected during that run. Its untested combinations are listed explicitly.

Earlier project documentation recorded manual work on Debian 13, Alpine 3.23 and
FreeBSD 14.3. Those reports describe earlier revisions; they are not current
release verification. Bhyve's implementation still rejects in-place restart, and
Jail/PF behavior has not been brought into the Linux reliability scope.

## Limits

- Single controller; up to 50 registered hosts and 1000 tracked VMs. These are
  configured limits, not benchmark results.
- Mutations are serialized for predictable accounting. Reads serve persisted
  snapshots and remain available during long operations.
- Environments group lifecycle operations, not cross-host networking or tenant
  isolation. The API key is a shared administrator credential; owner is a label.
- No automatic guest restart after a host reboot. No migration, HA, distributed
  storage, image distribution, or application/database provisioning is promised.
