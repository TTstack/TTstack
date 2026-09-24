# Linux live validation — 2026-09-24

This records the code tested on three authorized Linux hosts and committed as
[`8998db7`](https://github.com/TTstack/TTstack/commit/8998db7d4f6cae14feb4996a4bf14cdbd06c4b0b).
It is historical evidence for that revision, not a performance benchmark or a claim
that every platform/backend or later revision is verified. See
[compatibility](compatibility.md) for the overall support boundary.

## Setup and load limits

- Three Ubuntu 24.04.4 x86_64 hosts; existing workloads and services remained in place.
- Hosts A and B used their existing Docker installations and a temporary HTTP-server
  image derived from an already-cached Python image. Each test container had one
  CPU and 64 MiB of memory; no load generator was used.
- Host C used QEMU 8.2.2 and Firecracker 1.17.0 extracted into a temporary directory.
  No system packages were installed. QEMU/Firecracker networking ran inside a
  dedicated network namespace, including its bridge, TAP devices and nftables rules.
- Only one test guest ran per host at a time. QEMU used one vCPU and 256 MiB
  (Alpine) or 384 MiB (Debian); Firecracker used one vCPU and 128 MiB.
- Agent services had a CPU quota of half one core. Host C's service, including its
  guest processes, had a 1 GiB memory ceiling. Downloads were limited to 4 MiB/s.
- Release binaries were compiled locally. APIs used temporary keys and SSH tunnels;
  no public controller/agent listener was added to a host's normal network namespace.

## Problems reproduced and corrected

| Observation | Correction | Recheck |
|---|---|---|
| Built-in Alpine cloud-image download returned HTTP 404 | Pin the existing Alpine 3.21.7 NoCloud image | CLI download, qcow2 inspection and guest boot passed |
| Docker restart returned HTTP 200/Running after its workload immediately exited | Inspect real container state after start; report failure and retain the record | Same deliberately broken workload returned HTTP 502/Failed and remained deletable |
| Alpine booted but could not be reached; cloud-init generated `id0` while the NIC was `eth0` | Use cloud-init network format v1 with an explicit MAC/name mapping | Alpine and Debian SSH access passed |
| A correctly injected key was rejected because Alpine's non-PAM sshd considered root locked | Supply an impossible password hash without locking the account; keep SSH password authentication disabled | Public-key login passed; effective sshd configuration still denied password login |
| QEMU did not set a per-guest MAC | Derive a stable locally administered MAC from the allocated IPv4 address | NIC/seed configuration agree; regression test checks uniqueness across the entire allocation pool |
| Captured curl progress overwhelmed a failed download's error message | Capture only actual curl errors | CLI error output no longer includes a buffered progress stream |
| Full UUIDs and image tags misaligned CLI columns | Size ID/image columns from their content and pad engine/state strings | CLI rendering checked |

## Workflow results

- Docker: authenticated API access, preflight rejection of unsupported options,
  two-host scheduling, published HTTP ports, stop/start with retained filesystem
  contents, and CPU/memory accounting after stop all passed.
- Queries during Docker stop completed in approximately 0.43 seconds including SSH
  tunnel latency. This is one functional observation, not a latency benchmark.
- Agent restart preserved live Docker, QEMU and Firecracker guests. QEMU port
  forwarding and outgoing restrictions were restored after agent restart.
- Controller restart preserved environment identity and the tracked running guests.
- An externally killed Docker container was reconciled into stopped state and could
  be restarted. An externally paused container could be resumed.
- An unavailable agent caused deletion to fail visibly while retaining its VM record.
  Once the agent returned, the controller's periodic retry removed the remaining VM.
- CLI create/show/delete, no-expiry environments, explicit 24-hour lifetimes and
  automatic cleanup of a short-lived environment passed.
- QEMU/Alpine: SSH access, 2 GiB virtual-disk growth visible inside the guest,
  stop/start retaining a test file, agent restart and final deletion passed.
- QEMU/Debian 13: SSH access, static networking, 4 GiB disk growth and deletion passed.
- A disk request smaller than its base image produced a retained, actionable failure;
  subsequent deletion cleaned up the partial clone.
- Identical agent create requests reused the same VM and resource reservation.
- With outgoing restrictions enabled, a guest could still accept forwarded SSH,
  but could not contact a test HTTP endpoint outside the namespace. The same request
  succeeded with restrictions disabled. DNAT and deny entries disappeared on deletion.
- Firecracker: built-in Alpine rootfs creation, real boot, allocated-IP reachability,
  stop/start, agent restart and deletion passed. This image does not provide SSH.
- All three agents ended with zero tracked VMs and zero reserved CPU, memory and
  disk. Test services, containers, temporary image tags, network namespace, veth
  devices, files and keys were removed. Cleanup was independently checked on each
  host; pre-existing images were retained.

## Automated checks and limits

118 workspace tests passed, together with formatting, Clippy with warnings denied,
Rust 1.88 checks for every workspace target, and a locked release build.

This run did not reboot physical hosts or test throughput, high concurrency, ZFS/zvol,
Podman, Ubuntu guests, or FreeBSD. FreeBSD remains experimental. Running state reports
an observed engine process/container, not completion of guest boot or application readiness.
