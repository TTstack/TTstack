# TTstack audit, 2026-09-25

Point-in-time review of architecture, implementation, and documentation at
revision `83b7e95` (`master`). This is not a maintained behavior guide and not
a live-host validation. Findings below were checked against the current tree.
Where a failure depends on host firewall state or a local attacker, that
precondition is stated; it was not reproduced on a running fleet.

This revision merges two independent review passes over that same code: a
control-plane, cleanup, and deployment pass, and a second pass over lifecycle
recovery, scheduling inputs, deployment transport, operator workflows, and
documentation fidelity. Findings are numbered sequentially and cross-references
are internal to this file. The two passes agreed where they overlapped, and each
found things the other missed; nothing was removed in the merge.

Severity is the operational consequence if the documented install and upgrade
path is followed. Documented scope limits (one controller, no image
distribution, no migration, no high availability, owner as a label, HTTP
without TLS) are not listed as defects.

Baseline checks on the reviewed tree: `cargo build --release`, `cargo test
--workspace` (136 tests), `cargo clippy -- -D warnings` and `cargo fmt --check`
all pass; the tree contains no `TODO` markers and no `#[allow(dead_code)]`. The
defects below are therefore not visible to the existing gate.

## Summary

The lifecycle model is careful in the places the tests cover: a create plan is
stored before agent work, creates are not HTTP-retried, and a missing agent
does not by itself delete tracking rows. The unreasonable parts are around
that model.

The control plane is one fleet-wide lock held across multi-minute agent calls,
so one unreachable host stalls unrelated cleanup, and nothing in the system can
release a record that a dead host or an unreadable PID file has pinned. Host
identity is an upsert of whatever the agent reports, and the shared
administrator key is sent to that address before the body is trusted. Cleanup
is a single fallible pipeline: a firewall or resume error prevents disk and
process reclaim, and the retry repeats the same prefix. Scheduling, agent
admission, and engine limits do not use the same disk and memory numbers, and
the capability flags the scheduler trusts are a per-platform constant rather
than a measurement of the host. Recovery steps that run once (network restore,
tap ownership) are not retried, while the errors they record never clear.
Deployment rewrites service units and recursively changes ownership under the
service home, which undoes Firecracker jail ownership on the documented upgrade
path, and it stages root-executed binaries in predictable `/tmp` paths. Several
maintained sentences describe the agent-side or intended contract as if the
controller, dashboard, and upgrade path implemented it.

## High

### 1. One agent call holds the fleet mutation lock

Create, delete, stop, start, and expiry all take `CtlShared.operations` and
keep it across agent HTTP calls. The mutation client timeout is 360 seconds,
and VMs are handled one after another. Heartbeat refresh only `try_lock`s the
same mutex, so it cannot update host state while a call is stuck. Delete does
not skip a host already marked offline; a missing address still waits on the
timeout path when the host row exists but does not answer.

Evidence: `crates/ctl/src/handler.rs` (`create_environment` around the
`lock_owned` call, `finish_creation`, `delete_environment_inner`,
`change_environment`) and `crates/ctl/src/main.rs` (heartbeat and expiry
loops).

A create or delete against an agent that accepts TCP and never answers occupies
the lock for up to 360 seconds per VM. During that window expiry cannot delete
other environments, including ones on healthy hosts. There is no force-forget.
`remove_host` refuses while VM rows remain, those rows count toward the fleet
VM cap, and expiry retries forever, paying the timeout again. `finish_creation`
also does not re-check `expires_at`, so a short lifetime still runs the full
create before expiry can start.

The create path does not merely block one request. It moves the guard into a
detached task and returns `202` to the client, so the lock outlives the HTTP
call it belongs to:

```rust
let _operation = state.operations.clone().lock_owned().await;   // handler.rs:290
...
tokio::spawn(async move {
    let _operation = _operation;                                 // handler.rs:388-393
    if let Err((_, error)) = finish_creation(state, env_id, requests).await { ... }
});
```

Three consequences follow from that shape. A single request may carry up to
`MAX_VMS` VMs (`handler.rs:256` rejects only above 1000), and `finish_creation`
walks them sequentially with the 360-second client, so one wedged host can pin
the fleet lock for tens of hours, not minutes; eight VMs are already about 48
minutes. While it is pinned, `register_host` and `remove_host` answer `409`
through `try_lock` (`handler.rs:67`, `:191`), so host maintenance is blocked
too. The CLI's own 60-second create timeout can expire while the server is
still queued behind the lock, which means the documented ten-minute poll never
starts (finding 17).

`docs/rest-api.md` says mutations are serialized and that cleanup needs time
for queued operations. It does not say one unresponsive agent blocks unrelated
deletes, or that a dead host cannot be dropped from tracking without a
successful agent response.

### 2. Nothing can release a stuck record

Two independent paths reach the same dead end: a record whose cleanup cannot
make progress, and no supported operation that forgets it.

An unreadable PID file stops deletion. `existing_pid` parses the file and
returns an error rather than `None` when the contents are not a number:

```rust
Ok(s) => Ok(Some(s.trim().parse::<u32>().c(d!("invalid QEMU PID"))?)),
```

Evidence: `crates/core/src/engine/qemu.rs` (`existing_pid`),
`crates/core/src/engine/firecracker.rs` (`read_pid`),
`crates/agent/src/runtime.rs` (`destroy_vm`, `reconcile`).

`stop`, `state`, and `destroy` all call it, and `destroy_vm` propagates the
error, so the record stays `Deleting`. QEMU writes its own `-pidfile` in place
when it launches (`qemu.rs:48`), so a crash, a killed VMM, or a partially
restored filesystem can leave it empty or truncated; Firecracker's pid file is
written atomically by the agent (`private_write`), so it needs manual
corruption. While the record is stuck, `Resource::account` keeps counting its
disk and VM slot, its IP and host ports stay reserved, `create_vm` refuses the
same VM id (`runtime.rs:112-118`), and the 15-second reconcile retries the same
failing destroy forever, one stderr line per tick.

A dead host is the second path, and it is the more likely one. `DELETE
/api/envs/{id}` sends one agent call per VM and keeps the environment in
`Deleting` with a `502` when any of them fails (`handler.rs:519-523`,
`:534-548`); the expiry loop retries the same calls every 60 seconds
(`main.rs:131-155`). `remove_host` refuses while `vms_by_host` is non-empty
(`handler.rs:210-218`), and those rows keep counting toward `MAX_VMS`
(`handler.rs:302-309`), so a host that never returns permanently consumes
fleet capacity and can eventually make every new create fail with `409 fleet VM
limit reached`. Once the fleet is at `MAX_HOSTS`, its slot cannot be reused
either.

There is no third way out. The controller exposes nine routes
(`crates/ctl/src/main.rs:66-88`) and the agent six (`crates/agent/src/main.rs:86-96`);
none forgets or detaches a record, no CLI verb has a force flag, and hand
editing `ctl.db` is what `docs/deployment.md` tells operators not to do.
Retaining a record whose cleanup failed is the right default; what is missing
is an escape hatch with an explicit orphan report next to it. The
smallest useful shape is one "detach this record, report what remains on the
host" operation per side, plus making the delete path treat an unreadable PID
file as "unknown, continue cleaning" instead of a fatal error.

