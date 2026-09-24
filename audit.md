# TTstack audit, 2026-09-25

Point-in-time review of architecture, implementation, and documentation at
revision `83b7e95` (`master`). This is not a maintained behavior guide and not
a live-host validation. Findings below were checked against the current tree.
Where a failure depends on host firewall state or a local attacker, that
precondition is stated; it was not reproduced on a running fleet.

Severity is the operational consequence if the documented install and upgrade
path is followed. Documented scope limits (one controller, no image
distribution, no migration, no high availability, owner as a label, HTTP
without TLS) are not listed as defects.

## Summary

The lifecycle model is careful in the places the tests cover: a create plan is
stored before agent work, creates are not HTTP-retried, and a missing agent
does not by itself delete tracking rows. The unreasonable parts are around
that model.

The control plane is one fleet-wide lock held across multi-minute agent calls,
so one unreachable host stalls unrelated cleanup. Host identity is an upsert of
whatever the agent reports, and the shared administrator key is sent to that
address before the body is trusted. Cleanup is a single fallible pipeline:
a firewall or resume error prevents disk and process reclaim, and the retry
repeats the same prefix. Scheduling, agent admission, and engine limits do not
use the same disk and memory numbers. Deployment rewrites service units and
recursively changes ownership under the service home, which undoes Firecracker
jail ownership on the documented upgrade path. Several maintained sentences
describe the agent-side or intended contract as if the controller, dashboard,
and upgrade path implemented it.

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

`docs/rest-api.md` says mutations are serialized and that cleanup needs time
for queued operations. It does not say one unresponsive agent blocks unrelated
deletes, or that a dead host cannot be dropped from tracking without a
successful agent response.

### 2. Re-registering a host id retargets the fleet, and registration discloses the admin key

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

### 3. Deployment puts the fleet key on a command line, then recursively chowns guest disks

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

### 4. Paused Firecracker cannot be force-stopped, and delete is not best-effort

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

The retention rule (do not forget a VM whose cleanup failed) is sound. Applying
it to an unrelated earlier step is not: a missing nft table should not pin a
disk, and a failed resume should not pin a process that `SIGKILL` can reap.

### 5. `tt0` is treated as healthy if the name exists, and VM forwarding is not integrated with Docker

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

### 6. Root image build mounts a predictable `/tmp` path

`tt image create` for Firecracker downloads the kernel, then creates
`/tmp/tt-image-{pid}` with `create_dir_all` and loop-mounts an ext4 image
there. `create_dir_all` succeeds if that path already exists as a directory,
including through a symlink. `mount` follows the symlink. The kernel download
happens first, so the PID is visible for a long time. The documented recipe
command runs as root. A local user can point that path at another directory
and have root mount and extract an archive onto it. The Alpine tarball is
extracted with `tar` as root and is not checksum-verified. The guest-image
guide says recipes are not a lockfile. It does not mention this race.

Evidence: `crates/cli/src/image_builder.rs` (`create_firecracker`, `tempdir`)
and `docs/guest-images.md` (recipe command).

### 7. Network object names are not stable across a toolchain upgrade

`tap_name` is `DefaultHasher` of the VM id, formatted as `tt-` plus 12 hex
digits. Rust does not guarantee that hasher across compiler releases. The same
function supplies the tap device, the Firecracker jailer id, the cgroup path,
and the isolation table name.

Evidence: `crates/core/src/net.rs` (`tap_name`) and
`crates/core/src/engine/firecracker/sandbox.rs` (`prepare`).

Rebuilding with a newer compiler, which is a normal way to follow the
documented upgrade path, can change those names. Cleanup then targets the new name and leaves the old tap,
nft table, and cgroup behind. A running Firecracker sandbox JSON still points
at the old jail, while recovery creates a new tap the running VMM is not
attached to. This is an identity bug, not a style choice: these names are
on-host state.

## Medium

### 8. Controller placement does not reserve what the agent reserves

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

### 9. `failed` means too many different things, and some errors never clear

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

