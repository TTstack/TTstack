# REST API reference

The controller listens on port 9200 and agents on 9100 by default. Both use HTTP.
Each service requires `Authorization: Bearer KEY` on `/api/*` when configured with
`--api-key` or `TT_API_KEY`; without a key, its API is unauthenticated. Deployment
sets the same administrator key on the controller and agents. The dashboard HTML
at `/` is public; its API calls still require authentication. There is no per-owner
or per-environment authorization. See [deployment](deployment.md).

## Response format

Application handlers return an envelope:

```json
{"ok": true, "data": {"name": "example payload"}}
```

```json
{"ok": false, "error": "operation failed"}
```

The first payload is illustrative; actual types are listed below. Successful
environment actions return `data: null`; host removal and agent actions omit `data`.
Clients should check both the HTTP status and `ok`. Framework errors such as
malformed JSON may have non-envelope bodies; a JSON envelope is not guaranteed
for every failure. A transport timeout does not mean an operation was cancelled.

## Controller endpoints

| Method | Path | Success payload / behavior |
|---|---|---|
| GET | `/` | Dashboard HTML |
| POST | `/api/hosts` | `Host`, HTTP 201; body `{"addr":"10.0.0.2:9100"}` |
| GET | `/api/hosts` | `Host[]` |
| GET | `/api/hosts/{id}` | `Host` |
| DELETE | `/api/hosts/{id}` | No payload; refuses removal while any VM is tracked on it |
| POST | `/api/envs` | `EnvDetail`, HTTP 202 after persisting the creation plan |
| GET | `/api/envs` | `Env[]` |
| GET | `/api/envs/{id}` | `EnvDetail` |
| DELETE | `/api/envs/{id}` | No payload; idempotent, including an already absent environment |
| POST | `/api/envs/{id}/stop` | No payload |
| POST | `/api/envs/{id}/start` | No payload |
| GET | `/api/vms/{id}` | `Vm` |
| GET | `/api/images` | `ImageInfo[]`: `name`, `host_id`; cached file/zvol images on online hosts |
| GET | `/api/status` | `FleetStatus`: host/environment/VM counts and resource reservations |

Host addresses must be `host:port`, without a URL scheme or path. Registration
contacts the agent; there is no automatic host discovery. Mutations are serialized;
conflicting requests can return 409. GET of a missing host, environment or VM
returns 404. Invalid environment parameters return 400, duplicate names 409, and
unschedulable requests 422. Agent operation failures may surface as 502.

## Environment requests

`POST /api/envs` accepts:

| Field | Type | Default / meaning |
|---|---|---|
| `id` | string | Required unique environment name |
| `owner` | string | Empty label by default; the CLI defaults this to `$USER` |
| `vms` | `VmSpec[]` | Required, nonempty list |
| `ssh_keys` | string[] | Empty; public keys applied to every VM, subject to engine support |
| `lifetime` | integer | Seconds from creation; default 21600, `0` disables expiry; longer values allowed |

Each `VmSpec` accepts:

| Field | Type | Default / meaning |
|---|---|---|
| `image` | string | Required; file/zvol image name or container image reference |
| `engine` | string | `qemu` by default; JSON values: `qemu`, `firecracker`, `docker`, `bhyve`, `jail` |
| `cpu` | integer | Positive vCPU count; default 2 |
| `mem` | integer | Positive memory in MiB; default 1024 |
| `disk` | integer | QEMU virtual disk size in MiB; default 40960. Omit for other engines |
| `ports` | integer[] | TCP guest ports to expose; default empty; port 22 is added for QEMU/Bhyve/Jail |
| `deny_outgoing` | boolean | Default false; block routed outgoing initiation, not host/guest isolation; rejected for Docker |
| `isolated_network` | boolean | Default false; Linux QEMU/Firecracker only; block peers, guest-initiated host access, private/link-local destinations and IPv6; allow public IPv4 egress and replies to inbound connections |
| `guest_config` | object | Default `{}`; Firecracker only; up to 32 simple file names mapped to UTF-8 strings, 64 KiB total names/content; attached as a read-only config drive |
| `ssh_keys` | string[] | Empty; merged with environment keys; QEMU cloud-init / experimental Jail only |

CLI engine aliases such as `kvm`, `fc` and `podman` are not JSON enum values.
Bhyve and Jail are experimental. SSH keys are complete public-key strings in JSON,
not local file paths. If an environment mixes engines, use per-VM keys only for
engines that support injection. Other engine-specific requirements are in the
[image guide](guest-images.md).

This example assumes a registered Docker host and registry access (or a cached
`nginx:alpine` image). Replace the controller address and key:

```bash
TT_CONTROLLER='http://127.0.0.1:9200'
TT_KEY='PASTE_DEPLOYMENT_KEY_HERE'
curl --fail-with-body -X POST "$TT_CONTROLLER/api/envs" \
  -H "Authorization: Bearer $TT_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"id":"web","owner":"alice","vms":[{"image":"nginx:alpine","engine":"docker","cpu":1,"mem":128,"ports":[80]}],"lifetime":21600}'

curl --fail-with-body -H "Authorization: Bearer $TT_KEY" \
  "$TT_CONTROLLER/api/envs/web"
```

HTTP 202 acknowledges a saved plan, not a running application. Poll the second
endpoint until `env.state` leaves `creating`, then inspect the state and errors.

## Returned state and resources

`EnvDetail` contains `env`, `vms` and optional `warnings`. `Env` contains `id`,
`owner`, `vm_ids`, `created_at`, `expires_at`, `state` and nullable `error`.
Timestamps are Unix seconds; `expires_at: 0` means no expiry.

