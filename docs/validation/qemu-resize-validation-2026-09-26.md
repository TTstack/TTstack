# QEMU offline resize validation — 2026-09-26

Tested implementation revision `f6cb106e065f7c6b356b2630ac4a66006825b3a5` on two
explicitly authorized Linux hosts, A and B. Unlike the earlier
[native capability probes](engine-capability-probes-2026-09-26.md), this run used
the implemented CLI, controller, agent and persistent runtime state end to end.
The [offline resource API](../rest-api.md#offline-resource-updates) describes the
maintained behavior and guest filesystem boundary.

## Setup and limits

Both hosts used Ubuntu 24.04.4, Linux 6.8.0-138-generic, QEMU/KVM 8.2.2 and the
Alpine 3.21.7 NoCloud BIOS cloud-init image. Each had 32 logical CPUs and
126380 MiB RAM, with approximately zero initial load. One VM ran per host at a
time, first on a qcow2 file and then on a ZFS snapshot clone. The ZFS backend used
an explicitly authorized spare NVMe on each host, checked for existing
partitions, signatures, mounts, holders and open users before pool creation.
ZFS userspace was 2.2.2-0ubuntu9.5 and the kernel module was
2.2.2-0ubuntu9.4. Each temporary pool had a 12 GiB quota.

All test services and VMs were below a dedicated cgroup slice with a two-CPU,
2 GiB memory ceiling and no swap. ZFS ARC was separately capped at 256 MiB.
Recorded slice memory peaks were 859574272 bytes on A and 856289280 bytes on B;
slice CPU time was approximately 39 seconds each. ARC was approximately 129 MiB
before teardown. Downloads were limited to 4 MiB/s. No stress test was performed.

The controller and agent ran in a private network namespace; a second namespace
acted as an SSH client through the published host port. Neither namespace had a
link to the default host namespace. The agent used isolated state, images and
clone locations, and a private bind mount for the fixed engine runtime directory.
Temporary API credentials and SSH keys were not passed as command-line values.
No existing TTstack service or Docker daemon was replaced or restarted.

The locally built release binaries copied to both hosts had these SHA-256 values:

| Binary | SHA-256 |
| --- | --- |
| `tt` | `96334074052e1f708604b79a0cb89284a0615647a1575098249c5944acea79ff` |
| `tt-agent` | `4a034978bdb4f1ec5a6276dd9661743b5c274fc412276e4345813b8e822c093b` |
| `tt-ctl` | `f10e6ca5e84b226d4ed20d487a11c32e5826186a23958a80993c7d60460c1ab2` |

## End-to-end results

Every case below passed for all four combinations: A/file, B/file, A/zvol and
B/zvol. Creation, stop, resize, start and delete used the release CLI. Additional
HTTP probes checked controller and direct-agent rejection behavior.

| Case | Observed result |
| --- | --- |
| Capability advertisement | Agent advertised `qemu_resources` |
| Create 1 vCPU / 256 MiB / 2048 MiB, lifetime zero | Guest booted; SSH worked through the allocated host port |
| Resize a running VM | Both controller and agent returned 409 |
| Zero CPU, disk shrink, excess CPU or disk reservation | Rejected before disk mutation |
| Repeat stop | Remained stopped without deleting the disk |
| Error after successful physical disk growth | Pending target survived; old reported resources remained; larger disk reservation retained |
| Start or select a different target while pending | Rejected |
| Restart agent and controller while pending | Target persisted; start still rejected |
| Retry the recorded target and repeat it | Completed idempotently; VM remained stopped; pending target cleared |
| Start at 2 vCPUs / 384 MiB / 3072 MiB | Guest reported the new CPU/RAM/disk; filesystem expanded at boot; marker retained |
| Restart agent and controller while VM runs | Same QEMU PID survived; SSH and marker remained available |
| Stop and reduce to 1 vCPU / 256 MiB | Disk remained 3072 MiB; no storage resize command was executed |
| Shrink the expanded disk | Rejected |
| Preserve identity and options | VM/environment/host IDs, image, IP, port mapping, creation time, SSH keys and network flags retained |
| Delete | Agent VM inventory empty; CPU/RAM/disk/VM reservations zero; runtime disk/clone and PID file removed |

Initial guest block capacity was 4194304 sectors and became 6291456 sectors.
Mounted root filesystem capacity increased from 1958375 to 2940901 KiB. Guest
visible memory increased from approximately 222868 to 351264 KiB and returned to
approximately 222868 KiB after reduction. Kernel overhead explains the difference
from configured guest RAM. The synced marker survived all boots and service
restarts.

## Failure injection and recovery

The agent's private PATH contained test wrappers around the installed `qemu-img`
and `zfs` executables. A one-shot marker caused the wrapper to execute the real
growth successfully, then return an injected failure. All other commands were
passed directly to the installed tools. No TTstack fault-injection code was added.

The failed request targeted 2 vCPUs, 384 MiB RAM and 3072 MiB disk. Inspection
confirmed a real 3072 MiB virtual disk but a persisted VM still reporting its old
2048 MiB disk, with the full target in `pending_resources`. Agent accounting
reserved 3072 MiB disk and zero stopped-VM CPU. Starting was refused before and
after restarting both services. Retrying the exact target completed the operation
against the same already-grown disk.

For the later CPU/RAM-only reduction, the failure marker was armed again. The
update succeeded and left that marker unconsumed, demonstrating that unchanged
disk capacity did not invoke a physical resize command.

Local controller tests additionally cover a lost upstream resize reply, refusal
when the engine-specific capability is absent, and refusal to use
`firecracker_resources` as authorization for QEMU resizing. Agent tests cover
database reopen after partial growth, data/identity retention, recorded and actual
running-state rejection, invalid resources and continued Docker rejection.

## Shutdown and coverage boundaries

All stop/delete operations completed through TTstack's existing QEMU shutdown and
termination-fallback path. This harness did not separately classify each stop as
orderly guest shutdown versus forced fallback; it verifies confirmed process stop
and retained disk data, not universal application graceful shutdown.

Only the named Alpine image's automatic filesystem growth was verified. Other
guest layouts, distributions, LVM/encryption, disk exhaustion, host reboot and
running-VM hotplug were not tested. The operation grows a QEMU virtual block
device; API success is not a promise that a guest filesystem has already expanded.
Firecracker regression tests passed locally; no new live Firecracker run was
performed. Docker/Podman resource or network capabilities were not enabled.

## Cleanup and local checks

All test VMs, clones, services, cgroups, namespaces, temporary directories and
credentials were removed. Temporary pools were destroyed, their task-created
partition tables/signatures cleared, and the originally unloaded ZFS module was
unloaded again. Installed prerequisite packages remain installed as authorized.

Both hosts passed comparisons of their original IP addresses, routes, firewall
rule structure and existing container inventory. Comparisons ignored normal IPv6
lease countdowns and firewall counters. All pre-existing running services remained
running; fresh SSH connections and the original Docker service worked after
cleanup.

Local validation passed: 158 workspace tests, formatting, Clippy with warnings
denied, workspace check, Rust 1.88 workspace check, release build, documentation
links and diff checks. No test was counted as passed by skipping an external tool.
