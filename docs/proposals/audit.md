# Proposal and adjacent-contract review

Implementation follow-up: the ZFS/reflink disk-backup paths are now implemented;
the [maintained guide](../disk-backup.md) owns current behavior. The dated review
below describes its stated earlier baseline and is retained as design context.
Scoped [capability reports](../capabilities.md) and matrix-driven dashboard
controls are also implemented, including Firecracker SSH keys and Jail option
restrictions. Those later changes supersede the UI limitations recorded below.
Shutdown/OOM diagnostic recommendations remain separate work.

This is a dated design review of the documents in this directory and of the
current lifecycle contracts those documents depend on. It is not a maintained
behavior guide, and it is not the defect registry. Confirmed code defects
belong in the [audit registry](../audit.md). Implemented behavior remains in
the guides indexed by [../README.md](../README.md).

Reviewed on 2026-09-29 against `56302dc`. The Rust tree matches the disk-backup
proposal's code baseline `0547f3c`; the two later commits are the proposal
documents themselves. No VM, storage, or restore experiment was run. `zfs` and
`qemu-img` were not executed. Claims below come from the source and the
maintained guides.

Follow-up on 2026-09-29 against `df3281f`: source checks and upstream cloud-init
semantics refined the SSH-dispatch and timeout conclusions below. The related
proposals now incorporate those constraints and identify the remaining decisions.
No new runtime validation is implied by these documentation refinements.

This is not a fresh review of every subsystem already dispositioned in the
registry. Coverage is the two proposals, the integration points they cite, and
the stop, SSH, dashboard, lock, and timeout paths those proposals assume.

## Disposition

| Document | Code status at the reviewed baseline | Review result |
| --- | --- | --- |
| [VM disk backup](vm-disk-backup.md) | Not implemented | Draft is internally consistent with the code it describes. Section 13 decisions are still open. Do not treat the draft as authorization to implement. |
| [VM SSH and recovery evidence](vm-access-and-recovery.md) | Initial SSH is implemented on the API, CLI, and agent path. Exit/OOM history is not. | Follow-up specifies candidate evidence sources and their limits; none is a claim of implemented diagnostic history or full dashboard support. |

<a id="disk-backup-is-not-implemented"></a>

## Disk backup was not implemented at the reviewed baseline

Searches of the Rust tree found no backup route, CLI subcommand, persisted
backup field, capability string, or `--enable-disk-backup`. Controller routes
stop at VM get, resize, and environment stop/start. Agent routes stop at
create, get, delete, stop, start, and resize. Relevant existing `Vm` fields include
`ssh`, `pending_resources`, and `error`, with no backup state. Schema versions are
still agent v5 and controller v4, as the draft describes.

Existing ZFS `@ttsnap` clones and `cp --reflink=auto` are provisioning paths.
[Guest images](../guest-images.md) already says ordinary stop/start is not a
snapshot or suspend-to-disk. Those provisioning paths must not be reused unchanged
as the backup implementation: provisioning reflink can fall back to a full copy,
and `@ttsnap` does not capture later guest writes. Strict reflink and snapshots of
the correct runtime volume remain candidate backup primitives.

The draft's description of current integration points matched the code that
was re-read: zvol runtime clones and `zfs destroy -r` of the clone path,
QEMU's writable root plus read-only seed, Firecracker's raw root plus optional
config disk, jail hard links, Docker bypassing `ImageStore`, WAL with
`synchronous=NORMAL`, and the 300-second command bound.

## SSH is implemented; recovery evidence is not

The maintained [SSH guide](../ssh.md) matches the code that was re-read:

- Creation accepts public keys, `ssh.user`, and `ssh.sudo`. The CLI flags are
  `--ssh-key`, `--ssh-user`, and `--ssh-sudo`. Docker has no managed bootstrap.
- The agent generates an Ed25519 host identity once, stores only the public
  part in VM metadata, and writes the seed before the first external disk side
  effect. The guest script exits when `/var/lib/ttstack/ssh-initialized`
  exists, so a later cold start does not restore a key the guest removed.
- QEMU reads that retained seed into a new NoCloud ISO on every cold start.
  Prepared Firecracker images are checked for bootstrap version `1`, and the
  seed is placed on the configuration drive as `ttstack-ssh.sh`.
