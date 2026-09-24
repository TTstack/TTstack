# Deployment Guide

Deployment is built into the `tt` CLI binary — no external scripts needed.

## Prerequisites

**Linux agents**:
- nftables
- Kernel modules: `tun`, `vhost_net`, `kvm_intel` (or `kvm_amd`)
- QEMU: `qemu-system-x86_64`, `qemu-img` (monitor communication uses a Unix socket)
- `genisoimage` or `mkisofs` (for cloud-init seed ISO generation)

**FreeBSD agents (experimental; manual setup, outside the main validation scope)**:
- PF enabled

## Local Deploy

Local deployment supports Linux with systemd. Use distributed deployment for OpenRC.

```bash
# Deploy both agent and controller (auto-generates API key):
sudo tt deploy all

# Or deploy separately:
sudo tt deploy agent
sudo tt deploy ctl

# Specify a custom binary directory:
sudo tt deploy all --release-dir ./target/release
```

Local deployment reuses `/opt/ttstack/etc/api-key`, migrating an existing service
key on first use. Separate agent/controller deployments and upgrades share the
same key. Conflicting legacy service keys are reported instead of silently rotated.
The controller API key is printed on completion. Save it for CLI configuration:

```bash
tt config 127.0.0.1:9200 --api-key <printed-api-key>
```

## Distributed Deploy

For multi-host fleets, create a `deploy.toml` config:

```bash
cp tools/deploy.toml.example deploy.toml
# Edit with your fleet IPs...
tt deploy dist deploy.toml
```

### deploy.toml reference

```toml
[general]
prefix      = "/opt/ttstack"       # Install path on all hosts
user        = "ttstack"            # Runtime user (created if absent)
release_dir = "./target/release"   # Local path to compiled binaries
# api_key   = "my-secret-key"     # Optional; auto-generated if omitted

[controller]
host     = "10.0.0.1"             # Controller IP or hostname
# ssh_user = "root"               # SSH user (default: root)
# ssh_port = 22                   # SSH port (default: 22)
# listen   = "0.0.0.0:9200"      # Listen address (default: 0.0.0.0:9200)
# data_dir = "/home/ttstack/ctl"  # Database directory

[[agents]]
host = "10.0.0.2"                 # Agent IP or hostname
# ssh_user = "root"
# ssh_port = 22
# listen = "0.0.0.0:9100"
# storage = "file"                 # "file" or "zvol"
# image_dir = "/home/ttstack/images"
# runtime_dir = "/home/ttstack/runtime"
# cpu_total = 0                   # 0 = auto-detect
# mem_total = 0                   # MiB, 0 = auto-detect
# disk_total = "200G"             # MiB or "NNNG" shorthand
# host_id = "my-node"             # Custom host ID
# release_dir = "./target/release-musl"  # Per-agent binary path override

[[agents]]
host        = "10.0.0.3"
storage     = "zvol"
image_dir   = "tank/ttstack/images"
runtime_dir = "tank/ttstack/runtime"
cpu_total   = 32
mem_total   = 65536               # 64 GiB
disk_total  = "1000G"             # ~1 TiB
```

### Cross-platform notes

- **Alpine Linux**: Use `release_dir` per-agent to point to musl-compiled binaries
- **Systemd** (Debian/Ubuntu/Rocky): Auto-detected; creates systemd units
- **OpenRC** (Alpine): Auto-detected; creates `/etc/init.d/` scripts
- **FreeBSD**: Experimental; install and configure services manually. The automated deploy path targets Linux.

Without an explicit `[general].api_key`, distributed deployment reuses
`<config-file>.api-key` (mode 0600). On first use it imports the existing key from
the configured controller, or the first agent if no controller is listed, before
generating a new key. Keep this sidecar with your deployment configuration and
exclude it from version control. SSH accepts new host keys and rejects changed keys.

## Directory Layout

```
/opt/ttstack/bin/          # binaries (tt, tt-ctl, tt-agent)
/home/ttstack/             # runtime data (dedicated ttstack user)
  ├── images/              # base VM/container images
  ├── runtime/             # transient VM image clones
  ├── data/                # agent SQLite database
  ├── ctl/                 # controller SQLite database
  └── run/                 # PID files, sockets, seed ISOs
```

## Idempotent Upgrades

Deploy is idempotent — re-running copies new binaries, restarts services,
and preserves data and keys. Schema migrations run automatically on startup.
Upgrade controller and agents together. Schema v2 adds durable lifecycle state;
older binaries refuse to open a newer schema. Back up the SQLite databases before
upgrading if rollback is required. Running Linux guests survive an agent service
restart; after a host reboot, inspect their observed state and start stopped guests.
TTstack does not automatically restart guests after a host reboot.

```bash
# Rebuild and re-deploy:
make release
tt deploy dist deploy.toml
```

## Agent Configuration

Memory and disk values are in **MiB**. CPU and memory accept 0 for auto-detection;
`--disk-total` is an explicit scheduling budget, not physical free-space detection.
Set CPU/memory budgets below host totals to leave room for the host OS.

```
tt-agent [OPTIONS]

  --listen <ADDR>         Listen address              [0.0.0.0:9100]
  --image-dir <PATH>      Base image directory         [/home/ttstack/images]
  --runtime-dir <PATH>    Runtime clone directory      [/home/ttstack/runtime]
  --data-dir <PATH>       Database directory            [/home/ttstack/data]
  --storage <TYPE>        zvol | file                  [file]
  --cpu-total <N>         CPU cores (0=auto)           [0]
  --mem-total <MiB>       Memory in MiB (0=auto)       [0]
  --disk-total <MiB>      Disk in MiB                  [204800 (~200 GiB)]
  --host-id <ID>          Host ID (auto-generated)
```

## Controller Configuration

```
tt-ctl [OPTIONS]

  --listen <ADDR>       Listen address              [0.0.0.0:9200]
  --data-dir <PATH>     Database directory            [/home/ttstack/ctl]
  --api-key <KEY>       API key for auth (env: TT_API_KEY)  [none]
```