Evidence: `crates/ctl/src/handler.rs` (`change_environment`, `update_env_state`,
`apply_host_snapshot`), `crates/agent/src/runtime.rs` (`reconcile`), and
`crates/cli/src/main.rs` (create poll).

`docs/rest-api.md` says an offline agent means last snapshots, not proof the
guests stopped, and that partial failures remain visible in `env.error`, VM
`error`, and `warnings`. VM `error` and detail warnings do. List-level
`env.error` often does not, and `failed` is also what a transient refresh
produces.

### 10. Process identity is a pidfile plus a cmdline string

`terminate` returns success if `/proc/PID/cmdline` does not contain the marker.
The Firecracker boot path already notes that cmdline can be empty during
`exec` and that a missing marker is not proof of exit. Stop and destroy use the
opposite rule. A stop that races jailer or QEMU startup can report success,
after which the controller releases CPU and memory while the VMM is still
coming up. An invalid pidfile is the other failure: parse failure makes stop
or destroy return an error, so cleanup never reaches disk removal. There is no
pidfd or start-time check.

QEMU's marker is the VM id as a whole argv element (`-name` value). Controller
VM ids are UUIDs, so an accidental match against an unrelated process is
unlikely on the normal path. The false "already dead" result during `exec` does
not depend on that.

Evidence: `crates/core/src/engine/mod.rs` (`process_matches`, `terminate`),
`crates/core/src/engine/qemu.rs` (`stop`), and
`crates/core/src/engine/firecracker.rs` (`stop`, `wait_for_boot`).

### 11. Published VM ports are not reserved, and NAT state is not fully reclaimed

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

### 12. Firecracker tap recovery does not restore jail ownership

`prepare_jailed_tap` is only used before launching a stopped Firecracker. The
comment in `net.rs` says it is never used during live recovery. Agent startup,
for every VM still recorded as running or paused, calls `restore_network`, and
that calls `create_tap`. `create_tap_owned` creates a missing device as uid 0
when no owner is passed, and if the device already exists it does not change
the owner. A tap that disappeared while the jailed VMM is still running is
therefore recreated as root. The jailed process cannot open it. Recovery does
not restart that VMM, so the guest stays network-dead. The stored error is
`network recovery failed: ...`, which finding 9 does not clear when the process
is later observed running.

On a cold start the order is the opposite, and it is not atomic.
`prepare_jailed_tap` deletes the tap before `ip tuntap add` creates the
replacement. If that add fails, the tap is gone. `start_vm` has already
persisted `Failed`, and start refuses any state other than stopped or paused,
so the failed start is not retried. The operator has to delete and recreate.

Evidence: `crates/core/src/net.rs` (`create_tap_owned`, `prepare_jailed_tap`),
`crates/core/src/engine/firecracker/sandbox.rs` (`prepare`), and
`crates/agent/src/runtime.rs` (startup recovery loop, `restore_network`,
`start_vm`).

### 13. One bad JSON row disables list, refresh, and delete

Hosts, environments, and VMs are JSON blobs. `query_all` aborts the entire
result on the first deserialize error. Controller refresh returns immediately
if `list_hosts` fails. Delete needs that list before it can clean up. Agent
`load_all_vms` has the same all-or-nothing behavior, so one unknown engine
value prevents cleanup of every other VM, including `Deleting` retries. The
in-tree tests lock this in.

`PRAGMA foreign_keys=ON` does nothing: the schema has no foreign keys.
Environment, VM, and host rows can diverge; integrity is only in the handlers.
The agent writer connection has no `busy_timeout`, and nothing takes a lockfile,
so two agent processes can open the same database and both program taps and nft.

Evidence: `crates/ctl/src/db.rs` (`query_all`) and
`crates/agent/src/runtime.rs` (`load_all_vms`).

This is a reasonable prototype schema. It is not a reasonable recovery schema:
the operation that must still work when a row is corrupt is delete.

### 14. CLI and dashboard defaults disagree with the API the guides document

