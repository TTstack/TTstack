# Lifecycle correction validation — 2026-09-28

This run validates the container cleanup, resize reconciliation, image catalog
and published-port corrections on both explicitly authorized hosts, A and B.
Maintained behavior is described in the [REST API](../rest-api.md) and
[guest networking guide](../guest-images.md#networking-and-platform-scope).

## Tested source and artifacts

Implementation revision: `b382e748f4e651dd719d772cbd192cd8c54aa5cd`. Its source is
identical to the working tree used for the local checks and live tests. The release
artifacts were built before committing, from base
`bff24fb75d8fbb6b2b32a6a654f3ad79ff26b340` plus these corrections. SHA-256 of
`git diff --binary bff24fb b382e74 -- Cargo.toml Cargo.lock crates`:

`dd6bea95f8f9c3890aa742623fcd96001d08a652858cd21db60548edad259c43`

Both hosts received the same locally built release binaries. Their checksums and
the pinned Alpine image checksum were verified before execution.

| Binary | SHA-256 |
| --- | --- |
| `tt` | `d03caef36166c4a791e0f44e529345b9de45a0a605220e39ff41b15f70b3fd60` |
| `tt-agent` | `abb5ce03a0b5d282aad4413d82ff937c7ef6718240965cfa9db8fb81a37823d9` |
| `tt-ctl` | `9a540337941f2c7f57b5b5da25dce44536620b3ada2d7ad5f1ac11bb4ad4729d` |

## Isolation and resource bounds

Both hosts had 32 logical CPUs and 126380 MiB RAM. Initial memory use was about
3.2 GiB; initial load was approximately zero on A and one CPU on B. Both ran
Linux 6.8.0-138-generic, QEMU/KVM 8.2.2 and Docker 29.1.3. B had an existing
running container, which was retained throughout the experiment.

Each run used separate controller/agent databases, binaries, image and clone
directories, credentials and transient systemd units. A private bind mount
isolated the engine runtime directory. Agent/controller/VM networking lived in
one network namespace; a second namespace provided the external client and peer
endpoint. The namespaces connected only to each other, with no link to the
default host namespace. All firewall mutations, including the old-rule control
experiment, were confined to the test namespace.

All test services, the harness and QEMU processes shared a cgroup slice capped at
**4 CPUs, 2 GiB RAM and zero swap** per host. This is below the authorized 50%
ceiling with ample room for the observed existing load. At most one guest ran on
each host, initially 1 vCPU / 256 MiB and later 2 vCPUs / 384 MiB. Docker tests
used failed pulls and created no running container. No stress test was performed.

| Measured slice value | Host A | Host B |
| --- | --- | --- |
| Peak memory | 657305600 bytes (627 MiB) | 670588928 bytes (640 MiB) |
| Total CPU time | 11.93 seconds | 12.33 seconds |
| CPU quota | 400000 / 100000 microseconds | 400000 / 100000 microseconds |

## Results

Every case below passed on both hosts using the release controller and agent.
Creation and fault probes used the authenticated HTTP API; stop, resize, start,
inspection and final VM deletion also exercised the release CLI.

| Case | Observed result |
| --- | --- |
| Docker pull failure before a container exists | Failed environment retained for inspection; deletion and repeated deletion succeeded after recovery. |
| Docker inspection failure during deletion | An isolated command wrapper simulated daemon unavailability. Deletion failed and retained the record; removing the fault allowed cleanup. The real host daemon was not stopped. |
| QEMU create and access | Alpine 3.21.7 booted from a file-backed qcow2 disk; direct guest SSH and externally mapped SSH both worked. |
| Host-local access contract | Direct guest IP worked; loopback mapped-port connection was unavailable with the generated rules, confirming the corrected documentation. CLI inspection printed the direct access path. |
| Outgoing traffic to a published port number | A peer endpoint used the same TCP port as the VM's host SSH mapping. The corrected rule reached the peer; temporarily restoring the prior unscoped rule caused the probe to fail. Restoring the corrected rule recovered access. |
| Stop/start retention | A synced guest marker survived stop, disk growth and subsequent boots. |
| Lost resize reply before agent intent is recorded | Controller kept the pending target and 3072 MiB reservation while the agent still reported its old stopped VM. Start and a different resize target returned 409. |
| Controller restart during that unknown outcome | Pending target and reservation survived restart and old agent snapshots; start remained blocked. CLI inspection displayed the exact pending target. |
| Original request completes | The same agent request grew the disk; a confirming snapshot cleared controller intent. Repeating the target succeeded. |
| Resized guest | Guest reported 2 CPUs, approximately 351260 KiB RAM and a 3221225472-byte virtual block device. The original marker remained readable. |
| Slow catalog and agent restart | With a 20-second image-inspection delay, maximum sampled `/api/info` latency was 0.0007 seconds on A and 0.0008 seconds on B. The controller kept the host online, the warning exposed the slow scan, and the catalog recovered. |
| Running guest survives agent restart | Same QEMU PID, published SSH access and retained marker after restart. |
| Final API cleanup | Both inventories empty; CPU, memory, disk and VM reservations zero; clone files, PID files and TAPs removed. |

The resize fault used a private wrapper around the real `qemu-img`: it held the
agent's stopped-disk preflight before pending intent was written. A local test
proxy forwarded the request to the agent and then closed only the controller's
connection. The agent continued its original operation. Releasing the gate
allowed real disk growth and a successful upstream response, which the proxy
deliberately did not return to the original caller. No fault-injection hooks were
added to TTstack itself.

The catalog fault delayed a real base-image inspection in the same private PATH.
No host binaries, images belonging to other services or existing Docker settings
were modified.

## Shutdown and coverage limits

The measured stop call took 2.410 seconds on A and 10.062 seconds on B. A completed
before QEMU's forced-termination wait expired. B's end-to-end timing alone cannot
distinguish a late guest shutdown from termination fallback. This run confirms
process stop and retention of explicitly synced data; it does not certify that
every guest application shuts down gracefully.

This run covers QEMU/KVM with the named Alpine file image and the real Docker
failed-create cleanup path. It does not add live Firecracker, Podman, ZFS/zvol,
host reboot, storage exhaustion or production deployment coverage. The resize
probe checked guest-visible block capacity, not a general promise of guest
filesystem expansion. Firecracker reconciliation and resource-accounting cases
passed locally, including configuration-drive overhead.

## Cleanup and local checks

Task VMs, clones, transient services, namespace pairs, slice settings, binaries,
images, state and credentials were removed from both hosts. Fresh SSH connections
succeeded after cleanup, and both existing SSH and Docker services remained
active. Before/after comparisons matched for host addresses, routes, firewall
structure, running services, existing container inventory and the original engine
runtime directory. Comparisons ignored normal lease countdowns and packet counters.

Local checks passed: **162 workspace tests** (CLI 19, agent 31, controller 47,
core 65), formatting, Clippy for all targets with warnings denied, workspace check,
Rust 1.88 workspace check, locked release build, local documentation links and
diff checks. Configuration-drive/ext4 tests ran with their required external
tools; no skipped external-tool case was counted as a pass.