### 3. Re-registering a host id retargets the fleet, and registration discloses the admin key

`POST /api/hosts` GETs `http://{addr}/api/info` with the controller bearer token
before the body is validated, then `put_host` does `INSERT OR REPLACE` keyed
only by the agent-reported `host_id`. There is no check that this id is already
registered at another address, and no check that this address is already
registered.

Evidence: `crates/ctl/src/handler.rs` (`register_host`, `agent_client`) and
`crates/ctl/src/db.rs` (`put_host`).

A second registration whose agent returns an existing id (cloned agent
database, reused `--host-id`, or a non-agent that copies a previous info
response) overwrites `addr`. Later stop and delete go to the new address.
Agent delete is successful when the id is absent, so the controller can drop
the rows while guests on the old address keep running. That contradicts the
delete rule that a missing agent must not make resources disappear from
tracking.

The same function rejects a legitimate re-register when `host_count() >=
MAX_HOSTS`, because the cap is checked before replace. `host_count().unwrap_or(0)`
treats a database error as zero hosts and continues.

The first probe also sends `Authorization: Bearer <controller key>` to whatever
`host:port` was submitted. A typo or a non-agent receives the fleet admin key
in cleartext. Heartbeat refresh keeps sending that key to every registered
address. README says the key is shared and that the services speak HTTP. It
does not say registration discloses it to an unverified address.

### 4. Deployment puts the fleet key on a command line, then recursively chowns guest disks

Distributed deploy interpolates `TT_API_KEY=...` into the remote shell script.
`SshTarget::exec` passes that script as the SSH remote command. For the
duration of deploy, the key is visible in process arguments on the deploying
host and on the target. The script comment says the environment file keeps
secrets out of process arguments. That is true of the installed unit, and false
of the transport that writes the file. `parse_api_key` only restricts the
character set. The unit test `remote_script_contains_sudo` requires
`TT_API_KEY=test-key` to appear in the generated script, so the leak is frozen
by CI.

Local deploy prints the key and tells the operator to pass it to
`tt config ... --api-key`. README and `docs/deployment.md` repeat that command.
`TT_API_KEY` exists and is not what those instructions use.