- Public ingress is `--ssh-public-address`, with the ingress namespace and
  target required together. Readiness is an SSH banner plus, until
  initialization is confirmed, an `ssh-keyscan` match of the initial host key.
  `ssh.checked_at` is that observation time, not an exit or OOM record.

The dashboard does not expose this contract. Its SSH field is disabled unless
the engine is QEMU, the create body sends keys only for QEMU, per-VM key lists
are always empty, and there is no account or sudo control. The control is
disabled rather than silently dropped, so this is an incomplete UI, not a
false success. The implemented claim applies to the API, CLI, and agent; the
revised proposal now makes the dashboard limitation explicit.

Host-controlled stop/start already confirms the owned VMM has exited before a
cold start: QEMU and Firecracker refuse `create` when the recorded process
still matches. Stop retains the disk. What the recovery proposal still asks
for is absent: no persisted exit status, no OOM counter, no diagnostic
history, and no API field that distinguishes a graceful shutdown from
`terminate()`. Firecracker logs the fallback. QEMU does not. The
[2026-09-28 lifecycle report](../validation/lifecycle-corrections-2026-09-28.md)
already recorded a 10.062-second QEMU stop that could not be classified.
Firecracker sets a per-VM memory cgroup; nothing reads `memory.events`. QEMU
is not placed in a TTstack cgroup, so a Firecracker-only OOM read would not
be fleet-wide evidence.

## Review notes

These are recommendations and implementation constraints. They are not Open
registry entries: the shutdown split is already documented, and the backup
notes describe a draft that has no code yet.

### Guest shutdown budgets need a separate decision

| Stage | QEMU | Firecracker | Docker/Podman |
| --- | --- | --- | --- |
| Ask the guest to exit | `system_powerdown`, then poll about 10 seconds (`100 × 100 ms`) | x86_64: `SendCtrlAltDel`, then poll up to 30 seconds. A paused guest is resumed first. | `stop -t 10` |
| Then kill the runtime | `SIGTERM` up to 5 seconds, then `SIGKILL` up to 5 seconds | Same `terminate()` | The runtime kills PID 1 when `-t` expires |