- Create uses the client's 60-second timeout. The ten-minute poll starts only
  after POST returns. The controller holds the fleet lock and refreshes hosts
  before it returns 202, and another stop or delete can hold that lock for up
  to 360 seconds per VM. A create can therefore fail at 60 seconds while the
  server is still accepting it, so the documented poll never starts. README and
  `docs/rest-api.md` say the CLI waits up to ten minutes. The error string does
  say to inspect before retrying. Delete and stop use a 600-second client
  timeout, which is still shorter than several sequential 360-second agent
  calls. Dropping the client does not abort the controller task.
- `tt --server` replaces the address and drops the key stored in `~/.ttconfig`
  unless `--api-key` or `TT_API_KEY` is also set. The flag help says the address
  overrides the file. It does not say the key is ignored. A create with
  `--guest-config` then goes to whatever answers, with no bearer token.
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
- `~/.ttconfig` is written with `std::fs::write`, then `chmod 0600`, and a
  failed chmod is ignored. If `HOME` is unset, the key is written to
  `./.ttconfig`.

Evidence: `crates/cli/src/client.rs`, `crates/cli/src/main.rs` (server
selection, SSH key resolution, create poll, delete), and
`crates/ctl/src/web.rs` (create form and `createEnv`).

### 15. Deploy configuration is interpolated into a root shell, and unit edits are not preserved

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

Evidence: `crates/cli/src/deploy.rs` (`remote_setup_script`, `systemd_unit`,
`deploy_local`).

### 16. File-backed QEMU disks are not mode-restricted

`clone_image` is `cp -a` and does not chmod the clone. Runtime and image
directories are created with `create_dir_all`, so their mode is the process
umask (typically `0755` for a root agent). Cloud images are commonly `0644`.
Any local account that can traverse `/home/ttstack` can then read the guest
disk for the life of the VM. Firecracker later chmods the hard-linked rootfs
to `0600`, which does not cover QEMU images. Finding 3 then reowns the
Firecracker inode to the controller user on the next deploy.

Evidence: `crates/core/src/storage/file.rs` (`clone_image`) and
`crates/agent/src/runtime.rs` (directory creation).

## Low

### 17. API and operator mismatches that do not by themselves lose a guest

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
- Default agent and controller listeners are unauthenticated if `--api-key` is
  unset. That is warned and documented. It is still the default, and local
  deploy is what turns the key on. A manual start that omits the key is an
  open root control plane on `0.0.0.0`.

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
- Capability flags keep new Firecracker and isolation features off old agents.
- Dated validation reports describe their own revision and limits. They do not
  contradict current defaults, and this audit does not extend them.

## Fix order

The findings are coupled. A useful sequence is:

1. Stop disclosing the fleet key. Do not put it in SSH argv, deploy stdout, or
   the recommended `tt config --api-key` line. Do not send it to an address
   until that address is bound to an expected host id. Teach registration to
   refuse an id or address that already belongs to a different peer.
2. Stop `chown -R` of the service home on upgrade, or exclude jailer inodes.
   Preserve operator unit drop-ins, including dataset mount guards.
3. Make cleanup best-effort after the VMM is dead: resume failure must still
   reach `SIGKILL`; nft or tap errors must not skip disk removal. Add an
   explicit detach for a host that will not answer, so expiry of other
   environments is not behind that timeout.
4. Give tap, jail, cgroup, and isolation names a stable derivation. Repair
   `tt0` instead of treating the name as proof of configuration. On recovery,
   recreate a missing Firecracker tap with the jail UID instead of uid 0, and
   do not delete the old tap until the replacement exists. Decide an explicit
   Docker coexistence rule before advertising both engines on one host.
5. Reserve, on the controller, the same disk and memory the agent will reserve.
   Do not replace `host.resource` from a snapshot that omits a VM the controller
   still tracks.
6. Replace the predictable `/tmp/tt-image-{pid}` mount with a private directory
   created `O_EXCL`/`mkdtemp`, and refuse `--ssh-key` values that are not a
   single OpenSSH public key before the request is sent.

Until those land, repeating `tt deploy` on a host with running Firecracker
guests, and registering a host address that has not been verified, are the two
operations most likely to violate the recovery and confidentiality rules the
guides already state.
