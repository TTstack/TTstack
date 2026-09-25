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
| POST | `/api/hosts` | `Host`, HTTP 201 (200 for repeat registration); body `{"addr":"10.0.0.2:9100"}` |
| GET | `/api/hosts` | `Host[]` |
| GET | `/api/hosts/{id}` | `Host` |
| DELETE | `/api/hosts/{id}` | No payload; refuses removal while any VM is tracked on it |
| POST | `/api/hosts/{id}/detach` | `Vm[]` orphan report; forgets an offline host without stopping or deleting its guests |
| POST | `/api/envs` | `EnvDetail`, HTTP 202 after persisting the creation plan |
| GET | `/api/envs` | `Env[]` |
| GET | `/api/envs/{id}` | `EnvDetail` |
| DELETE | `/api/envs/{id}` | No payload; idempotent, including an already absent environment |
| POST | `/api/envs/{id}/stop` | No payload |
| POST | `/api/envs/{id}/start` | No payload |
| GET | `/api/vms/{id}` | `Vm` |
| POST | `/api/vms/{id}/resources` | Updated stopped `Vm`; see [offline resources](#offline-resource-updates) |
| GET | `/api/images` | `ImageInfo[]`: `name`, `host_id`; cached file/zvol images on online hosts |
| GET | `/api/status` | `FleetStatus`: host/environment/VM counts and resource reservations |

Host addresses must be `host:port`, without a URL scheme or path. Registration
contacts the agent with the shared bearer key; register only an address whose
ownership you have verified through your trusted management network or tunnel.
An ID check cannot authenticate an untrusted HTTP server. There is no automatic
host discovery. Re-registering the same ID/address is idempotent; conflicting ID
or address bindings are rejected. Mutations serialize per environment; unrelated
environments and heartbeat refresh can progress concurrently. Conflicting requests
for the same environment return 409. GET of a missing host, environment or VM
returns 404. Invalid environment parameters return 400, duplicate names 409, and
unschedulable requests 422. Agent operation failures may surface as 502.

## Offline resource updates

`POST /api/vms/{id}/resources` takes `{"cpu": 4, "mem": 8192, "disk": 16384}`.
All three positive values are required; memory/root disk use MiB. This operation
supports stopped Firecracker VMs on file or ZFS storage. The agent must advertise
**`firecracker_resources`**, separate from creation-time `firecracker_disk_resize`.
It does not stop or start guests, resize containers, or offer an atomic multi-VM
operation. The CLI equivalent is:

```sh
tt env stop demo
tt env resize VM_ID --cpu 4 --mem 8192 --disk 16384
tt env start demo
```

CPU/RAM take effect on the next cold boot. Disk can only grow; `disk` excludes the
4 MiB guest configuration drive. The existing disk, VM identity, ports and opaque
configuration are retained. Both recorded and actual stopped state are checked
before disk mutation. Host admission includes VMM overhead and the additional disk
reservation. A stopped VM does not reserve CPU/RAM for a later start; capacity is
checked again on start. No migration is attempted when its host has no capacity.

Intent is persisted as `Vm.pending_resources` before storage changes. While set,
the larger disk remains reserved and start is refused. Retry the **same target**
to complete an interrupted operation: equal device capacity still runs filesystem
growth, covering interruption between device growth and `resize2fs`. A successful
response clears intent and updates `cpu`, `mem`, `disk` and `options.requested_disk`.
The VM remains stopped. Invalid values/shrink/unsupported capability return 400;
state conflicts or insufficient schedulable capacity return 409. A timeout or 502
can have an unknown outcome: inspect the VM, including pending resources and error,
before retrying. Do not delete the environment to recover a resource update.

This is grow-only operational behavior, not a promise of zero storage risk or a
backup facility. Maintain backups independently. Controller and agent use schema
v3 so older binaries cannot ignore pending reservations; upgrade both together.

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
| `engine` | string | `qemu` by default; JSON values: `qemu`, `firecracker`, `docker` |
| `cpu` | integer | Positive vCPU count; default 2 |
| `mem` | integer | Positive memory in MiB; default 1024 |
| `disk` | integer | Disk size in MiB. QEMU defaults to 40960; Firecracker defaults to the base rootfs size and allows creation-time ext4 growth. Omit for other engines |
| `ports` | integer[] | Up to 256 TCP guest-port entries; default empty; port 22 is added for QEMU |
| `deny_outgoing` | boolean | Default false; block routed outgoing initiation, not host/guest isolation; rejected for Docker |
| `isolated_network` | boolean | Default false; Linux QEMU/Firecracker only; block peers, guest-initiated host access, private/link-local destinations and IPv6; allow public IPv4 egress and replies to inbound connections |
| `guest_config` | object | Default `{}`; Firecracker only; up to 32 simple file names mapped to UTF-8 strings, 64 KiB total names/content; attached as a read-only config drive |
| `ssh_keys` | string[] | Empty; merged with environment keys; QEMU cloud-init only |

CLI engine aliases such as `kvm`, `fc` and `podman` are not JSON enum values.
SSH keys are complete public-key strings in JSON,
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
quota-enforced; Firecracker reserves its requested rootfs size (or the image size when omitted) plus 4 MiB when a
configuration drive is present. Firecracker memory reservations include 128 MiB
of VMM headroom in addition to the guest's `mem`; stopped guests release both.
The scheduler uses reported base-image sizes and includes configuration disks in
creation plans. An omitted Firecracker disk requires an agent that reports that
image's size; otherwise specify a size or upgrade the agent. Explicit sizes below
a known base size are rejected before placement. Agents recheck before allocation.
Docker memory-plus-swap is capped at the requested RAM. Host budgets still need
headroom for the OS, QEMU overhead and other services. Fleet capacity totals
include only online hosts; tracked VM counts also include offline allocations.

Agent `/api/info` returns `host_id`, `resource`, `engines`, `storage`, `images`,
`image_sizes`, `capabilities`, `vms` and optional `warnings`. VM rows and resources
come from the same snapshot; older agents without `vms` use the legacy list read.
Known allocations missing from a snapshot retain conservative reservations;
untracked agent VMs are counted and logged, never silently adopted. Hosts retain
image sizes and a nullable `error` describing the latest probe failure/warnings.

Capabilities are gated on startup prerequisite probes: KVM/tool availability,
nft/ip tooling, cgroup v2 controllers and relevant disk tools. Runtime permissions,
daemon health and guest compatibility can still change or fail. Upgrade agents
before requesting capabilities they do not advertise. A transient catalog failure
returns an empty catalog instead of declaring every existing guest offline.
Unreadable VM rows retain their raw data and close new admission; healthy rows
remain available for background recovery and targeted deletion. Strict inventory
reads report malformed state rather than silently discarding it.
VM placement prefers eligible ZFS hosts, then file hosts; see the
[storage policy](guest-images.md#storage). This does not migrate existing VMs.

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
  disks/containers; start boots/restarts them and reserves capacity before
  concurrent placement. VM memory is not retained. QEMU tries
  guest shutdown for about ten seconds before termination. Firecracker on x86_64 sends `SendCtrlAltDel`,
  waits up to 30 seconds, then terminates on failure/timeout and logs the fallback.
  The image must support orderly shutdown; a successful stop alone does not prove
  that the guest flushed its data. Stopping a paused Firecracker first resumes it.
  Resume failure also falls back to forced termination. Save work before stopping. Partial failures are reported instead of hidden.
- **Delete:** failed cleanup returns an error and retains remaining records as
  `deleting`. The controller retries, or the client can repeat DELETE. A missing
  agent does not make its resources disappear from tracking. Offline hosts are
  retained without repeatedly waiting on mutation timeouts. Confirmed process
  termination gates disk removal; later cleanup steps are attempted independently,
  with any remaining errors retained for retry.
- **Expiry:** the controller checks every 60 seconds and retries cleanup failures.
  Expiry is a cleanup trigger, not an exact termination deadline. Cleanup needs a
  running controller, reachable agents and time for queued operations.
- **Detach:** `tt host detach ID` is an explicit escape hatch for an offline host.
  It returns the tracked VM records as an orphan report, releases controller
  reservations and removes the host. It does not stop guests or delete disks.
  Save the report and reclaim resources on that host separately before reuse.
- **Recovery:** failed network restoration is retried; late VMM readiness can clear
  stale errors without restarting the guest. A missing live TAP cannot be attached
  to a running VMM by recreating its name; stop/start is required. Missing jailed
  Firecracker metadata fails safely until restored. Missing/corrupt PID files use
  VM-specific process markers for recovery, never unconditional disk deletion.
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
| POST | `/api/vms/{id}/resources` | Updated stopped `Vm`; see [offline resources](#offline-resource-updates) |
| DELETE | `/api/vms/{id}` | No payload; idempotent |
| POST | `/api/vms/{id}/stop` | No payload |
| POST | `/api/vms/{id}/start` | No payload |

`CreateVmReq` requires `vm_id`, `env_id`, `image`, `engine`, `cpu`, `mem`, `disk`,
`ports` and `deny_outgoing`; `ssh_keys` and `guest_config` default to empty and
`isolated_network` defaults to false. Unlike controller
requests, QEMU requires a positive `disk`; Firecracker accepts 0 for the image
size or a positive creation-time size in MiB. Other engines require 0. Agent
mutation errors currently return HTTP 500, including validation failures.

Repeated creation with the same VM ID and parameters reuses the record rather
than allocating a second VM. A stopped VM stays stopped; use start explicitly.
Port and SSH-key order does not change request identity. Different parameters or
a failed/deleting record are rejected. Clean up failed
records before attempting fresh creation with that ID.