[REST API](../rest-api.md#lifecycle-and-recovery) documents the 10-second and
30-second guest windows separately. Cooperative guests return as soon as the
process is gone; a guest that does not exit within the window reaches escalation.

Unifying the QEMU guest-shutdown window to 30 seconds is a candidate, not a
demonstrated requirement or prerequisite for backup. The unclassified 10.062-second
stop establishes missing evidence, not proof that 10 seconds caused truncation or
that 30 seconds would fix it. First distinguish request, escalation, and observed
exit and validate representative guests. Do not stretch `terminate()` to 30 seconds
or change Docker's `-t 10` in the same change; that signal goes to container PID 1,
not through ACPI or Ctrl-Alt-Del. The
[recovery proposal](vm-access-and-recovery.md#shutdown-behavior-and-evidence)
owns the follow-up recommendation and its acceptance cases.

The cost is concrete. Agent mutations take the host-wide runtime mutex and
keep it for the whole stop, including the guest wait. A hung QEMU stop would
block other mutations on that agent for about 30 seconds plus up to 10
seconds of `terminate()`, instead of about 10 plus 10. Firecracker already
has that cost. A change must update the REST and guest-image guides and keep
the early return for a guest that exits promptly.

Paused QEMU is a related gap. Firecracker resumes a paused guest before
requesting shutdown. QEMU sends `system_powerdown` without `cont`. A paused
guest is not running, so it cannot handle that request during the wait, and
the stop falls through to `terminate()`. The REST guide documents only the
Firecracker resume. Either resume a paused QEMU before the power-button
request, or document that a paused QEMU stop is a forced termination.

### Backup work must not copy today's stop path

The draft already requires slow storage work to leave the fleet and SQLite
locks, and says an HTTP timeout does not cancel an admitted operation. The
current call path makes that easy to miss:

- `mutate` holds the agent runtime mutex until the operation returns, and
  continues after the caller disconnects.
- Controller mutation clients use 360 seconds. The CLI mutation timeout is
  600 seconds. The command helper kills its direct child at 300 seconds.
  Several sub-300-second commands can still outlive the controller client.
  A lost controller response is then an unknown outcome, not proof that the
  agent stopped.
- Environment expiry and delete use the same 360-second client and the
  per-environment operation lock. A backup that releases the agent mutex
  during storage work must still enforce the persisted exclusion against
  direct agent calls, or expiry can delete the VM while a candidate exists.

Budget the whole foreground attempt against the waiting limits, not just each
command. The 360-second HTTP timeout is not a safe storage-transaction deadline:
expiry of a request must not clear intent, release reservations, or make a disk
available while a writer's result is unresolved. The draft now distinguishes
admission/execution/completion, HTTP waiting, backend step deadlines, and deferred
cleanup in [worker supervision](vm-disk-backup.md#64-durability-and-worker-supervision).
It retains inspection and same-operation retry after a waiting timeout.

### Storage placement is already constrained

- QEMU `resolve_disk` rejects a clone directory that contains more than one
  `.qcow2` file. A backup file inside that directory makes the active disk
  ambiguous and blocks start. The draft's private backup directory is
  required, not optional.
- Image listing skips names that start with `.` or `clone-`, but only in the
  image catalog directory. Runtime disk resolution is a separate operation;
  catalog filtering cannot protect it from a misplaced backup file. With distinct
  configured roots, a runtime sibling is not scanned as a catalog image at all.
- Firecracker stages `rootfs.ext4` into the jail with a hard link, then
  rebuilds that jail on the next cold start. Replacing the active inode while
  the stopped jail still links the old inode is safe only if start keeps
  using that cleanup. Do not rename the backup file onto the active path.
- The configuration drive and kernel sit beside the root volume, not inside
  it. A rootfs snapshot or reflink does not capture them. Recursive snapshot
  of the Firecracker clone dataset would. The draft's non-recursive target
  matches this layout; an implementation that snapshots the parent would
  capture other files and make rollback too wide.
- VM deletion already runs `zfs destroy -r` on the clone path. That removes
  descendants of that clone, which is the desired fate of owned backup
  snapshots. It must not be widened to the runtime parent or changed to
  `-R` to clear a foreign clone or hold. Whether today's `-r` fails closed
  on an external clone of a future backup snapshot was not executed. That
  case belongs in the draft's foreign-artifact test before the backend is
  advertised.

### Restore interacts with the SSH marker

The guest completion marker, actual SSH keys/configuration, and cloud-init records
are disk state. The retained host seed and advertised initial identity are separate
host state; the guest may also hold copies of seed material. Restore does not
reconcile these automatically.

The original inference that a pre-marker restore necessarily reruns the seed was
too broad. [QEMU](../../crates/core/src/engine/qemu.rs) uses a stable instance ID
and cloud-init `runcmd`; its once-per-instance execution records also govern
dispatch. Prepared Firecracker init must invoke its script before the marker can
act as a guard. Marker absence alone proves neither path ran. A marker-present
restore preserves captured guest keys when the bootstrap guard is honored, while
the host still advertises the original identity.

The [revised backup proposal](vm-disk-backup.md#65-ssh-observations-after-disk-restore)
cites upstream dispatch semantics and defines a proposed restore-specific
observation reset. In particular, the agent's `ssh.initialized` is not the guest
marker: retaining a later `true` value can skip fresh identity verification.
Conservative re-verification must report a changed guest key without rewriting
it or misreporting a successful disk restore as a storage failure. Ordinary
restart and disk rollback remain distinct behaviors.

### Idempotency is a new protocol

Resize retries the same persisted target. Backup restore must not reapply
after a later guest write, so the draft's operation id and revision are
justified. Nothing in the current API forwards `Idempotency-Key` or
`If-Match`. Those headers have to be added on the controller, agent, and CLI
together. Reusing "retry the same JSON body" would repeat a completed
restore.

Section 13 of the backup draft is still open, including the host admission
flag, manual stopped-only trigger, no recovery after VM deletion, the
retirement bound, resize-versus-backup, restore-time SSH observations, and WAL/FULL.
Full-copy backup is not an open choice. Schema numbers must be chosen at
implementation time; the agent still rejects every schema other than v5 and has
no in-place migration.

## What not to do next

Do not implement backup from this review. Decide section 13 first, then
implement one backend with the failure tests the draft already lists.

A shutdown-budget change is a separate lifecycle change. It updates the
REST and guest-image guides, keeps the early return, and does not smuggle
in Docker's stop timeout or a longer `terminate()`.

Do not copy this review into the maintained guides, and do not promote its
recommendations to support claims.

Return to the [documentation index](../README.md).
