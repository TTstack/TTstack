# Offline resource update validation — 2026-09-25

Tested the resource-update changes based on `166b318`, included with this report.
Linux x86_64, Firecracker/jailer 1.17.0 and the existing verified 6.1 guest kernel.
The dedicated resource host used independent test agents/controllers, state roots,
network namespaces and file/ZFS images; its existing OMM guest was not changed.

## Results

Both storage backends passed the same sequence, one small guest at a time:

| Case | File image | ZFS zvol |
| --- | --- | --- |
| Create 1 vCPU / 256 MiB / 128 MiB root disk | Pass | Pass |
| Write and sync a marker from the live guest | Pass | Pass |
| Reject resource update while running | 409 | 409 |
| Stop, select 2 vCPU / 384 MiB / 192 MiB root disk | Pass | Pass |
| Repeat the same target | Pass | Pass |
| Reject disk shrink | 400 | 400 |
| Start a new VMM process using the same VM/disk | Pass | Pass |
| Guest reports 2 CPUs and a larger mounted filesystem; marker retained | Pass | Pass |
| Restart isolated agent/controller, start retained VM, read marker | Pass | Pass |
| Delete task VM and release its clone | Pass | Pass |

The minimal BusyBox probe image did not complete orderly shutdown; these tests
observed the existing forced-stop fallback after its grace period. The marker was
synced before stop. This validates the forced-stop recovery path, not orderly
shutdown of every application. Full OMM application acceptance is a separate gate.

Initial harness setup exposed two operator constraints, corrected before passing:
`ip netns exec` remounts sysfs and hid cgroup v2, so the harness uses `nsenter --net`;
Firecracker's Unix socket requires a short runtime mountpoint. No host-wide network
or cgroup reset was used. The rejected/failed task allocations were inspected and
removed through their isolated controller before retrying.

All task guests, clone volumes, base test datasets, namespaces, services and test
credentials were removed. The pre-existing agent remains active. Peak functional
allocation was 2 vCPUs and 384 MiB guest RAM, well below the half-host budget; no
load/throughput testing was performed.

## Local verification

Formatting, Clippy (`-D warnings`), workspace tests (156 passed), the Rust 1.88
workspace check and release build passed. Regression coverage includes durable
pending intent across database reopen, retained disk reservation, refusal to start
an unfinished update, retry after device growth but before filesystem growth,
marker retention, shrink/capacity/running-state rejection, a controller capability
gate and recovery of an upstream reply lost after a successful update.

The operation rebuilds the runtime from the retained disk; it does not migrate a
guest, retain RAM or replace its identity. Large disks, storage exhaustion during
growth, QEMU/container resizing and online hotplug were not tested or enabled.
The [API contract](../rest-api.md#offline-resource-updates) is the maintained behavior.