Each `Vm` includes its `id`, `env_id`, `host_id`, image/engine, `cpu`, `mem`, `disk`,
internal `ip`, `port_map`, saved creation `options`, `state`, nullable `error` and
`created_at`. For example, `"port_map":{"22":20000}` means host TCP port 20000
forwards to guest port 22. JSON object keys are strings. Always use the assigned
port on the relevant agent host; guest IPs need not be reachable from the client.

Resource `*_total` fields are configured scheduling capacities; `*_used` fields
are **reservations**, not measured CPU load, RAM use or physical filesystem usage.
Stopped guests release CPU/memory reservations and retain disk reservations. Failed
or incomplete operations conservatively retain resources until cleanup. `vm_count`
includes stopped/failed/deleting records. Docker disk usage is not accounted or
quota-enforced; Firecracker reserves its existing rootfs size plus 4 MiB when a
configuration drive is present. Firecracker memory reservations include 128 MiB
of VMM headroom in addition to the guest's `mem`; stopped guests release both.

Agent `/api/info` returns `host_id`, `resource`, `engines`, `storage`, `images` and
`capabilities`. Linux agents advertise `guest_config`, `isolated_network` and
`firecracker_jailer`; hosts retain these fields. The controller rejects placement
on older agents that do not advertise the required capabilities. Upgrade agents
before requesting these features. Reported capabilities describe implementation
support, not a substitute for host prerequisites or application readiness checks.

`Vm.options` contains `isolated_network` and optional `guest_config_digest` (SHA-256),
never configuration contents. Configuration is immutable for that VM and retained
across stop/start. TTstack treats it as opaque data, not shell commands or a
cloud-init document. Use a guest application protocol for live credential renewal.
Protect create requests with a private network or TLS because they can carry secrets.
The [guest configuration contract](guest-images.md#firecracker-guest-configuration)
describes mounting, limits and ownership.
`storage: "file"` covers files/directories; `"zvol"` means ZFS raw volumes. Docker's
image store is independent. Full serialized models are in
[model.rs](../crates/core/src/model.rs) and [api.rs](../crates/core/src/api.rs).

## Lifecycle and recovery

Environment states are `creating`, `active`, `stopped`, `failed` and `deleting`.
An environment is active when all its VMs are running without errors, and stopped
when all are stopped without errors. Mixed, paused or failed VM states can make
an environment `failed`. VM states are `creating`, `running`, `stopped`, `paused`,
`failed` and `deleting`. `running` checks the process/container, not guest SSH or
application readiness. An offline agent makes the environment failed while its VM
states remain the last observed snapshots; it does not prove the guests stopped.

Agent reconciliation and controller refresh each normally run on a 15-second loop.
Busy operations and unreachable hosts can delay them; reads are snapshots, not
immediate engine probes, and end-to-end freshness is not guaranteed within 15 seconds.

- **Create:** the plan is saved before work starts. The CLI polls for up to ten
  minutes. If the client times out, inspect the existing name before retrying.
  Partial failures remain visible in `env.error`, VM `error` and `warnings`; delete
  and recreate failed environments after correcting the cause. A controller crash
  can leave creation failed; there is no automatic re-creation of failed workloads.
- **Stop/start:** on Linux, stop releases execution resources while preserving
  disks/containers; start boots/restarts them. VM memory is not retained. QEMU tries
  guest shutdown before termination. Firecracker on x86_64 sends `SendCtrlAltDel`,
  waits up to 30 seconds, then terminates on failure/timeout and logs the fallback.
  The image must support orderly shutdown; a successful stop alone does not prove
  that the guest flushed its data. Stopping a paused Firecracker first resumes it.
  Save work before stopping. Partial failures are reported instead of hidden.
- **Delete:** failed cleanup returns an error and retains remaining records as
  `deleting`. The controller retries, or the client can repeat DELETE. A missing
  agent does not make its resources disappear from tracking.
- **Expiry:** the controller checks every 60 seconds and retries cleanup failures.
  Expiry is a cleanup trigger, not an exact termination deadline. Cleanup needs a
  running controller, reachable agents and time for queued operations.
- **Restart:** state persists across service restarts. Host reboot does not trigger
  automatic guest restart. See [upgrade guidance](deployment.md#upgrades-and-recovery).

## Agent endpoints

These are primarily the controller-to-agent interface. Direct agent mutations do
not create/adopt controller environment records; use controller endpoints for
normal fleet operations to avoid untracked resources.

| Method | Path | Success payload |
|---|---|---|
| GET | `/api/info` | `AgentInfo` |
| GET | `/api/images` | `string[]` of file/zvol image names |
| POST | `/api/vms` | `CreateVmResp` containing `vm`, HTTP 201 |
| GET | `/api/vms` | `Vm[]` |
| GET | `/api/vms/{id}` | `Vm` |
| DELETE | `/api/vms/{id}` | No payload; idempotent |
| POST | `/api/vms/{id}/stop` | No payload |
| POST | `/api/vms/{id}/start` | No payload |

`CreateVmReq` requires `vm_id`, `env_id`, `image`, `engine`, `cpu`, `mem`, `disk`,
`ports` and `deny_outgoing`; `ssh_keys` and `guest_config` default to empty and
`isolated_network` defaults to false. Unlike controller
requests, there are no sizing defaults here: non-QEMU `disk` must be 0. Agent
mutation errors currently return HTTP 500, including validation failures.

Repeated creation with the same VM ID and parameters reuses the record rather
than allocating a second VM. A stopped VM stays stopped; use start explicitly.
Different parameters or a failed/deleting record are rejected. Clean up failed
records before attempting fresh creation with that ID.
