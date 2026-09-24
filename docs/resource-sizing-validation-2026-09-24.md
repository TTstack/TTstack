# Firecracker creation-time resource sizing — 2026-09-24

This report covers the change adding `firecracker_disk_resize`, creation-time
ext4 clone growth, and scheduler accounting for requested rootfs/configuration
sizes. It was tested on top of `ec2994c`; the accompanying commit contains the
implementation and this report. This is a functional check, not a capacity test.

## Environment

One authorized Ubuntu x86_64 host with KVM, cgroup v2, 32 logical CPUs and roughly
123 GiB RAM. Firecracker/jailer 1.17.0 and kernel 6.1.186 were downloaded into a
task directory. Agent/controller ran as temporary systemd services in a dedicated
network namespace; no public management listener, host firewall edits, or
permanent installation were needed. Existing host services were preserved.

A minimal BusyBox HTTP guest used a 128 MiB unpartitioned ext4 base image with a
marker file and boot counter. This fixture verifies generic VM sizing, not the
memory requirement of OMM Code or another application. At most two VMs ran:

| Guest | vCPU | RAM (MiB) | Requested rootfs (MiB) | Reserved disk, including config (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Small | 1 | 256 | 192 | 196 |
| Larger | 2 | 384 | 256 | 260 |

## Results

- Both guests booted and served their status, with the expected CPU counts and
  different memory/filesystem capacities. Runtime records matched the requested
  CPU/RAM, and clone files had the exact requested logical disk sizes.
- Guest `df` reported 163752 and 229288 KiB filesystem capacities respectively.
  These are lower than virtual disk capacities due to ext4 metadata. The initial
  probe allowed too little metadata overhead; the probe was corrected, while
  exact clone sizes and a local filesystem block-count regression verified growth.
- While running, reservations totaled 3 vCPU, 896 MiB RAM including VMM overhead,
  and 456 MiB disk. Stopping both released all CPU/RAM and retained all disk.
- Restarting the small VM preserved its marker and advanced its boot counter.
- A direct-agent request smaller than the base image was rejected before a VM
  record/clone was created. A request above host CPU capacity was rejected by
  the controller with HTTP 422.
- The base rootfs SHA-256 remained unchanged. Deleting both environments returned
  CPU, memory, disk reservations, and VM count to zero.

Temporary services, namespace, binaries, images, credentials and runtime/cgroup
parents were removed after verification. No test VMM remained.

## Local checks and scope

Workspace formatting, Clippy with warnings denied, all 126 Rust tests, release
builds of agent/controller/CLI, and the Rust 1.88 workspace check passed. A real
ext4 regression checks file preservation, exact filesystem growth, no-op sizing,
shrink refusal and invalid-image failure. Scheduler tests cover legacy-agent
capability refusal and configuration-drive capacity accounting across multiple VMs.

Large resource ceilings, performance, resizing an existing VM, other host platforms and other
storage engines were not tested or added by this change. Resource policy remains
with callers; TTstack checks generic engine support and host capacity.

Return to the [documentation index](README.md).
