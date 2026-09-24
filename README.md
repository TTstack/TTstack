# TTstack — Lightweight Private Cloud

[![CI](https://github.com/TTstack/TTstack/actions/workflows/ci.yml/badge.svg)](https://github.com/TTstack/TTstack/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](Cargo.toml)

TTstack manages VMs and containers across a small fleet of hosts for developers
and small teams. Its focus is creating temporary environments, accessing them,
and reliably stopping, restarting and deleting them.

Linux x86_64 is the primary host platform. **FreeBSD support is experimental**
(Bhyve, Jail and PF), outside the primary reliability and CI scope.

## Architecture

```text
tt CLI / Browser → HTTP → tt-ctl → HTTP → tt-agent (one per host)
                          │                 │
                     SQLite state      SQLite state
                                       engines, disks, networking
```

| Component | Responsibility |
|---|---|
| `tt` | CLI client, local image recipes, deployment over SSH |
| `tt-ctl` | Scheduling, environment lifecycle, fleet state and web dashboard |
| `tt-agent` | Host-local VM/container lifecycle, storage and networking |
| `crates/core` | Shared API models, engine, storage and networking implementations |

The Rust workspace uses Tokio/Axum for HTTP and asynchronous coordination, SQLite
for persistent state, and installed hypervisor/container tools to run workloads.
There is one controller; no separate message queue or database server is required.

## Quick start: one Linux/systemd host

Install the [build and QEMU prerequisites](docs/deployment.md#prerequisites) first.
Deployment installs TTstack binaries and services; it does not install engines,
create guest images or register hosts. Run these commands from the repository on
the intended host:

```bash
make release
sudo ./target/release/tt deploy all --release-dir ./target/release
export PATH="/opt/ttstack/bin:$PATH"

# Replace the value with the key printed by the controller deployment.
tt config 127.0.0.1:9200 --api-key 'PASTE_DEPLOYMENT_KEY_HERE'

sudo mkdir -p /home/ttstack/images
sudo /opt/ttstack/bin/tt image create alpine-cloud
tt host add 127.0.0.1:9100

# Use an existing SSH public key; never pass the private key.
tt env create demo --image alpine-cloud --engine qemu \
  --cpu 1 --mem 256 --disk 2048 --ssh-key ~/.ssh/id_ed25519.pub
tt env show demo
```

Use the host and mapped SSH port printed by `env show`, for example
`ssh -i ~/.ssh/id_ed25519 -p 20000 root@127.0.0.1` **if that is the assigned port**.
`running` means the VM process is running; SSH may need more time to start.
These loopback addresses assume the CLI and SSH client are on the same host.
For remote access, register a host address reachable by the controller and use
that host's reachable address for guest connections.

```bash
tt env stop demo
tt env start demo
tt env delete demo
```

For containers or custom guests, see the [image guide](docs/guest-images.md).
For multiple hosts, use [distributed deployment](docs/deployment.md#distributed-deployment).

## Core behavior and boundaries

- Environments group one or more VMs/containers. They expire after six hours by
  default; `--lifetime 0` disables expiry and longer lifetimes are allowed.
- Linux stop/start retains the disk but stops and boots the workload. It does not
  preserve VM memory. Deleting an environment removes its runtime disks/containers.
- Creation is persisted before execution. The CLI waits up to ten minutes; after
  a timeout or interruption, inspect `tt env show NAME` before retrying. Incomplete
  deletion remains visible and is retried while the controller is running.
- Scheduling uses configured CPU, memory and disk reservations, not measured load.
  Limits of 50 hosts and 1000 tracked VMs are guardrails, not tested fleet capacity.
- QEMU cloud images support root SSH key injection and virtual-disk growth.
  Firecracker uses a prepared kernel/rootfs; its built-in recipe only checks boot
  and networking. Docker requires a long-running image default command.
- Firecracker uses jailer and per-VM resource limits, supports opaque read-only
  guest configuration, and requests orderly shutdown before forced termination.
  Linux QEMU/Firecracker can opt into host-enforced guest network isolation. See
  [guest configuration and networking](docs/guest-images.md#firecracker-guest-configuration).
  Identity, application installation and idle-stop policy remain caller responsibilities.
- Image preparation happens on each agent host. There is no automatic image
  distribution, cross-host private network, guest migration or high availability.

Resource and lifecycle details, including failure recovery, are in the
[API reference](docs/rest-api.md#lifecycle-and-recovery). Use `tt --help` and
`tt env create --help` for CLI options; request defaults are listed in the
[API request reference](docs/rest-api.md#environment-requests).

## Access and authentication

Deployment configures one shared administrator API key on the controller and
agents. Manual starts require `--api-key` or `TT_API_KEY`; without one, that
service's API is unauthenticated. Owner names are labels, not access controls.

The dashboard HTML at `http://CONTROLLER:9200/` is public, but its API requests
require the key when authentication is enabled. The browser keeps the entered key
in session storage. Services use HTTP without built-in TLS; keep their listeners
on a trusted network or access them through a protected tunnel/proxy.

Guest access is separate from API authentication: QEMU uses the SSH public keys
supplied at creation; Docker publishes the requested application ports. TCP host
ports are allocated dynamically, so always read the actual mappings from `env show`.

## Documentation

| Document | Scope |
|---|---|
| [Documentation index](docs/README.md) | Reading paths, maintained guides and dated evidence |
| [Deployment](docs/deployment.md) | Dependencies, installation, fleet configuration, upgrades |
| [Guest images](docs/guest-images.md) | Recipes, image formats, guest access, storage and networking |
| [REST API](docs/rest-api.md) | Endpoints, request defaults, response/state semantics and recovery |
| [Compatibility](docs/compatibility.md) | Supported scope, CI and limits of live verification |
| [Linux validation, 2026-09-24](docs/live-validation-2026-09-24.md) | Results for a specific tested code revision |
| [Fleet configuration template](tools/deploy.toml.example) | Commented distributed deployment configuration |
| [Claude workflows](.claude/README.md) | Focused review, checks, commits and lightweight live validation |

`make help` lists development commands. `make doc` generates Rust source API
documentation; the HTTP API is documented in the REST reference above.

## License

[MIT](LICENSE)
