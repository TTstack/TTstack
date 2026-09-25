# Firecracker cold-start entropy validation — 2026-09-24

This is bounded application-startup evidence, not a capacity or security
certification. The tested TTstack revision is `675be03` (previously `8775fdb`),
with matching Firecracker/jailer 1.17.0 on an authorized Linux x86_64 KVM host.

## Finding and fix

An application requiring secure random bytes did not begin serving until its
legacy Linux 4.14.174 guest kernel initialized the random pool, 171.6 seconds
after boot. The VM process was already running. An application readiness deadline
was therefore close to expiring despite successful VM creation.

TTstack now attaches Firecracker's rate-limited virtio entropy device. No fixed
seed, insecure randomness, or application-specific identity logic was added.
Custom guests need a maintained kernel with built-in virtio RNG support, as
specified in the [guest image guide](../guest-images.md#firecracker-prepared-microvm-workloads).

The test used Linux 6.1.186 from the official Firecracker CI artifact
`firecracker-ci/20260923-6f82ac4cf331-0/x86_64/vmlinux-6.1.186` in the
`spec.ccfc.min` S3 bucket. SHA-256:
`586a02db8ea1fd331d45efa6ebf502fff21ceddfedf010ef70ba389b8e095523`.
Its configuration has `CONFIG_HW_RANDOM=y` and `CONFIG_HW_RANDOM_VIRTIO=y`.
The cloned kernel was updated offline for this controlled test; TTstack does not
silently upgrade existing guest kernels or replace user disks.

## Observed behavior

- With the new engine configuration and kernel, the random pool initialized in
  approximately 17 ms and application readiness was reached in about 5 seconds.
  Both changes were applied together; this does not isolate their individual
  contributions or establish a latency guarantee.
- Repeated cold start retained files and application state. One measured reopen
  took approximately 4 seconds; an orderly-shutdown marker survived the stop.
- A failed initial sandbox launch remained visible and recovered with stop/start,
  retaining its VM ID and disk. The cause was test setup using `ip netns exec`,
  which hides cgroup mounts; `nsenter --net` preserved the required cgroup v2 view.
- Two isolated guests used 2 vCPU, 4096 MiB RAM and 8192 MiB disk each. Host CPU and
  memory usage remained well below the authorized approximate 50% test budget.
  No stress testing or additional platform claim is implied.
- Workspace formatting, Clippy with warnings denied, all 128 tests, and the
  release build passed. The change adds no dependencies or newer Rust API usage.

The entropy device is part of the generic Firecracker engine. Business identity,
application readiness checks and idle policy remain outside TTstack.

Both test environments were deleted through the controller. The isolated agent
then reported zero VMs and zero CPU, memory and disk reservations. Task-owned
services, namespace, firewall rules, guest disks and private state were removed.
The pre-existing Docker service remained active; no shared build cache was pruned.

Return to the [documentation index](../README.md).