Both local and remote deploy then run `chown -R` on the service home
(`/home/ttstack` by default). Remote deploy appends `|| true`. Local deploy
checks that `chown` started, not that it exited zero. The default runtime
directory is `/home/ttstack/runtime`. Firecracker jail members live under
`{clone}/.jailer` and, on the file backend, the root disk and config image are
hard links. `chown` of a hard link changes that inode, which jailer had assigned
to the per-VM UID (mode `0600` / `0400`). After the documented upgrade ("repeat
the same deployment command"), those files are owned by `ttstack`, which is
also the controller account. Query APIs return only a configuration digest; the
files on disk are the contents. A running VMM may keep an already-open
descriptor. A later reopen by the jail UID fails.

This contradicts `docs/guest-images.md`, which says the jail disk is owned by
the non-root VMM UID, and the upgrade section, which treats redeploy as an
idempotent restart that keeps guests. The 2026-09-24 validation records an
agent service restart, not this `chown`. A custom `runtime_dir` outside the
service home is not chowned; the default layout is. Zvol root devices created
with `mknod` inside the jail are reowned if that jail directory is under the
chowned tree. The host `/dev/zvol` node is not.

Evidence: `crates/cli/src/deploy.rs` (`remote_setup_script`, `SshTarget::exec`,
`local_ensure_dirs`, `deploy_local`) and
`crates/core/src/engine/firecracker/sandbox.rs` (`prepare`, `stage_member`).

The same deploy path stages root-executed binaries in a predictable directory
before this; see finding 7.

### 5. Paused Firecracker cannot be force-stopped, and delete is not best-effort

If the Firecracker API reports `Paused` and `PATCH /vm` resume fails, `stop`
returns before `SIGKILL`. `destroy` calls `stop` first, so the VMM is never
killed. The record stays `Deleting`, and reconcile retries the same path.
Memory and the cgroup stay held.

Evidence: `crates/core/src/engine/firecracker.rs` (`stop`, `destroy`).

`docs/rest-api.md` says stopping a paused Firecracker resumes it, then
terminates on failure or timeout. The code terminates only when resume
returned success, or when the VM was not observed as paused.

Separately, agent delete is one sequence: engine destroy, then NAT rule
deletion, denylist removal, tap deletion, isolation removal, then disk removal.
The first error is saved and the rest is skipped. `remove_port_forwards` and
`allow_outgoing` fail if the `tt-nat` table cannot be listed. A VM that failed
after a clone, or a host whose firewall table was removed, therefore retries
forever and never reaches `remove_image`. `remove_isolation` is idempotent.
Port-forward and denylist cleanup are not.

Evidence: `crates/agent/src/runtime.rs` (`destroy_vm`) and
`crates/core/src/net.rs` (`remove_port_forwards`, `allow_outgoing`).

`remove_port_forwards` is also text parsing: it lists the chain with `nft -a`,
splits each line on whitespace, and matches any field whose portion before `:`
equals the guest address. It is correct for the rules this code writes, and it
is the only thing standing between a stale rule and a live one under a future
`nft` output change.

The retention rule (do not forget a VM whose cleanup failed) is sound. Applying
it to an unrelated earlier step is not: a missing nft table should not pin a
disk, and a failed resume should not pin a process that `SIGKILL` can reap. The
same rule is inconsistent in the other direction too: agent `DELETE /api/vms/{id}`
returns success for an unknown id (idempotent), while `stop` and `start` return
error `500` for one (`runtime.rs:274-280`). A VM row lost on the agent — a
restored or replaced `agent.db` — therefore makes `tt env stop` fail the whole
environment forever with no convergence path, while `tt env delete` succeeds.
Making stop treat "already absent" as success matches the delete contract and
the intent of the operation.

### 6. `tt0` is treated as healthy if the name exists, and VM forwarding is not integrated with Docker

`setup_bridge` returns success when any interface is named `tt0`. It does not
check that the address is `10.10.0.1/16` or that the link is up. A crash after
`ip link add` and before `ip addr add`, or a pre-existing `tt0` with another
configuration, is permanent: later starts only set `ip_forward`. Guests are
given that gateway by cloud-init and by the Firecracker `ip=` argument. Delete
does not remove the bridge, so a bad device survives VM cleanup.

Evidence: `crates/core/src/net.rs` (`setup_bridge`).

The same file installs NAT and a filter chain only in the `ip tt-nat` table.
There is no `DOCKER-USER` accept, no `br_netfilter` handling, and no exception
for `tt0`. Docker's default filter integration sets the forward policy to drop
for traffic that is not on a Docker bridge. Those hooks are independent of an
accept in `tt-nat`. On a host where that Docker policy is active, QEMU and
Firecracker egress can fail while container publish still works. The agent
advertises Docker and the VM engines together whenever the binaries exist.
This coexistence failure was not live-tested here; the gap is that the
implementation has no policy for it.

The mechanism is worth stating because it is not obvious from the code: both
`tt-nat` and Docker's `filter` table install base chains on the same forward
hook, and a `drop` verdict or policy in either chain is final for the packet.
An `accept` in `tt-nat` cannot exempt traffic from Docker's policy.

### 7. Root-executed staging lives in predictable `/tmp` paths

`tt image create` for Firecracker downloads the kernel, then creates
`/tmp/tt-image-{pid}` with `create_dir_all` and loop-mounts an ext4 image
there. `create_dir_all` succeeds if that path already exists as a directory,
including through a symlink. `mount` follows the symlink. The kernel download
happens first, so the PID is visible for a long time. The documented recipe
command runs as root. A local user can point that path at another directory
and have root mount and extract an archive onto it. The Alpine tarball is
extracted with `tar` as root and is not checksum-verified (finding 20). The
guest-image guide says recipes are not a lockfile. It does not mention this
race.

Evidence: `crates/cli/src/image_builder.rs` (`create_firecracker`, `tempdir`).

Distributed deploy has the same shape for binaries that are about to run as
root: it creates `/tmp/ttstack-deploy-{pid}` with `mkdir -p`, copies the
release binaries there over `scp`, and only later runs `sudo cp` into
`{prefix}/bin` and starts the service. The directory name is predictable, the
value is visible in the `ssh` argv, and `mkdir -p` accepts a directory that an
unprivileged local user already created and therefore owns. Between the `scp`
and the `sudo cp` (a separate connection later) that user can replace the
staged file, and root installs and starts it. Nothing checks ownership, mode,
or checksum of the staged binaries.

Evidence: `crates/cli/src/deploy.rs` (`deploy_distributed` controller and agent
staging).

### 8. Network object names are not stable across a toolchain upgrade

`tap_name` is `DefaultHasher` of the VM id, formatted as `tt-` plus 12 hex
digits. Rust does not guarantee that hasher across compiler releases. The same
function supplies the tap device, the Firecracker jailer id and cgroup path,
and the isolation table name.

Evidence: `crates/core/src/net.rs` (`tap_name`),
`crates/core/src/engine/firecracker/sandbox.rs` (`prepare`), and
`crates/core/src/net/isolation.rs` (`table`).

Rebuilding with a newer compiler, which is a normal way to follow the
documented upgrade path, can change those names. Cleanup then targets the new name and leaves the old tap,
nft table, and cgroup behind. A running Firecracker sandbox JSON still points
at the old jail, while recovery creates a new tap the running VMM is not
attached to. This is an identity bug, not a style choice: these names are
on-host state. The in-tree test `tap_name_deterministic` only proves stability
within one build, which is exactly the property that is not guaranteed.

### 9. A missing `sandbox.json` makes every Firecracker liveness check miss

`identity` returns the in-jail API socket path when the sandbox metadata file
exists, and silently falls back to the host path when it does not:

```rust
fn identity(vm: &Vm) -> Result<(String, String)> {
    if let Some(sandbox) = Sandbox::load(vm)? {
        return Ok((sandbox.socket(), sandbox.marker));
    }
    let path = format!("{RUN_DIR}/fc-{}.sock", vm.id);
    Ok((path.clone(), path))
}
```

`Sandbox::load` maps `NotFound` to `Ok(None)`, so a missing
`/home/ttstack/run/fc-{id}.sandbox.json` is not an error. The fallback marker
is the host path, while a jailed VMM's argv contains the jail-relative path
(`marker = "/fc-{id}.sock"`), so `process_matches` can never match a jailed
process.

Evidence: `crates/core/src/engine/firecracker.rs` (`identity`, `stop`,
`destroy`) and `crates/core/src/engine/firecracker/sandbox.rs` (`Sandbox::load`,
`prepare`).

The consequence is a cascade rather than a single failed call. `stop` returns
`Ok(())` without stopping anything because the marker does not match
(`firecracker.rs:164-171`), and the agent persists `Stopped`
(`runtime.rs:281-292`). A later `start` therefore takes the "create a stopped
VM" branch, which destroys and recreates the live VM's tap
(`net.rs:291-296`), overwrites the PID file with the new short-lived child, and
then fails on the existing cgroup or socket, leaving the record `Failed` while
the original VMM still runs. A subsequent `destroy` finds no sandbox, kills
nothing, removes the clone disk, and drops the row — deleting the disk of a
running guest.

`firecracker.rs:234-235` states the invariant this violates: during `exec`, a
missing identity marker is not evidence that the child exited. The stop path
assumes the opposite. The fix is to fail loudly when the sandbox metadata is
absent for a VM the agent believes exists, and to derive the marker from a
source that cannot disappear independently of the process.

## Medium

### 10. Controller placement does not reserve what the agent reserves

For an omitted Firecracker disk, `disk_reservation` uses `Engine::default_disk()`,
which is 0, plus 4 MiB when a config drive is present. The agent reserves
`max(requested, image size)` plus that 4 MiB. Many image-sized disks can
`can_fit` on one host. The plan is committed, then the agent rejects later VMs.
That is a partial environment the operator must delete and recreate. Placement
also prefers any fitting ZFS host, so the over-admission lands on ZFS first.
QEMU placement uses the requested size only; the agent can still reject the
create when the base virtual size is larger, after the controller has accepted
the environment.

Evidence: `crates/ctl/src/scheduler.rs` (`disk_reservation`) and
`crates/agent/src/runtime.rs` (Firecracker disk calculation in `create_vm`).

`docs/rest-api.md` says Firecracker reserves the requested size, or the image
size when omitted. The agent does that after it has the image. The scheduler,
which is what "scheduling uses disk reservations" refers to in the README, does
not. The scheduler comment acknowledges the split. The result is still
over-admission.

Memory is similarly split. Default `--mem-total 0` advertises `MemTotal`, not
available memory and not a host reserve. QEMU is started with `-m` and no
cgroup, so VMM overhead is outside the shared reservation. Docker sets
`--memory` but not `--memory-swap`. When the host has swap, Docker's default
is to allow swap equal to that limit, so the host commitment can be twice
`mem`. Firecracker is the only engine hard-capped
at guest RAM plus 128 MiB. Stopped guests release CPU and memory only in the
agent's `Resource::account`. A controller row whose VM is absent from the latest
`/api/info` snapshot keeps a `MAX_VMS` slot and is marked failed, but
`host.resource` is replaced by the agent totals, so the missing VM's CPU,
memory, and disk are free for the next placement. `/api/info` and `/api/vms`
are also two requests, not one snapshot.

Evidence: `crates/agent/src/config.rs` (`effective_mem`),
`crates/core/src/engine/docker.rs` (`create`), and
`crates/ctl/src/handler.rs` (`apply_host_snapshot`).

`docs/rest-api.md` says failed or incomplete operations retain reservations
until cleanup. That holds for rows the agent still reports. It does not hold
for a controller row the latest info snapshot omits, or for a create timeout
whose request has not yet been saved on the agent.

The same probe also drops rows it does not know. `apply_host_snapshot`
iterates the controller's own VM list and adopts matching agent records; a VM
that exists on the agent but not in `ctl.db` is ignored without a log line.
After a controller database loss or reset, running guests stay invisible to
`tt env list`, and there is no fleet-wide VM inventory (only `GET
/api/vms/{id}`) to discover them. A one-line warning per unknown VM would make
the condition visible without adopting anything.

### 11. Agent capabilities are a per-platform constant, so the scheduler trusts labels

`AgentInfo.capabilities` is a fixed vector of five names for every Linux agent,
independent of what the host can actually do. `nft` missing, cgroup v2 absent,
or an unusable `firecracker`/`jailer` pair do not change it. The scheduler
treats those names as facts when it decides placement.

Evidence: `crates/agent/src/runtime.rs` (`agent_info`) and
`crates/ctl/src/scheduler.rs` (`supports`).

A host without `nft` advertises `isolated_network`; a host without cgroup v2
advertises `firecracker_jailer` even though `Sandbox::prepare` refuses to start
(`sandbox.rs:34-36`); engine detection is only a `which` probe, so a
present-but-broken binary advertises the engine. Each of these is discovered at
create time, after the controller has accepted the environment with `202`, and
lands as a failed VM. Deriving the list from host facts at startup (or from the
first failing call) would move the failure to registration, where the operator
can see it before scheduling.

`docs/rest-api.md` says reported capabilities "describe implementation support,
not a substitute for host prerequisites". That is honest, but it means the
field cannot do the job the scheduler uses it for.

### 12. `failed` means too many different things, and some errors never clear

A 5-second host refresh treats timeout as `HostState::Offline` and does not
update VM rows. `update_env_state` then marks every environment on that host
`failed` with "agents are offline". Stop and start require the post-refresh
environment state to match the requested state even when every agent call
succeeded, so a slow but successful stop returns 502 and `failed`. The next
healthy refresh can set `stopped` and clear `env.error`. The CLI, on any
non-active create result, tells the operator to inspect or delete. An offline
snapshot and a partial create therefore look like the same recovery action.

`update_env_state` sets `failed` if any VM has `error`, but it does not copy
that text into `env.error` except for the offline case. After a partial create,
`GET /api/envs` can show `failed` with `error: null`. `Creating` is checked
before errors, so one VM still reported as creating hides sibling failures from
the CLI poll.

Agent reconcile clears `error` only when it starts with `cannot query engine:`.
A Firecracker boot-timeout string is kept after the VMM is later observed
running. The controller then keeps the environment `failed`. The same stickiness
applies to `network recovery failed` on an otherwise running guest.

This state is not only confusing, it is terminal without a guest restart. Once
a VM carries an error, the environment is recomputed as `failed` on every
refresh, and the only operations that clear the field are `stop_vm` and
`start_vm` on success (`runtime.rs:284`, `:337`). A guest that is running
correctly therefore cannot return to `active` except by stopping it — losing
its memory and in-flight application state — or by deleting the environment.
The case is reachable by design: Firecracker's boot-readiness wait is 10
seconds, and `create` deliberately leaves the VMM alive on a timeout so it can
be inspected.

Two further causes of a false `failed` are worth naming, because both look like
agent downtime from the API:

- A transient failure to list the image catalog fails the whole `GET
  /api/info` call (`crates/agent/src/handler.rs` `get_info`), which the
  controller records as `HostState::Offline` (`apply_host_snapshot`), flipping
  every environment on that host to `failed`. A broken image directory or a
  busy `zfs list` should degrade to an empty catalog, not take the host
  offline.
- The agent performs `ensure_network`, `reconcile`, and per-VM network restore
  before it binds the listener (`crates/agent/src/main.rs:48-113`), so a
  restart of an agent with guests is an interval of "offline" from the
  controller's point of view, with the same `failed` outcome.

Evidence: `crates/ctl/src/handler.rs` (`change_environment`, `update_env_state`,
`apply_host_snapshot`), `crates/agent/src/runtime.rs` (`reconcile`),
`crates/agent/src/handler.rs` (`get_info`), and `crates/cli/src/main.rs`
(create poll).

`docs/rest-api.md` says an offline agent means last snapshots, not proof the
guests stopped, and that partial failures remain visible in `env.error`, VM
`error`, and `warnings`. VM `error` and detail warnings do. List-level
`env.error` often does not, and `failed` is also what a transient refresh
produces.

### 13. Process identity is a pidfile plus a cmdline string

`terminate` returns success if `/proc/PID/cmdline` does not contain the marker.
The Firecracker boot path already notes that cmdline can be empty during
`exec` and that a missing marker is not proof of exit. Stop and destroy use the
opposite rule. A stop that races jailer or QEMU startup can report success,
after which the controller releases CPU and memory while the VMM is still
coming up. An invalid pidfile is the other failure: parse failure makes stop
or destroy return an error, so cleanup never reaches disk removal (finding 2).
There is no pidfd or start-time check.

QEMU's marker is the VM id as a whole argv element (`-name` value). Controller
VM ids are UUIDs, so an accidental match against an unrelated process is
unlikely on the normal path. The false "already dead" result during `exec` does
not depend on that. A VM created directly on the agent may use any
`validate_name`-safe id, which widens the same check to any process whose argv
happens to contain that word. QEMU already has a unique marker in its own argv
(`-pidfile` and `-monitor` paths, `qemu.rs:48-52`); using one of those removes
the ambiguity without changing the interface.

Evidence: `crates/core/src/engine/mod.rs` (`process_matches`, `terminate`),
`crates/core/src/engine/qemu.rs` (`stop`), and
`crates/core/src/engine/firecracker.rs` (`stop`, `wait_for_boot`).

### 14. Published VM ports are not reserved, and NAT state is not fully reclaimed

Allocation binds `0.0.0.0:port` and drops the socket, then later installs
`tcp dport {port} dnat` with no interface or destination match. Another process
can bind the port in between; prerouting DNAT still steals new connections.
There is no per-VM port cap, so one request can walk 20000–65535 with a bind
per port while holding the agent runtime mutex, blocking stop and delete on
that host. Delete does not flush conntrack, and the lowest free IP is reused
immediately, so a new guest can inherit the previous guest's NAT sessions.
After the last VM is gone, `tt0`, the `tt-nat` masquerade, and `ip_forward=1`
stay in place.

Evidence: `crates/agent/src/runtime.rs` (`allocate_ports` call site) and
`crates/core/src/net.rs` (`add_port_forward`).

Within one agent the allocation itself is safe: it runs under the runtime
mutex and seeds its used set from every record, so two VMs cannot receive the
same host port, and exhaustion fails explicitly. The gaps above are all about
state the agent does not own or does not clean.

### 15. Firecracker tap recovery does not restore jail ownership, and nothing retries it

`prepare_jailed_tap` is only used before launching a stopped Firecracker. The
comment in `net.rs` says it is never used during live recovery. Agent startup,
for every VM still recorded as running or paused, calls `restore_network`, and
that calls `create_tap`. `create_tap_owned` creates a missing device as uid 0
when no owner is passed, and if the device already exists it does not change
the owner. A tap that disappeared while the jailed VMM is still running is
therefore recreated as root. The jailed process cannot open it. Recovery does
not restart that VMM, so the guest stays network-dead. The stored error is
`network recovery failed: ...`, which finding 12 does not clear when the process
is later observed running.

On a cold start the order is the opposite, and it is not atomic.
`prepare_jailed_tap` deletes the tap before `ip tuntap add` creates the
replacement. If that add fails, the tap is gone. `start_vm` has already
persisted `Failed`, and start refuses any state other than stopped or paused,
so the failed start is not retried. The operator has to delete and recreate.

The larger shape of this gap is that network recovery is single-shot. Agent
startup flushes the whole `prerouting` chain while installing NAT
(`net.rs:124-128`), then re-adds port forwards once per VM in a loop that only
records an error on failure. The 15-second `reconcile` repairs state, never
networking. A transient failure in any of `isolate`, `create_tap`,
`remove_port_forwards`, or `add_port_forward` therefore leaves a served guest
without its published ports indefinitely, while the API continues to report it
as running. Making the recovery step idempotent and retrying it from
`reconcile` costs one pass over the same records that loop already visits.

Evidence: `crates/core/src/net.rs` (`create_tap_owned`, `prepare_jailed_tap`,
`setup_nat`), `crates/core/src/engine/firecracker/sandbox.rs` (`prepare`), and
`crates/agent/src/runtime.rs` (startup recovery loop, `restore_network`,
`start_vm`, `reconcile`).

### 16. One bad JSON row disables list, refresh, and delete

Hosts, environments, and VMs are JSON blobs. `query_all` aborts the entire
result on the first deserialize error. Controller refresh returns immediately
if `list_hosts` fails. Delete needs that list before it can clean up. Agent
`load_all_vms` has the same all-or-nothing behavior, so one unknown engine
value prevents cleanup of every other VM, including `Deleting` retries. The
in-tree tests lock this in.

`PRAGMA foreign_keys=ON` does nothing: the schema has no foreign keys.
Environment, VM, and host rows can diverge; integrity is only in the handlers.
The agent writer connection has no `busy_timeout` (only the read path sets one,
`runtime.rs:464-467`), and nothing takes a lockfile, so two agent processes can
open the same database and both program taps and nft. The controller rewrites
every host row, every VM row on that host, and every environment row each 15
seconds, on tokio worker threads behind a `std::sync::Mutex`, so a slow
`data_dir` stalls API reads as well.

Evidence: `crates/ctl/src/db.rs` (`query_all`) and
`crates/agent/src/runtime.rs` (`load_all_vms`, `read_vms`).

This is a reasonable prototype schema. It is not a reasonable recovery schema:
the operation that must still work when a row is corrupt is delete.

### 17. CLI and dashboard defaults disagree with the API the guides document

- Create uses the client's 60-second timeout. The ten-minute poll starts only
  after POST returns. The controller holds the fleet lock and refreshes hosts
  before it returns 202, and another stop or delete can hold that lock for up
  to 360 seconds per VM. A create can therefore fail at 60 seconds while the
  server is still accepting it, so the documented poll never starts. README and
  `docs/rest-api.md` say the CLI waits up to ten minutes. The error string does
  say to inspect before retrying. Delete and stop use a 600-second client
  timeout, which is still shorter than several sequential 360-second agent
  calls. Dropping the client does not abort the controller task.
- Read requests carry the same "a submitted operation may still be running"
  message as writes, so a failed `tt env show` against a stopped controller
  suggests a phantom operation was submitted.
- `tt --server` replaces the address and drops the key stored in `~/.ttconfig`
  unless `--api-key` or `TT_API_KEY` is also set. The flag help says the address
  overrides the file. It does not say the key is ignored. A create with
  `--guest-config` then goes to whatever answers, with no bearer token.
- `tt config <addr>` without `--api-key` rewrites `~/.ttconfig` with the
  address alone, silently deleting a stored key. Re-pointing the CLI at the
  same controller then fails authentication with no indication why.
- `--ssh-key` reads any absolute or `~/` path, including a private key, and
  puts the bytes in the create JSON. The controller rejects material that is
  not one OpenSSH public key, and the error does not echo it, but rejection
  happens after `post()` has already sent the body over HTTP. README says never
  pass the private key. The CLI does not enforce that.
- `tt env delete` has no confirmation and prints `Environment deleted` when the
  API's idempotent delete finds no such name. The dashboard confirms. The API
  behavior is documented; the CLI sentence is not.
- The dashboard disk field defaults to 40960 and create always sends that value
  for Firecracker. The API and CLI treat an omitted Firecracker disk as image
  size. `fc-alpine` is a 128 MiB rootfs, so a dashboard create grows and
  reserves 40 GiB. Clearing the lifetime field is `parseInt(...) || 0`, and `0`
  means never expire. The page always sends `lifetime`, so the server default
  of 21600 is not used. The image placeholder is `ubuntu-22.04`, which is not a
  built-in recipe.
- The dashboard refresh interval is 5000 ms under a comment that says 30
  seconds. Its HTML-escape helper does not escape quotes, which is safe today
  only because `validate_name` keeps quotes out of the identifiers that reach
  the inline handlers; that is a property of a different file, not of the
  dashboard.
- `~/.ttconfig` is written with `std::fs::write`, then `chmod 0600`, and a
  failed chmod is ignored. If `HOME` is unset, the key is written to
  `./.ttconfig`. The write is not atomic, so a crash between write and chmod
  can lose the key line silently; a hand-added second line is treated as the
  key with no syntax to mark a comment.

Evidence: `crates/cli/src/client.rs`, `crates/cli/src/main.rs` (server
selection, SSH key resolution, create poll, delete), and
`crates/ctl/src/web.rs` (create form and `createEnv`).

### 18. Deploy configuration is interpolated into a root shell, and unit edits are not preserved

`user`, `prefix`, `image_dir`, `runtime_dir`, `listen`, `host_id`, and
`data_dir` are inserted unquoted into the remote script. Only the API key is
charset-checked. A semicolon in `image_dir` or `user` runs as root on the
target. `docs/deployment.md` and `tools/deploy.toml.example` do not say values
must be shell-safe. Zvol dataset names are also passed to `mkdir -p`, so a
dataset name becomes a relative directory under the SSH user's home. The
example says these are dataset names, not directories.

`systemd_unit` has no `RequiresMountsFor=` or `ExecStartPre=mountpoint`. Every
deploy overwrites `/etc/systemd/system/tt-agent.service`.
`docs/guest-images.md` tells operators to add those lines so a missing dataset
mount fails startup instead of creating VM state on the system disk.
`docs/deployment.md` says to upgrade by repeating deploy, which removes those
guards. Nothing in the deployment guide says unit edits are not preserved.

If the target has neither systemd nor OpenRC, the script runs
`pkill -f '{prefix}/bin/{service_name}'`. That pattern matches the deploy
shell itself, because the remote command line contains the same path.

Local deploy always binds the agent to `0.0.0.0:9100` and the controller to
`0.0.0.0:9200`, as root for the agent, with no flag to bind a management
address. Distributed deploy can set `listen`. The quick start never configures
a firewall. README already says to use a trusted network. The install path
cannot express that constraint.

Three further differences between the two deploy paths are worth recording,
because only one of them is exercised by the local path:

- Failure reporting. `SshTarget::exec` keeps only stderr, and the script's own
  progress lines go to stdout, which the CLI prints only when the step
  succeeds. With `set -e` and `cmd && echo ...` checks, a failing
  `systemctl is-active` therefore surfaces as `ssh command failed on HOST:`
  with an empty reason, and no indication of which step failed. The command is
  also fully buffered with no timeout, so a `systemctl restart` waiting for
  `TimeoutStopSec=360` looks like a hang with no output.
- Init detection. The remote script tests `command -v systemctl && [ -d
  /etc/systemd/system ]`; the local path tests `/run/systemd/system`. On a
  target where systemd is installed but not PID 1, the remote script takes the
  systemd branch, fails `daemon-reload`, and aborts after users, directories,
  and binaries have been created.
- OpenRC supervision. The generated OpenRC script uses `command_background`
  with no `supervise-daemon`/`respawn`, while the systemd unit sets
  `Restart=on-failure` and `RestartSec=5`. The same scripts log to an
  unrotated `/var/log/{service}.log` in the no-init fallback. An OpenRC host
  therefore has different reliability from the same deployment on systemd.

Evidence: `crates/cli/src/deploy.rs` (`remote_setup_script`, `systemd_unit`,
`deploy_local`).

### 19. File-backed QEMU disks are not mode-restricted

`clone_image` is `cp -a` and does not chmod the clone. Runtime and image
directories are created with `create_dir_all`, so their mode is the process
umask (typically `0755` for a root agent). Cloud images are commonly `0644`.
Any local account that can traverse `/home/ttstack` can then read the guest
disk for the life of the VM. Firecracker chmods the hard-linked rootfs to
`0600` and its other jail members to `0400` (`sandbox.rs:85-87`), which does
not cover QEMU images; nothing in the file store or the agent runtime sets a
mode at all. Finding 4 then reowns the Firecracker inode to the controller user
on the next deploy.

Evidence: `crates/core/src/storage/file.rs` (`clone_image`) and
`crates/agent/src/runtime.rs` (directory creation).

### 20. Guest images are downloaded without upstream verification

`download_file` runs `curl -fsSL --connect-timeout 15 --max-time 900` against
recipe URLs and nothing else: no `SHA256SUMS` or `SHA512SUMS` fetch, no
signature, no size cap, and no digest recorded or printed. Two of the three
QEMU recipes and the Firecracker kernel are mutable upstream objects
(`cloud.debian.org/.../daily/latest/`, `cloud-images.ubuntu.com/noble/current/`,
the `spec.ccfc.min` quickstart kernel); only the Alpine minirootfs URL is
version-pinned.

Evidence: `crates/cli/src/image_builder.rs` (`download_file`, the recipe
table).

The published image becomes the root disk of every VM created from it, with the
caller's SSH keys injected, and the operator has no record of what was
downloaded. `docs/guest-images.md` names the rolling URLs and says recipes are
not a lockfile, which is honest about reproducibility but not about integrity.
Pinning a digest for the version-pinned recipes and printing the digest for the
rolling ones would close most of the gap at low cost.

### 21. The agent API is reachable from guest networks when no key is configured

Guests are attached to `tt0` (`10.10.0.0/16`, gateway `10.10.0.1`) and the
agent listens on `0.0.0.0:9100` by default. Guest-initiated traffic to the host
is an `input` path; `--deny-outgoing` only installs a drop in the `forward`
hook, and only `isolated_network` adds the `inet` `host` chain that blocks it.
A guest that is not isolated can therefore reach the agent API on its own
gateway.

Evidence: `crates/agent/src/config.rs` (`listen` default),
`crates/core/src/net.rs` (`setup_bridge`, `setup_nat`, `deny_outgoing`), and
`crates/core/src/net/isolation.rs` (`rules`).

With a key configured the guest gets `401` and this is a reachability note, not
an escalation. Without one — a manual start, which the docs warn about but
which is also the only way to run an agent outside `tt deploy` — it is a full
host control plane on the guest network: create, stop, start, and delete any
VM on that host. `docs/guest-images.md` and README say to keep controller and
agent endpoints on a protected management network and to restrict published
ports, but neither says that the VM bridge itself is part of that reachability,
so "trusted network" is easy to read as "not the guests". Either binding the
default to a management address or stating the bridge case explicitly would
remove the ambiguity.

### 22. Administrator-key plumbing fails silently in three ways

- An empty key is accepted and reported as protection. `--api-key ""` and
  `TT_API_KEY=` are valid values for the CLI parser, `main.rs` prints
  "API key authentication enabled", and the middleware authorizes any request
  whose token equals the empty string, i.e. the header `Authorization: Bearer `
  with a trailing space and no token. `deploy.rs` rejects an empty key, so this
  needs a manual start — but the process should refuse to start rather than
  claim authentication it does not have.
- The controller silently drops the key when it is not a valid header value.
  `agent_client` only inserts the Authorization header `if let Ok(val) =
  HeaderValue::from_str(...)`; a key containing a newline or a non-ASCII
  character produces a keyless client, and every host then reports `Offline`
  with the real cause visible only in the controller's stderr.
- The same helper ends in `reqwest::Client::builder()...build().unwrap()`. It is
  the only `unwrap` on a request path in the controller. In the heartbeat task
  it runs before the first loop iteration, so a panic there kills host and VM
  refresh permanently: hosts stay `Online` forever and no VM state is ever
  reconciled again, with no supervision to restart the task.

Evidence: `crates/ctl/src/config.rs`, `crates/ctl/src/auth.rs`,
`crates/ctl/src/handler.rs` (`agent_client`), and `crates/ctl/src/main.rs`
(heartbeat task).

### 23. Create-time validation and idempotency are asymmetric

Two small mismatches between what the docs promise and what the create path
does:

- Firecracker checks its base image before writing the record
  (`runtime.rs:126`); QEMU does not, and `ImageStore::image_exists` — which
  exists for exactly that purpose — has no caller outside its own tests. A
  direct agent call with a misspelled image therefore writes a `Failed` record
  that holds an IP, host ports, a disk reservation, and a VM slot until an
  operator deletes it, and blocks re-creation under the same id. The
  controller's scheduler usually screens image names first, so this shows up
  for direct agent API use and for hosts whose image catalog is empty.
- The idempotency comparison uses `VmOptions` equality, so `ports` and
  `ssh_keys` are compared as ordered vectors. The documented retry path
  ("repeated creation with the same VM ID and parameters reuses the record,
  rather than allocating a second VM") fails for a request that differs only in
  the order of `ports`, with `VM ID already exists with different creation
  parameters`. Sets, not sequences, are what identify those options.

Evidence: `crates/agent/src/runtime.rs` (`create_vm`, `VmOptions` comparison)
and `crates/core/src/storage/mod.rs` (`image_exists`, unused).

## Low

### 24. API and operator mismatches that do not by themselves lose a guest

- `GET /api/hosts` and `GET /api/status` return `ApiResp::err` with axum's
  default 200 on a database error. Other reads use `response()` and return 500.
  The guides say clients must check both status and `ok`. `DELETE /api/hosts/{id}`
  of an unknown id is also 200. Idempotent success is documented for
  environments, not for hosts.
- `constant_time_eq` returns immediately when the lengths differ. Equal-length
  comparison is constant-time. The comment overclaims. Bearer scheme comparison
  is case-sensitive.
- Fleet totals add every host, including `Offline`. Those hosts are not
  schedulable, so the dashboard overstates free capacity. Placement among
  fitting hosts is `(not ZFS, mem_free)` only. There is no per-host VM cap or
  per-owner share. README does not claim fairness. Owner is stored and never
  authorized, which matches the guides.
- Placement diagnostics can name the wrong cause. The capability-specific
  message requires that no online host satisfies the capability set at all;
  when one host advertises the capabilities but not the engine, and another has
  the engine but not the capabilities, the operator is told there are not
  enough resources. The image-catalog case is only reported when a host passes
  every other test.
- The dashboard HTML is public, as documented, and has no framing policy. The
  API key is in `sessionStorage`, not a cookie. A framed page can still use a
  key the operator already entered. Create and add-host are not behind
  `confirm()`.
- QEMU waits about 10 seconds (`100 * 100ms`) after `system_powerdown`, then
  terminates. The lifecycle section only says QEMU tries guest shutdown.
  Firecracker's 30-second wait is documented and matches the code.
- Firecracker boot failure says to inspect the VM console log. The machine
  config has no `logger` and no serial device. `{RUN_DIR}/fc-{id}.log` is
  jailer and VMM stderr, not the guest console.
- QEMU disk selection uses the first `.qcow2` returned by `read_dir` when a
  clone directory contains more than one. Restart can boot a different image
  than create did.
- Agent stderr diagnostics are all the API keeps. `Host` has no error or
  last-error field, so a host that is offline because of a key mismatch, a DNS
  failure, or a timeout looks identical to one that is simply down.
- `ApiResp` parsing discards the HTTP status: when an agent or controller
  returns a non-envelope body, the caller reports a decode error rather than
  the status code that would identify it.
- Default agent and controller listeners are unauthenticated if `--api-key` is
  unset. That is warned and documented. It is still the default, and local
  deploy is what turns the key on. A manual start that omits the key is an
  open root control plane on `0.0.0.0` (finding 21 covers the guest-network
  path to it).
- `tt image create` treats any existing path as a finished image: a zero-byte
  or truncated file at the destination is reported as already present, and the
  next `env create` fails later with an agent-side error. `qemu-img info` is
  already used two lines below to inspect the same path.
- Host tool discovery is `which` on `PATH`, and the generated unit sets no
  `PATH`. A deployed agent normally inherits a usable `PATH`, but engine
  detection silently reports nothing when it does not, which presents as "no
  online host supports engine=qemu" rather than as a missing dependency.

## Unverified residual risks

These are not findings; each needs one live check before it can be graded.
They are recorded so the next validation run can close them.

- **QEMU `-name` and `/proc/PID/cmdline`.** `-name <id>` also sets the QEMU
  process title, and some QEMU builds rewrite the argv region to do so. If that
  destroyed the exact id element, `process_matches` would never match and
  `stop` would return success without stopping anything (finding 13). The
  validation records show QEMU stop and restart both succeeding, which argues
  the marker does match; the evidence is indirect, because a mismatched marker
  would have surfaced as a failed restart rather than as a failed stop.
- **Isolation `oifname` versus routed DNAT traffic.**
  `bridge {t} peers oifname "<tap>" drop` is intended to stop L2 peer traffic.
  If the bridge forward hook also matches locally routed frames on a host with
  `br_netfilter` loaded, inbound published-port traffic for an isolated guest
  could be dropped. Published ports for isolated guests were exercised in
  validation, which implies it does not, and a targeted test (isolated guest,
  published port, external client) would settle it.
- **Docker coexistence** (finding 6). The claim that a Docker-managed forward
  policy breaks VM egress on the same host was reasoned from the rule layout,
  not reproduced.
- **QEMU CPU model and entropy.** `build_cmd` passes no `-cpu` model and no
  `virtio-rng` device, while the Firecracker path received explicit entropy
  work. Cloud images boot in validation, so this is a note about
  feature/entropy quality rather than a known failure.
- **`ip_forward` write without readback.** `setup_bridge` writes `1` and
  ignores whether the value took effect (read-only `/proc` in a container).
  Unverified, and the failure would look like "no egress".

## Documentation and evidence drift

The maintained documentation is unusually accurate for a project at this stage.
Everything below was checked mechanically against the tree, and the majority of
what was checked is correct.

Verified correct: every internal link and anchor checked across the Markdown
resolves; the default lifetime (21600), the 50-host and 1000-VM limits, the port
pool (20000–65535), the QEMU disk default (40960), the 128 MiB Firecracker
headroom, the guest-configuration limits (32 files, 64 KiB, 512 KiB on the CLI),
the 4 MiB configuration drive, the 30-second Firecracker shutdown wait, the
60-second expiry and 15-second refresh loops, both route tables, every README
quick-start command and flag, and the recipe names and URLs all match the code.
Each dated report's test count matches the revision it cites, its cited
revisions and parent commits exist and are correctly described, and no
credential, key material, private hostname, or private inventory appears in any
tracked file.

Drift that should be corrected:

- **`docs/rest-api.md` reservation sentence.** "Firecracker reserves its
  requested rootfs size (or the image size when omitted) plus 4 MiB when a
  configuration drive is present" describes the agent, not the scheduler that
  makes the placement decision (finding 10). The same sentence is the one an
  operator would use to reason about a fleet's disk budget.
- **Docker/Podman selection is described three ways.** `docs/deployment.md`
  says the agent selects Docker "when its binary is installed" and warns that a
  broken Docker binary next to Podman "will select Docker";
  `docs/guest-images.md` says "if its binary is installed, otherwise Podman";
  `docs/compatibility.md` says "Alternate runtime selected when Docker binary is
  absent". The engine actually requires `docker --version` to succeed
  (`docker.rs:17-28`), while the *agent* advertises the engine when `which`
  finds the binary (`runtime.rs:682-684`). A present-but-broken Docker binary
  therefore selects Podman for containers while the host still advertises
  Docker to the scheduler — the opposite of what `deployment.md` warns about,
  and a case where the advertisement and the engine disagree.
- **The Firecracker UID/GID range.** `docs/deployment.md` documents
  `100000–165535`; the formula is `100_000 + c * 256 + d` over the allocated
  address octets, which yields `100002–165534`. The published range is a safe
  superset for the stated purpose (reserving host accounts) but neither
  endpoint is ever assigned.
- **"This run" in `docs/compatibility.md`.** The phrase is used four times to
  mean the 2026-09-24 live validation, while the same document now cites seven
  dated runs; for at least one row (`QEMU + zvol`) the newest run also did not
  cover the case. Naming the run removes the ambiguity.
- **Amended dated reports still cite their original revision.**
  `docs/live-validation-2026-09-24.md` states it records the code "committed as
  `8998db7`", but the file as it exists today was edited afterwards (in
  `8775fdb`, alongside the FreeBSD removal) and no longer matches what that
  commit contains. The edits are truthful; the citation is now imprecise. A
  one-line "amended in <commit>" restores the one-description rule the
  repository sets for itself.
- **Undocumented flag.** `--dup` is the only `env create` option with no
  mention in any guide; the CLI design deliberately delegates options to
  `--help`, so this is minor, but it is also the flag the dashboard's create
  form has no equivalent for.
- **Tool list.** `docs/compatibility.md` names `mkfs.ext4` and `debugfs` as the
  e2fsprogs tools the tests need; `crates/core/src/storage/file.rs` also calls
  `dumpe2fs`.
- **Report content beyond the evidence.** Two dated reports name an internal
  application image and a deployment's service topology
  (`firecracker-zvol-validation-2026-09-24.md`,
  `resource-sizing-validation-2026-09-24.md`). Neither is a credential, an
  address, or a host inventory, so no stated rule is violated; substituting "a
  task-owned application image" would lose no evidence and disclose less.

## What matches the stated design

These are not findings. They are the boundaries the review treated as
intentional, because the guides already say so and the code follows them:

- One controller, SQLite on each side, no message bus, no guest migration, no
  automatic image distribution, no cross-host private network.
- Owner is a label. API authentication is one optional shared bearer token.
  Services do not terminate TLS.
- Stop keeps disks and drops VM memory. Delete is destructive. A timeout does
  not prove that create failed. The controller does not HTTP-retry create.
- Engine commands are argv arrays, not a shell. Environment, host, and image
  names are constrained. Guest configuration contents are not returned by the
  query API.
- A controller shutdown aborts `finish_creation` the same way a crash does.
  The guides already say a crash leaves creation failed and does not
  automatically recreate. That path was checked and is not a separate finding.
- Capability flags keep new Firecracker and isolation features off old agents
  (their *truthfulness* is finding 11).
- Dated validation reports describe their own revision and limits. They do not
  contradict current defaults, and this audit does not extend them.

Checked and found sound, recorded so later reviews do not re-derive them:

- The creation plan is persisted atomically before any agent call
  (`put_environment` runs inside a transaction, `db.rs:169-180`), duplicate
  environment names are rejected under the mutation lock, and a retried create
  after a client timeout gets `409` rather than a second environment.
- Agent-side create is idempotent for identical parameters, reserves identity,
  ports, and resources before the first side effect (`runtime.rs:188`), and
  writes `Failed` with the retained partial resources on any error.
- Delete and expiry write intent before acting, remove VM rows only after
  confirmed agent success, and never report success for a partial delete.
- Reconcile repairs both crash directions (`Creating` + live engine becomes
  running; `Creating` + dead becomes failed; engine `Stopped` wins) and
  deliberately never turns `Failed` into `Stopped`, so a failed record cannot
  silently start.
- No lock is held across an `await` in the controller: every handler future is
  `Send`, which the compiler enforces, and all database access is
  statement-scoped behind one mutex.
- Host-port allocation cannot double-allocate inside one agent (runtime mutex
  plus a used set built from every record), and exhaustion fails explicitly.
- Firecracker sandbox limits are real and enforced: cgroup v2 with `cpu.max`,
  `memory.max = guest + 128 MiB`, `memory.swap.max=0`, `pids.max`, a per-VM
  non-root UID derived from the guest address, hard links (never copies of
  writable roots), `0400`/`0600` modes and a read-only configuration drive, and
  a hard refusal when cgroup v2 is absent — there is no unjailed fallback.
- Guest configuration files are validated for name and size, published
  atomically, created `0400` in staging, and exposed only as a SHA-256 digest;
  a create request cannot smuggle paths or echo secrets into diagnostics.
- `nft` rules are delivered through `nft -f -` from a temp file; every
  interpolated value is a number, a hash-derived tap name, or a validated
  `10.10.x.y` address that rejects embedded shell text.
- Disk shrink is rejected on every path (file, zvol, ext4 growth) before any
  mutation, and the base image is never resized or written.
- Every host command is bounded (300 s default, tighter for `nft`, `docker
  inspect`, and `curl`), output is redirected to temp files to make pipe
  deadlock impossible, and a timeout kills and reaps the child.
- Outside tests there are no `unwrap`/`expect` on request paths in the agent,
  engines, networking, or storage; reads use a separate read-only SQLite
  connection under WAL with a busy timeout.
- `validate_name`, `validate_image`, and `validate_vm_options` reject path
  traversal, option injection, and unsupported engine combinations before any
  side effect, on both the controller and the agent.

## Fix order

The findings are coupled. A useful sequence is:

1. Stop disclosing the fleet key. Do not put it in SSH argv, deploy stdout, or
   the recommended `tt config --api-key` line. Do not send it to an address
   until that address is bound to an expected host id. Teach registration to
   refuse an id or address that already belongs to a different peer. Refuse to
   start an agent or controller on an empty key, and fail loudly when a key
   cannot be used as a header.
2. Stop `chown -R` of the service home on upgrade, or exclude jailer inodes.
   Preserve operator unit drop-ins, including dataset mount guards.
3. Give the delete path an escape hatch: an unreadable PID file must not stop
   cleanup, a paused VMM must still be killable, and nft or tap errors must not
   skip disk removal. Add an explicit detach for a host that will not answer,
   so expiry of other environments is not behind that timeout and the VM budget
   can be released.
4. Make recovery retryable: re-run network restore and tap ownership from
   reconcile, recreate a missing Firecracker tap with the jail UID instead of
   uid 0, and do not delete the old tap until the replacement exists. Clear VM
   errors that no longer describe reality, so a running guest can return to
   `active` without a stop.
5. Give tap, jail, cgroup, and isolation names a stable derivation. Repair
   `tt0` instead of treating the name as proof of configuration. Decide an
   explicit Docker coexistence rule before advertising both engines on one host.
   Fail loudly when `sandbox.json` is missing instead of falling back to an
   identity that cannot match.
6. Reserve, on the controller, the same disk and memory the agent will reserve.
   Do not replace `host.resource` from a snapshot that omits a VM the
   controller still tracks, and log VMs the controller does not know about.
   Derive advertised capabilities from host facts rather than platform.
7. Replace the predictable `/tmp` staging paths (image build and deploy) with
   private directories created `O_EXCL`, verify the staged binaries, and refuse
   `--ssh-key` values that are not a single OpenSSH public key before the
   request is sent. Verify or pin the digests of downloaded guest images.
8. Make the schema's recovery primitives tolerate one bad row, so a corrupt
   record cannot disable the delete path for every other VM.

Until those land, repeating `tt deploy` on a host with running Firecracker
guests, and registering a host address that has not been verified, are the two
operations most likely to violate the recovery and confidentiality rules the
guides already state. A stuck record with no reachable host is the most likely
to require manual database surgery.
