# VM disk backup — two-host validation, 2026-09-29

The final functional checks passed on both authorized hosts, and all task-owned
remote resources were cleaned up. The limits of the network baseline comparison
are recorded below; this is not a blanket platform or security certification.

## Tested source and local checks

The implementation is the source introduced with this report, built and tested
before its commit. Its base revision is `37100e1`; stop-retry correction `5aac9b5`
is included in the tested source. SHA-256 of the canonical, sorted JSON mapping of
`Cargo.toml`, `Cargo.lock`, and all files under `crates/` to their SHA-256 digests:

`2a7b9f036aaed0fd05405399b7f883c4959585b6b9f3f97c571d1902c6df0ff6`

Final Linux x86_64 release artifacts:

| Binary | SHA-256 |
| --- | --- |
| `tt` | `73b2f9dd4b203aaf6d3c6e81e2836c2ca6e0b1da8fd650ead67c3d6ad4e92bd3` |
| `tt-agent` | `d1673cbc3ceb3c63bc91294a6cf7cd72a885188ad35db005ce0de6d7d1f695e6` |
| `tt-ctl` | `3b9fc17fea10ef73a865c6db0b0377b682e8714a40c20fe7a1da647b519e1288` |

Local verification passed **194 tests**: CLI 19, agent 46, controller 52, and core
77. Formatting, Clippy for all targets with warnings denied, the locked workspace
tests, Rust 1.88 workspace checking, and the locked release build passed. The
release compiler was Rust 1.98.1. Dependencies and `Cargo.lock` were unchanged.

Focused tests cover disabled admission, lifecycle exclusion before/during storage
work, failed refresh preserving the current generation, retirement accounting,
restore intent across database reopen, completed-request replay, forwarding of
conditional/idempotency headers, settled-operation inventory sequence floors,
stale observations, transactional controller migration, truncated storage output,
and inherited worker locks after agent process loss. Backup inspection also remains
available while the runtime mutex is held or an unrelated VM row is corrupt.

The local file test exercised the explicit unsupported-reflink result, including
an empty unpublished candidate and no copy fallback. Real reflink restore was
verified on the XFS fixtures below; a conditional local branch is not presented
as a successful live reflink test. Existing ext4 tests used their actual external
tools. No skipped test was counted as a pass.

## Hosts and isolation

The requester authorized two hosts, identified here as A and B. Both had 32 logical
CPUs and 126380 MiB RAM. Initial available memory exceeded 122000 MiB; load averages
were approximately 0.49 and 0.97 respectively. Existing services were not upgraded.

Both hosts used Linux 6.8.0-138-generic, QEMU 8.2.2
(`1:8.2.2+ds-0ubuntu1.18`), ZFS userspace 2.2.2-0ubuntu9.5 with kernel module
2.2.2-0ubuntu9.4, and the pinned Firecracker/jailer v1.17.0 binaries.

Each host had a separate task directory, private API credentials and guest login
key, databases, transient services, network namespace, and persistent test mount
namespace. The latter retained the private engine run-directory bind and ZFS
runtime mounts across agent restarts. Guest networking and firewall operations
remained inside the task's network namespace, without a host-network uplink.

Storage fixtures were an owned 8 GiB file-backed ZFS pool, an owned 8 GiB loopback
XFS filesystem with reflink enabled, and the host's non-reflink file storage for
the unsupported case. No existing pool, partition, image, or guest was repurposed.
Dataset clones and snapshots were confined to the task-owned pool.

Preparation and service/QEMU processes shared a slice limited to **4 CPUs,
4 GiB RAM, and zero swap**. Firecracker used its separate jailer cgroup, at most
two vCPUs and 384 MiB including VMM overhead for a guest in these cases. At most
one test guest ran per host at a time. These bounds and observed existing load
were well below the authorized half-host ceiling. ZFS kernel cache use was also
observed through host memory availability rather than assumed to belong to the
service slice. This was functional testing, not stress or throughput testing.

