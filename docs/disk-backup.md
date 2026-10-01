# VM disk backup

TTstack can retain one current root-disk recovery point for a stopped QEMU or
Firecracker VM, or an experimental FreeBSD bhyve VM on zvol storage. Backup
admission is **disabled by default**. This is a local disk
rollback facility: it does not save memory or recover a deleted VM or failed host.

Use [Enable and use](#enable-and-use) for CLI operations, [Supported storage](#supported-storage)
for eligibility, and [API and exact retries](#api-and-exact-retries) after an unknown
outcome. Component ownership is described in [architecture](architecture.md).

## Enable and use

Start an eligible agent with `--enable-disk-backup`. Distributed deployment exposes
the equivalent `enable_disk_backup = true` agent setting. Enabling it does not
create a backup automatically. There is no timer or stop/start backup hook.

```sh
tt env stop demo
tt env show demo
# Use the VM ID from the environment, not its environment name.
tt env backup create VM_ID
tt env backup show VM_ID

# Overwrites the current disk; obtain GENERATION from backup show.
tt env backup restore VM_ID --generation GENERATION
tt env start demo
```

Create means create or refresh the recovery point. Restore leaves the VM stopped
and retains the backup for reuse. Both recorded state and the actual VMM process
must be stopped; a paused guest is not eligible. TTstack does not stop or restart
the VM implicitly. Viewing backup status is allowed while the VM runs.

Turning admission off prevents new create/refresh operations. Already accepted
operations can finish, and existing backups remain inspectable, restorable, and
removable. Disabled admission does not remove backups or their reservations.

`tt env backup delete VM_ID` explicitly removes the backup and its retired
generations from a stopped VM. **VM/environment deletion and expiry also delete
its backups.** Backup does not extend the environment lifetime. Read the expiry
shown by `env show`; the normal default is six hours unless explicitly changed.

## Supported storage

| Engine and storage | Mechanism |
| --- | --- |
| QEMU + `zvol` | Snapshot the VM's runtime root zvol |
| Firecracker + `zvol` | Snapshot the runtime clone's `rootfs` child zvol |
| QEMU + `file` | Strict filesystem reflink of an eligible standalone qcow2 image |
| Firecracker + `file` | Strict filesystem reflink of its raw `rootfs.ext4` file |
| File storage without qualified reflink support | Backup unsupported |
| FreeBSD bhyve + `zvol` | Snapshot the VM's runtime root zvol |
| FreeBSD bhyve + `file` | Backup unsupported; no strict file reflink implementation |
| Docker/Podman or FreeBSD Jail | Backup unsupported |

There is **no full-file-copy fallback or opt-in slow mode**, including sparse
copies or image conversion. qcow2 internal snapshots are not implemented by this
release. A file image on ZFS still uses the file/reflink path, not an automatic
dataset snapshot. Provisioning's existing `cp --reflink=auto` behavior does not
apply to backup.

Strict file reflinks currently use Linux `FICLONE`; FreeBSD file storage returns
unsupported without copying or modifying the disk. bhyve zvols use the shared ZFS
ownership, retry, restore and cleanup protocol; its root capacity is reserved
at placement and rechecked on the agent before cloning. Jail roots are directories and
are rejected before storage admission even when the host has ZFS.

Reflink support is probed at the configured runtime location, with independent
writes verified. Source and backup files must be on the same eligible filesystem.
QEMU admission checks format, capacity, and standalone storage: backing chains,
external data files, encryption dependencies, existing internal snapshots, and
dirty images are not accepted by this implementation. Zvols must be writable and
must not use `sync=disabled`. Admission checks storage headroom as well as the
configured logical disk budget.

The [scoped host report](capabilities.md) exposes `disk.backup.point` qualification
and separate `backup_admission` state. It does not gate recovery of already
accepted work on the current admission flag.

Legacy host capabilities remain `disk_backup_v1` for the protocol and, when qualified,
`disk_backup_zvol_v1` or `disk_backup_reflink_v1`. Inspection's `supported` flag
describes engine and host-mechanism eligibility; the particular disk is revalidated
on admission.
Unsupported eligibility returns an explicit `backup unsupported: ...` error.
Timeouts, unavailable storage, and exhausted budgets remain operational errors;
none triggers a copy fallback or an unvalidated backend switch.

## Contents and consistency

The recovery point contains root-disk blocks: partitions/filesystems, guest
software, guest keys, and application files. It excludes RAM/device state, fleet
databases, host networking, the retained host SSH seed, QEMU's generated seed ISO,
and Firecracker's host kernel/configuration drive. Guest-side copies of seed
material already written to the root disk are naturally included.

Restore requires the existing VM, matching root capacity, and compatible retained
boot dependencies. Those dependencies are checked by digest. It keeps UUID,
placement, CPU/RAM, IP, port mappings, environment membership, and expiry. It does
not restore an application's external database or other caller-managed state.

Stop may use forced termination. A stopped-disk backup is not a guarantee that an
application flushed its data or was healthy when captured. Operators/callers own
application quiescing. Structural artifact validation is distinct from a guest
restore exercise or application-readiness test. Snapshots and reflinks share the
original storage's failure domain and are not independent disaster-recovery copies.

## Refresh and asynchronous cleanup

One backup means one **published recovery point**, not one physical artifact.
TTstack creates and verifies a candidate while preserving the previous current
backup, then durably publishes the replacement. The old generation is retired
and removed asynchronously. Cleanup failure does not undo successful publication.

The agent bounds retirement backlog: a refresh is refused when two retirement
records already remain. File restore may additionally retain one temporary-file
metadata record until cleanup; another fresh restore waits for that staging
cleanup. The current recovery point remains available. ZFS holds or foreign
dependencies are reported, never removed using force or recursive rollback flags.
Older held retired snapshots do not prevent restoring the current snapshot; an
unowned newer snapshot can prevent rollback and is left intact.

Retired ZFS snapshots and reflink files can be cleaned while the VM runs. Cleanup
uses exact owned generation identities, bounded workers, and backoff after errors.
It never removes the current backup or a source referenced by unfinished work.
File backups live outside active-disk resolution in a private `.tt-backups`
directory. File restore makes a new reflink and replaces the stopped active disk;
it never turns the backup itself into the writable disk. Firecracker's old jail
links are removed before replacement and rebuilt at the next cold start.

Logical disk reservations include each current, candidate, and retired generation
at its root capacity, plus file-restore staging. Shared physical blocks can use much
less space; these conservative reservations are not physical quotas. Unknown or
failed cleanup retains its reservation. Root-disk capacity changes require explicit
backup removal and completed cleanup. CPU/RAM-only updates remain available when
no backup mutation is pending and existing resource checks pass.

## API and exact retries

Controller and agent expose the same VM-scoped paths. Use the controller for
normal fleet mutations. All responses use `ApiResp<T>`; successful data is a view
containing `vm`, `enabled`, `supported`, and `unsupported_reason`.

| Method | Path | Operation |
| --- | --- | --- |
| GET | `/api/vms/{id}/backup` | Inspect status and the revision ETag |
| PUT | `/api/vms/{id}/backup` | Create/refresh; no body required |
| POST | `/api/vms/{id}/backup/restore` | Restore `{"generation":"GENERATION"}` |
| DELETE | `/api/vms/{id}/backup` | Explicitly remove the recovery point and retired artifacts |

Mutations require `Idempotency-Key: OPERATION_UUID` and
`If-Match: "REVISION_FROM_INSPECTION"`. IDs are UUIDs and the revision is quoted.
The CLI obtains a revision, generates a key, and prints the exact retry command
before sending. After an unknown outcome, inspect and reuse that command:

```sh
tt env backup restore VM_ID --generation GENERATION \
  --operation-id OPERATION_UUID --expected-revision REVISION
```

Do not replace the key/revision automatically. A matching pending request joins
the existing operation; a matching completed request returns without applying the
disk again, even if the guest has since run and written new data. Changed parameters
under a retained key conflict. Older requests outside the bounded receipt window
are rejected by their stale revision, not executed as new work.

`vm.backup` reports the revision/sequence, current generation, pending request,
retired records, latest result, cleanup diagnostics/backoff, and controller
forwarding intent. Generation data includes ID, backend, creation time, root
capacity and ownership/dependency evidence; paths are derived internally. The
sequence prevents older observations from overwriting newer backup completion.

Malformed input, disabled admission, or unsupported eligibility returns 400;
missing required mutation headers returns 428; stale revisions return 412;
running/busy VMs, backlog, resource, or ownership conflicts return 409. Missing
VM/source returns 404. Storage or transport errors may return 500/502/503 with an
unknown outcome. Inspect the pending request and latest result before retrying.
An error status alone does not establish cancellation. The agent's internal
operation-specific `X-TT-Backup-Settled` acknowledgement and its sequence floor
let the controller resolve forwarding intent only after an equally new inventory
accounts for any remaining artifacts. A settled writer alone does not imply that
all candidate/retired space has been released.

Accepted work survives client disconnection. The agent persists exclusion and
reservations before storage mutation, releases its host runtime mutex for slow
storage work, and checks ownership again on publication. Conflicting start,
resize, backup, cleanup, or destructive deletion cannot bypass that exclusion via
direct agent calls. Deletion can supersede unresolved work only after its writer
is settled. Reads use database snapshots. Storage subprocesses inherit a file lock
so an orphan writer continues to exclude a second writer after agent failure.

Normal foreground storage commands share a 240-second attempt budget with a
60-second per-command maximum. Controller and CLI HTTP waits remain 360 and 600
seconds. These are waiting/execution limits, not proof that storage made no changes.
After an interrupted restore, the VM remains fenced until exact retry/recovery
settles the operation. Cleanup is outside successful refresh publication and uses
separate bounded attempts. Host power-loss durability depends on storage honoring
flushes; the writer databases use WAL/FULL.

## SSH observations after restore

Committed restore sets `ssh.ready=false`, `ssh.initialized=false`, and
`ssh.checked_at=0`, retaining the initial advertised host key and configured
account/endpoint metadata. A fresh banner and initial-key observation are required
after the caller starts the VM. `ssh.observation_error` distinguishes this pending
identity observation from disk restore success. An intentionally changed guest
host key can remain unconfirmed without invalidating the disk recovery point.
TTstack does not overwrite the guest key or adopt an unverified new one.

The guest initialization marker and the agent's `initialized` observation are
different state. QEMU's stable instance ID and cloud-init execution records govern
whether its seed script runs; marker absence alone does not force dispatch.
Prepared Firecracker init has its own script-dispatch contract. Restore does not
clear cloud-init records, delete markers, change instance IDs, or force provisioning.
Ordinary stop/start retains its existing SSH behavior.

## Upgrades and evidence

Read [upgrade compatibility](deployment.md#persistent-state-schema-gate) before replacing
services with retained inventory; disabling admission does not make old binaries
safe to use on the new state.

The [proposal](proposals/vm-disk-backup.md) retains rationale and deferred backends.
The [validation index](README.md#validation-evidence) records tested combinations
and limitations. A supported primitive or successful unit test alone is not a
claim of live guest recovery on every host/filesystem version.

Return to the [documentation index](README.md).
