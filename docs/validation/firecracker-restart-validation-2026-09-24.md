# Firecracker restart on a dedicated ZFS host — 2026-09-24

## Scope

A dedicated Linux x86_64 host ran Firecracker 1.17.0 and kernel 6.1.186, with the
controller on a separate host. A private WireGuard link carried administrative
and published application traffic. One 2-vCPU, 4096-MiB, 8192-MiB VM was used;
there was no stress test. The host had 32 CPUs and about 186 GiB RAM, with
reservations limited to 16 CPUs and 80 GiB RAM.

A separately verified unused NVMe device was provisioned as the single-device
`ttvm` ZFS pool. The operating-system disk was not modified. Dataset mountpoints
held images, VM runtime files and agent state. The Firecracker backend remained
`file`; this does not validate QEMU's zvol backend or per-VM ZFS cloning.
Compression, an aggregate 3-TiB quota and a 16-GiB ARC limit were configured.
A single-device pool is not redundant storage.

## Observed defect and correction

During a stopped VM's first boot after a resource-host reboot, the old runtime
reported that jailer had exited. The recorded child PID was still a running
Firecracker and its API returned 200. The old boot loop treated a missing
`/proc/PID/cmdline` identity marker as proof of exit; jailer's exec transition can
make that marker temporarily unavailable.

Startup now checks the owned `Child` with `try_wait()` for actual exit and waits
separately for the existing API readiness check. Process identity checks still
protect subsequent stop, discovery and cleanup operations. Real early exit fails;
a readiness timeout preserves the process and disks, and the child is reaped on
every outcome. Focused tests cover delayed readiness with no identity marker,
actual early exit, and a live child retained on timeout.

## Checks and live result

The corrected agent is `3be1723`; the controller remains `a875433` because no
controller/API change was needed. Formatting, Clippy with warnings denied, all
131 workspace tests, the three focused boot checks and the release agent build
passed. No dependency or minimum-Rust change was introduced.

After deploying the verified release artifact, the retained workspace recovered.
A second, clean dedicated-host reboot automatically imported the healthy ZFS
pool, mounted all four datasets, restored the encrypted link and started the
agent. Explicit acquisition booted the same VM; both a file marker and stored
application session survived. The test allowed the controller to reconcile the
brief host outage before retrying acquisition. No empty disk replacement was used.

The old agent was removed from the controller host. Its task-created namespace
and NAT rules were removed without resetting host networking or other services.
The final agent listens only on the private link. An external probe could not
reach its management port on the public host address.

## Verification boundaries

The application-level checks use an external caller and do not add application
identity or installation policy to TTstack. See the [storage contract](../guest-images.md#storage)
for maintained behavior. Stop/start preserves disk data, not VM RAM. A stopped
workspace snapshot was used as an offline recovery point; no online consistency,
disk-failure recovery, cross-host migration or capacity claim is made.
