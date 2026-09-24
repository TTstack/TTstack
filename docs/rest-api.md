# REST API Reference

All `/api/*` endpoints require `Authorization: Bearer <api-key>` when the
controller is started with `--api-key`. The web dashboard (`/`) is always open.

## Controller Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/` | Web dashboard (no auth required) |
| POST | `/api/hosts` | Register a host |
| GET | `/api/hosts` | List hosts |
| GET | `/api/hosts/{id}` | Host details |
| DELETE | `/api/hosts/{id}` | Remove host |
| POST | `/api/envs` | Create environment |
| GET | `/api/envs` | List environments |
| GET | `/api/envs/{id}` | Environment + VM details |
| DELETE | `/api/envs/{id}` | Destroy environment |
| POST | `/api/envs/{id}/stop` | Stop environment |
| POST | `/api/envs/{id}/start` | Start environment |
| GET | `/api/vms/{id}` | Single VM details |
| GET | `/api/images` | List images across fleet |
| GET | `/api/status` | Fleet-wide resource status |

## Agent Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/info` | Host info and resources |
| GET | `/api/images` | Available images |
| POST | `/api/vms` | Create a VM |
| GET | `/api/vms` | List VMs |
| GET | `/api/vms/{id}` | VM details |
| DELETE | `/api/vms/{id}` | Destroy VM |
| POST | `/api/vms/{id}/stop` | Stop VM |
| POST | `/api/vms/{id}/start` | Start VM |

## Examples

### Register a host

```bash
curl -X POST http://controller:9200/api/hosts \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{"addr": "10.0.0.2:9100"}'
```

### Create an environment

```bash
curl -X POST http://controller:9200/api/envs \
  -H "Authorization: Bearer <key>" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "my-env",
    "owner": "alice",
    "ssh_keys": ["ssh-ed25519 AAAA... alice@laptop"],
    "vms": [
      {
        "image": "alpine-cloud",
        "engine": "qemu",
        "cpu": 2,
        "mem": 2048,
        "disk": 40960,
        "ports": [80],
        "deny_outgoing": false
      }
    ],
    "lifetime": 21600
  }'
```

### Fleet status

```bash
curl -H "Authorization: Bearer <key>" http://controller:9200/api/status
```

## Request / Response Reference

### CreateEnvReq (POST `/api/envs`)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Environment name |
| `owner` | string | no | Owner label, not an authorization boundary |
| `ssh_keys` | string[] | no | Public keys for root access with QEMU cloud-init / experimental Jail; rejected for other engines |
| `vms` | VmSpec[] | yes | List of VM specifications |
| `lifetime` | integer | no | Auto-expiry in seconds (default: 21600 = 6h; 0 = no expiry; no six-hour cap) |

### VmSpec (element of `vms` array)

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `image` | string | yes | Base image name |
| `engine` | string | no | `qemu`, `firecracker`, `docker`, `bhyve`, `jail` (FreeBSD engines are experimental; default: `qemu`) |
| `cpu` | integer | no | vCPUs (default: 2) |
| `mem` | integer | no | Memory in MiB (default: 1024) |
| `disk` | integer | no | QEMU virtual disk in MiB (default: 40960); omit for other engines |
| `ports` | integer[] | no | TCP guest ports to expose; port 22 auto-included for QEMU / Bhyve / Jail only |
| `deny_outgoing` | boolean | no | Block routed outbound initiation while allowing replies; not host/VM isolation; rejected for Docker (default: false) |

### Storage field (agent `/api/info`)

The `storage` field in host info reports the backend type:
- `"file"` — plain qcow2 files, filesystem-agnostic
- `"zvol"` — ZFS zvol raw block devices

## Operation results and recovery

`POST /api/envs` returns **202 Accepted** with an `EnvDetail` after the entire plan
is saved. Poll `GET /api/envs/{id}`: states are `creating`, `active`, `stopped`,
`failed`, and `deleting`. Partial failure remains visible through VM `error`,
environment `error`, and `warnings`; inspect and delete/recreate failed environments.
A duplicate environment name returns 409 and never creates another set of VMs.

`DELETE /api/envs/{id}` is idempotent. Failed agent cleanup returns 502 and retains
its records in `deleting`; background cleanup retries it, or repeat the same DELETE.
Start/stop return errors if any requested operation fails. A transport timeout
never implies the operation was cancelled: inspect the saved environment first.

Agent creation is idempotent for the same VM ID and creation parameters. Reusing
an ID with different parameters is rejected. Agent states are observed periodically;
resource totals include stopped disks and conservatively reserve incomplete cleanup.
List/status endpoints serve snapshots so slow agents do not block the dashboard.