The last recorded service-slice memory peaks were 1448488960 bytes on A and
1443753984 bytes on B (about 1.35 GiB). B's value was sampled before final teardown,
not reconstructed after its cgroup disappeared. Sampled Firecracker peaks during
the final checks were approximately 62–63 MiB, with `memory.max=402653184` and
`cpu.max=200000 100000` verified for the two-vCPU cases. Host available memory
remained above 122000 MiB in the later resource samples.

## Guest fixtures

QEMU used the repository's Alpine 3.21.7 NoCloud qcow2 image with its pinned
SHA-512 verified. Its 200 MiB base virtual disk was grown to 512 MiB for the main
cases. Zvol preparation converted the verified image into the owned raw volume;
that fixture preparation is distinct from the backup implementation, which never
copies or converts a whole disk as fallback.

Firecracker used a prepared 256 MiB ext4 Alpine rootfs with SSH bootstrap version
1, OpenSSH, sudo, an independent sshd startup, and a shutdown marker hook. Its
separate SSH configuration drive reserved 4 MiB. The minirootfs tarball and
kernel were checksum verified; APK signatures were checked when preparing the
guest, and the resolved package inventory was retained. Package repositories
were not content pinned.

| Asset | SHA-256 |
| --- | --- |
| Firecracker v1.17.0 release archive | `06094a1108ae9e82aa4c23a775aa92758f53f1175d422270d9d6162cb9ade558` |
| Linux 6.1.186 guest kernel | `586a02db8ea1fd331d45efa6ebf502fff21ceddfedf010ef70ba389b8e095523` |
| Alpine 3.21.3 minirootfs archive | `1a694899e406ce55d32334c47ac0b2efb6c06d7e878102d1840892ad44cd5239` |

## Functional results

Both hosts exercised all four QEMU/Firecracker and zvol/reflink combinations.
The final binaries repeated disk round trips after source was frozen; B's file
coverage used the full lifecycle harness, while final confirmation on the other
combinations used the shorter round-trip harness. The extended fault suites also
ran on both hosts with the final binaries.

| Case | Observed outcome |
| --- | --- |
| Default-off and running VM | Disabled creation returned 400 without a recovery point. Running guests returned 409; the operation did not stop them. |
| Real disk restoration | A synced marker A was backed up, replaced with B, and restored to A after cold start, on both engines/backends. |
| Completed restore replay | After writing C, repeating the original operation ID/revision/generation retained C; no second rollback occurred. |
| Service restart and disabled admission | Existing backups and receipts survived isolated agent/controller restart. Restore remained usable with new backup admission disabled. |
| Refresh and running cleanup | The published generation changed; the previous generation was retired and removed while the guest ran, without changing its marker. |
| Resource updates | Root growth was rejected while backups remained. CPU/RAM-only changes worked; explicit backup removal allowed subsequent growth. Final Firecracker cases omitted initial disk size, changed CPU with an explicit root size, and still restored successfully. |
| ZFS retirement holds | Two retired snapshots were held deliberately. Refresh at the backlog limit returned 409, while the current recovery point still restored. Releasing only those task holds allowed cleanup to finish. |
| Failed refresh | An injected snapshot-command failure retained the previous current generation. Retrying that terminal operation did not create a new snapshot or consume another revision. |
| Agent loss with a surviving storage child | A task wrapper delayed one snapshot. Killing only the isolated agent main process left the inherited lock in place. A restarted agent recovered the same persisted operation; inspection stayed responsive, conflicting start was rejected, and exactly one snapshot command completed. |
| Guest SSH identity and cloud-init marker | A QEMU guest's host key was changed and its TTstack marker removed after cloud-init completion. Backup/restore preserved that disk state, did not replay the original seed, and reported managed SSH identity unconfirmed while separately trusted SSH still worked. |
| Unsupported host filesystem | Enabled backup on the non-reflink file fixture returned 400 with `backup unsupported`. No backup directory, published generation, or whole-file-copy fallback appeared. |
| CLI and cleanup | CLI inspect/create/restore/delete and API exact retries were exercised. Deleting environments removed owned recovery points; agent/controller inventories and disk reservations returned to zero. |

