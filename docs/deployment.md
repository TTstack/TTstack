# Deployment guide

TTstack deployment copies prebuilt binaries, configures service keys and starts
services. Engine installation, image preparation and host registration are separate
steps. See the [README quick start](../README.md#quick-start-one-linuxsystemd-host)
for a complete single-host QEMU example.

## Prerequisites

Build on a compatible system with Rust **1.88+** and a C/C++ compiler/linker.
SQLite is bundled. The HTTP client uses reqwest 0.13 with rustls/AWS-LC; OpenSSL
development files are no longer required. AWS-LC and bundled SQLite still compile
native code. `make release` uses the committed `Cargo.lock` with `--locked`. Build
binaries for each target's architecture and libc; copying a glibc binary to a musl
host does not make it compatible.

Linux agents run as root. Install only the dependencies needed for your engine:

| Workflow | Host prerequisites |
|---|---|
| QEMU/KVM | Working `/dev/kvm`, `qemu-system-x86_64`, `qemu-img`, `genisoimage` or `mkisofs` |
| QEMU and Firecracker networking | Full `iproute2`, `nftables`, kernel TUN/TAP support; QEMU also uses `vhost_net` |
| Firecracker | Working `/dev/kvm`, matching `firecracker` and `jailer`, cgroup v2 with CPU/memory/PID controllers, `curl`, compatible kernel/rootfs; `mkfs.ext4` for config drives; `e2fsck` and `resize2fs` for rootfs growth (all from `e2fsprogs`) |
| Docker | Working Docker daemon or Podman runtime; the agent selects Docker when its binary is installed |
| QEMU/Firecracker image recipes | `curl`; Firecracker additionally uses `dd`, `mkfs.ext4`, loop mount/unmount and `tar` |
| Zvol storage | Existing ZFS pool/datasets and `zfs`; provision these manually |

Docker uses its own networking and does not need TTstack's TAP/nftables setup.
Engine detection is not a complete health check: permissions, daemon availability
and guest compatibility still matter. Avoid installing a broken Docker binary next
to a working Podman installation; the agent will select Docker.

New Firecracker processes always run through jailer; there is no silent unjailed
fallback. Existing unjailed processes remain queryable/stoppable during upgrade;
their next cold start needs the new prerequisites. The jailer uses a chroot under
`runtime_dir/.jailer`, a per-VM UID/GID in **100000–165535** derived from the allocated
guest IP, and cgroups under `/sys/fs/cgroup/ttstack`. Reserve this UID/GID range
from host accounts and other services. Keep runtime storage private. File root
disks use persistent hard links on the same filesystem as the jail. Zvol roots
use private block-device nodes; read-only kernel/config files may be copied
across datasets. See the [storage layouts](guest-images.md#storage).
Avoid long runtime paths because Unix sockets have a 108-byte limit.

Each VMM has a CPU quota of its vCPU count, a memory ceiling of guest RAM + 128 MiB,
swap disabled, and a bounded thread count. Scheduling reserves the same memory
headroom. Leave capacity for the host OS and agent in the configured host budgets.
Jailer cgroups are separate from the agent service's cgroup; limiting only the
agent service does not limit guest processes. Use `isolated_network` for guest
network isolation; jailer alone does not provide that policy. Upgrade controller
and agents together before requesting the new capabilities.

The controller must reach every registered agent address. Clients need access to
the controller, and guest connections need access to each host's published TCP
ports (allocated from 20000–65535). Arrange host/provider firewalls accordingly.
Service authentication does not encrypt HTTP; see [access and authentication](../README.md#access-and-authentication).

## Local deployment

Local automated deployment requires **Linux with systemd** and root. From the
repository after building:

```bash
sudo ./target/release/tt deploy all --release-dir ./target/release
# Alternatively deploy only one role:
# sudo ./target/release/tt deploy agent --release-dir ./target/release
# sudo ./target/release/tt deploy ctl --release-dir ./target/release
export PATH="/opt/ttstack/bin:$PATH"
```

Local deployment uses fixed defaults: prefix `/opt/ttstack`, service user `ttstack`
for the controller, and root for the agent. Agent-only deployment installs
`tt-agent`; controller deployment installs `tt-ctl` and `tt`. `deploy all` installs
all three. No symlink or PATH update is installed. With `sudo`, use the full binary
path if `/opt/ttstack/bin` is not in sudo's PATH.

The key is reused from `/opt/ttstack/etc/api-key` (mode 0600). On first deployment,
an existing service key is imported before a new key is generated. Conflicting
legacy service keys cause an error instead of an automatic rotation. Controller
deployment prints the key; an agent-only deployment stores it but does not print it.

```bash
/opt/ttstack/bin/tt config 127.0.0.1:9200 --api-key 'PASTE_DEPLOYMENT_KEY_HERE'
/opt/ttstack/bin/tt host add 127.0.0.1:9100
```

`make install PREFIX=...` copies binaries only; it does not configure services.
`PREFIX` applies to Makefile install/uninstall, not to `tt deploy` or `make deploy`.

## Distributed deployment

Use one fleet configuration to keep the controller and agents on the same API key.
Separate local deployments on different machines otherwise generate different keys.
The deploying machine needs `ssh`, `scp`, prebuilt binaries and SSH access. Remote
hosts need `sudo` even when connecting as root, with privileges usable without an
interactive password prompt. SSH accepts new host keys and rejects changed keys.

```bash
cp tools/deploy.toml.example deploy.toml
# Edit host addresses, paths and resource budgets before running.
./target/release/tt deploy dist deploy.toml
```

The [configuration template](../tools/deploy.toml.example) documents every supported
field. `[general]`, `[controller]` and `[[agents]]` are optional sections; provide
at least one role to deploy. Each included controller/agent needs `host`.

- `[general].release_dir` and per-agent `release_dir` refer to **local** binary
  directories on the deploying machine. Other configured paths refer to target hosts.
- CPU and memory `0` mean auto-detect. Memory is an integer in MiB. `disk_total`
  is a **TOML string**: `"204800"` and `"200G"` both mean 200 GiB; unquoted `204800`
  is invalid. Disk must be positive and is a scheduling budget, not a filesystem quota.
- Leave CPU/memory capacity for the host OS and other services. Auto-detection uses
  the host totals, not unused capacity. Disk is never auto-detected.
- The controller runs as `[general].user`; agents run as root. Default data/image
  paths follow `/home/USER`. If setting a custom controller `data_dir`, create it
  with ownership for that user before deployment.
- File storage uses filesystem directories. Zvol storage uses ZFS dataset names;
  deployment does not create pools, datasets or base image volumes.
- `host_id` is generated once and persisted in the agent database when omitted.
  Keep IDs unique and stable; do not change one while it has tracked VMs.

Without an explicit `[general].api_key`, deployment reuses `<config-file>.api-key`
(mode 0600). On first use it imports the configured controller's existing key, or
the first agent's if there is no controller, before generating a new one. Preserve
this sidecar with your configuration and keep it out of version control. Explicit
keys must use only letters, digits, `-`, `_` and `.`.

Distributed deployment detects systemd or OpenRC. OpenRC and musl targets are
implemented but not covered by the current live validation. The fallback on hosts
with neither init system is an unmanaged background process, not a persistent
service setup. Deployment targets must be Linux hosts.

Deployment does **not** register agents or distribute images. Configure the CLI
with the printed controller address/key, prepare images on the relevant hosts,
then register each agent, for example:

```bash
./target/release/tt config 10.0.0.1:9200 --api-key 'PASTE_DEPLOYMENT_KEY_HERE'
./target/release/tt host add 10.0.0.2:9100
./target/release/tt host list
```

## Paths and manual configuration

| Default path | Contents |
|---|---|
| `/opt/ttstack/bin/` | Binaries for the deployed role |
| `/opt/ttstack/etc/` | `api-key` for local deployment; `tt-agent.env` / `tt-ctl.env` service keys |
| `/home/ttstack/images/` | Base files/directories for file storage; excludes Docker images |
| `/home/ttstack/runtime/` | Per-VM clones, retained through stop/start until deletion |
| `/home/ttstack/data/agent.db` | Agent state and persistent host identity |
| `/home/ttstack/ctl/ctl.db` | Controller state |
| `/home/ttstack/run/` | Engine logs, PID files, sockets, cloud-init seeds; fixed independently of deployment user |
| `~/.ttconfig` | CLI controller address and optional API key |

Direct `tt-agent`/`tt-ctl` starts do not load the deployment environment files
automatically. Set `TT_API_KEY` or pass `--api-key` to both services. Run each
binary with `--help` for the full options. Agent defaults are file storage,
`0.0.0.0:9100`, auto-detected CPU/memory, and a 204800 MiB disk budget. The controller
listens on `0.0.0.0:9200`. Unlike TOML, the agent's `--disk-total` takes an integer
in MiB, without a `G` suffix. Distributed configuration does not expose an agent
`data_dir` override; it uses `/home/USER/data`.

## Upgrades and recovery

Rebuild and repeat the same deployment command to replace binaries and restart
services while keeping data and keys. Upgrade the controller and agents together.
Schema changes are applied on startup; older binaries reject a newer schema.
For rollback, retain matching binaries and consistent backups of controller/agent
databases and guest storage, taken while services/workloads are stopped.

Live Linux guests survived agent service restarts in the systemd validation.
This is not a promise about host reboots: TTstack does not automatically restart
guests after a host reboot. Inspect their observed state and start stopped guests.
See [lifecycle and recovery](rest-api.md#lifecycle-and-recovery) for timeouts,
failed operations and expiry cleanup, and [compatibility](compatibility.md) for
tested combinations.

## Upgrade compatibility

Before upgrading, back up controller and agent SQLite state together with retained
VM disks. All tracked engines must be among the current `qemu`, `firecracker`,
and `docker` values, including cached host engine lists. Unknown engine values
are rejected, never mapped to another backend or silently deleted. If old state
contains an unsupported engine, use its compatible prior release to drain/remove
those workloads and unregister their hosts before upgrading. Do not edit raw state
to pretend that an existing workload uses a different engine.

The retained engine names and state schema are unchanged. Ordinary Linux
workspaces keep their identities, disks and lifecycle state through an upgrade.