Final sampled create calls included roughly 0.05 seconds for a small QEMU reflink
fixture and 0.25–0.38 seconds for the small zvol fixtures. These are individual
functional observations including API overhead, not performance guarantees.

## Failure observations and harness corrections

Early attempts identified test-harness assumptions separately from product
behavior. A controller's old image catalog could outlive an agent restart; the
harness was corrected to wait for both the new agent's catalog and the controller
catalog. Per-agent private mount namespaces also lost task-created Firecracker
kernel/config mounts at restart. The tests now share one persistent, isolated mount
namespace, preserving those prerequisites without changing product mount policy.
Only exact task-owned datasets were remounted while cleaning the earlier attempts.

The optional resource sampler initially looked for cgroup files through
`ip netns exec`'s remounted sysfs. It was corrected to sample through the keeper's
mount namespace. Disk restore assertions had completed before that sampler failed;
the live guest was rechecked, its limits measured, and owned cleanup completed.

A QEMU stop encountered a transient unreadable process identity while exiting.
The conservative engine check retained resources. The agent could subsequently
observe `stopped` yet keep the old stop error, causing an explicit retry to remain
failed. The focused correction identifies unfinished stop errors, rechecks the
owned engine on retry, and clears only that stop error after confirmation.
Unrelated errors remain. The regression covers the observed stopped-state case;
live stop retries also completed after the correction. Process identity safety
checks and the existing guest shutdown windows were not relaxed.

Some delete attempts likewise returned a conservative incomplete-cleanup response
during process exit. Exact idempotent retries/reconciliation completed cleanup;
the harness records those retries instead of treating a first response as proof
that the process or storage had disappeared.

## Limits

No physical host power cycle or power-loss experiment was performed. Agent and
controller service restarts and controlled loss of the isolated agent process do
not establish power-loss behavior. WAL/FULL and backend flush ordering have local
coverage/design checks, not a claim that every storage device honors flushes.

QEMU stop duration alone does not establish whether guest shutdown or termination
fallback completed it. Firecracker used the prepared image's shutdown path.
Persistence assertions concern explicitly synced disk markers, not guest RAM or
application-consistent state. SSH reachability/identity observations are separate
from disk restore success.

This report adds no live Btrfs, network-filesystem, internal-qcow2-snapshot,
container-backup, online-backup, cross-host restore, or deleted-VM recovery claim.
There was no in-place conversion of a production agent v5 database. The native
agent v6 gate and controller v5 migration were tested locally; upgrade restrictions
are documented in the maintained guide. No maximum-capacity or latency guarantee
is established by these small fixtures.

## Cleanup and maintained contracts

Cleanup verified empty private VM inventories and no remaining owned VMMs before
stopping task services and the namespace keeper. The owned pool was destroyed only
after checking its backing-file identity; the exact XFS mount/loop device and
network namespace were removed. Task directories and remote staging/evidence
archives were removed after the non-secret evidence was retrieved. Final host
checks showed no task pool, loop attachment, namespace, or staging directory.

Both hosts' pre-existing running service lists matched the baseline; SSH, Docker,
and containerd remained active. There was no host `tt0` bridge or `tt-nat` table
after cleanup. No host firewall reset or rule replacement was performed.

The initial firewall baseline was an opaque hash of raw `iptables-save` output,
which includes volatile timestamps and chain counters. Its later mismatch cannot
establish a structural rule change or full before/after equivalence. That broad
comparison is therefore **not claimed**. Isolation, absence of task networking on
the host, and service-state checks were verified separately; A also compared
normalized host rules immediately before and after cleanup. This baseline
limitation does not justify editing shared host rules to manufacture a match.

Private host inventories, API credentials, guest keys/configuration, databases,
and disk images are excluded from retained reports and evidence archives.

Current behavior is maintained in [disk backup](../disk-backup.md),
[deployment/schema compatibility](../deployment.md#resource-update-schema-gate),
and the [lifecycle API](../rest-api.md#lifecycle-and-recovery). The
[design proposal](../proposals/vm-disk-backup.md) retains rationale and deferred
work; it is not a second behavior guide.
