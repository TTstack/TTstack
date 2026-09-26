# Engine capability probes — 2026-09-26

Native-engine feasibility tests on two explicitly authorized Linux hosts, A and B.
The code review baseline was TTstack `8009daa`. These experiments invoked native
engines and temporary network rules; they did **not** test a new TTstack API
implementation or enable new product capabilities. The existing
[offline resource contract](../rest-api.md#offline-resource-updates) is unchanged.

## Environment and bounds

Both hosts had Ubuntu 24.04.4, Linux 6.8.0-138-generic, 32 logical CPUs,
126380 MiB RAM, cgroup v2 and accessible KVM. Initial load was approximately zero.

| Component | Tested version/configuration |
| --- | --- |
| QEMU | 8.2.2, KVM, one guest at a time per host |
| Guest | Alpine 3.21.7 NoCloud BIOS cloud-init image |
| ZFS userspace / kernel module | 2.2.2-0ubuntu9.5 / 2.2.2-0ubuntu9.4 |
| Docker | 29.1.3, separate daemon/socket/data root, vfs storage, systemd cgroups |
| Podman | 4.9.3, separate storage/run/config roots, vfs, cgroupfs |
| Podman network / OCI runtime | Netavark 1.4.0, Aardvark DNS 1.4.0, runc 1.3.4 |
| Container fixture | Alpine 3.21.3 minirootfs with a read-only host BusyBox HTTP server |

Each host's test processes and containers belonged to a dedicated slice capped at
two CPU equivalents, 2 GiB RAM, no swap and 512 tasks. Container cgroup paths were
explicitly placed below that slice. ZFS ARC was separately capped at 256 MiB
because kernel cache is not bounded by the service memory limit. Downloads were
limited to 4 MiB/s. There was no stress or throughput test.

Recorded slice memory peaks were 1142956032 bytes on A and 1168642048 bytes on B
(approximately 1.06 and 1.09 GiB). Recorded slice CPU time was approximately 45 and
41 CPU seconds respectively. ARC was approximately 128 MiB before teardown.
These figures include test preparation within the slice and are not benchmarks.

Docker, Podman and a simulated external client had separate network namespaces.
Veth links connected only test namespaces; none connected to the host's default
namespace. Public-address HTTP and DNS endpoints existed only inside the client
namespace, with no connection to the Internet. All firewall mutations occurred
inside test namespaces. The existing Docker daemon was not reconfigured or
restarted.

Each host had a separate, unpartitioned NVMe with no filesystem signature,
mountpoint, holder or open user found during preflight. With explicit user
authorization, each became a temporary ZFS pool with an 8 GiB dataset quota and
a sparse test zvol. This was real block-device testing, not a file-vdev substitute.

Tools were extracted into a temporary directory. The missing `libyajl2` runtime
dependency was installed with user authorization and remains installed. The ZFS
module was initially unloaded, loaded with the ARC cap, and unloaded at cleanup.

## QEMU offline resizing

The following sequence was observed on both hosts for both qcow2 files and ZFS
zvols:

| Step | Observed result |
| --- | --- |
| Boot at 1 vCPU, 256 MiB RAM, 2 GiB disk | SSH ready; one CPU; disk has 4194304 sectors |
| Write a marker and sync | Marker saved in the guest root filesystem |
| Stop; grow disk to 3 GiB; boot at 2 vCPUs / 384 MiB | Two CPUs; increased guest memory; 6291456 disk sectors |
| Inspect the mounted root filesystem | Capacity increased from 1958375 to 2940901 KiB; marker retained |
| Stop; boot at 1 vCPU / 256 MiB with the same 3 GiB disk | CPU/RAM reduced; expanded filesystem and marker retained |

File growth used `qemu-img resize`; zvol growth used `zfs set volsize`.
CPU/RAM changes used new `-smp` and `-m` arguments on the next QEMU process.
The Alpine image expanded its own filesystem at boot. This does not establish
automatic filesystem growth for arbitrary partition layouts, LVM, encrypted
guests, other filesystems or other distributions.

Both orderly guest shutdown and forced fallback occurred. Some Alpine shutdowns
did not terminate QEMU within the harness's 30-second grace period; after a prior
guest sync, the test service stopped the VMM and confirmed its exit before disk
mutation. Retained-data results cover those occurrences, not guaranteed orderly
shutdown of every workload. The harness grace period is not TTstack's shutdown
configuration.

The extracted ZFS tools did not install system udev rules. The harness resolved
the task zvol with `zvol_id` and used a private symlink to its block device. No
system disk was substituted for the test zvol. Snapshot-clone lifecycle and
TTstack crash recovery were not exercised by these native probes.

## Container resource updates

Docker passed on both hosts:

- Stop a container at 1 CPU / 64 MiB; update to 2 CPUs / 96 MiB with matching
  `--memory-swap`; repeat the target; restart the same container.
- Observe `cpu.max` of `200000 100000`, `memory.max` of `100663296` and
  `memory.swap.max` of `0` from inside the container.
- Stop and reduce to 1 CPU / 64 MiB; observe the reduced limits after restart.
- Preserve container identity and the written marker throughout.
- Restart only the test Docker daemon, start the retained container, and observe
  the same marker and configured limits.

Podman 4.9.3 did **not** pass the equivalent offline operation on either host.
Updating an exited container returned exit status 125: runc reported that the
container did not exist. Inspect still reported the old CPU quota and 64 MiB
memory limit. Starting again also retained those old limits. A native update
while running succeeded, and the container then reported 2 CPUs / 96 MiB limits.
Thus an available `podman update` command does not prove stopped-container support.
Newer Podman versions were not tested.

The HTTP fixture did not handle PID-1 SIGTERM as an application shutdown request;
bounded container stops used the runtimes' forced-stop fallback. This is not
application graceful-shutdown coverage.

## Container networking

Each network case had a working positive baseline before interpreting blocked
traffic. The final results below were the same on both hosts.

| Setup | Published TCP ingress and reply | Public HTTP initiation | Public DNS lookup |
| --- | --- | --- | --- |
| Ordinary native bridge | Allowed | Allowed | Allowed when a test DNS server was configured |
| Native internal bridge | Blocked | Blocked | Not a compatible replacement for the existing API contract |
| Docker bridge + original-direction egress drop + explicit public DNS | Allowed | Blocked | Blocked |
| Podman bridge + original-direction egress drop + default Aardvark proxy | Allowed | Blocked | **Allowed: proxy bypass** |
| Podman bridge with native DNS disabled + explicit public DNS + egress drop | Allowed | Blocked | Blocked |

Native internal networks also allowed access to the bridge gateway's HTTP service
in the default-DNS tests. They therefore cannot simply stand in for either
`deny_outgoing` or `isolated_network`.

The filtering prototype used a stable, task-owned bridge name, native IPAM/NAT
and port publishing, and a separate nftables table. It accepted reply-direction
traffic and dropped container-originated routed traffic for the deny case.
For the isolation case, separate INPUT and FORWARD rules blocked host services
and the tested private destinations while permitting public IPv4 HTTP. These
rules did not edit Docker/Netavark tables.

DNS required a real backend distinction. With Podman's default Aardvark proxy,
the query could leave through the proxy despite the container-forwarding drop.
Blocking host access instead broke DNS. Creating the Podman network with
`--disable-dns` and supplying the public DNS server directly removed both issues:
DNS worked under the isolation policy and was blocked under deny-outgoing.
Docker's explicit public upstream, reached through its embedded resolver, obeyed
the tested forwarding restriction.

Docker's prototype also passed test-daemon restart and a fresh container's first
application-command denial. Podman's direct-DNS prototype passed stop/start with
published ingress preserved and public HTTP/DNS initiation still blocked.

These are feasibility results, not full network-policy acceptance. IPv6,
cross-peer spoofing, all non-public address ranges, rootless networking, alternate
firewall backends, host reboot and policy reconstruction after namespace loss were
not validated. No comparison proved complete equivalence with all existing VM
network behavior.

## Selected implementation direction

1. Extend the existing stopped-VM update transaction to QEMU, reusing its storage
   size/growth helpers and cold-boot CPU/RAM parameters. Keep disk growth separate
   from guest filesystem readiness; no guest agent or universal offline filesystem
   editor is needed for the initial scope.
2. Use native Docker CPU/RAM update plus inspect confirmation. Permit resource
   requests without a disk field, retain a normalized pending target for recovery,
   and advertise support for the actual selected runtime rather than assuming
   Docker and Podman are interchangeable.
3. Do not enable stopped-container updates for the tested Podman 4.9.3. Do not
   start an application merely to work around its offline-update failure. A newer
   version needs its own acceptance test before advertising this capability.
4. Retain the current Docker/Podman rejection of `deny_outgoing` and
   `isolated_network` for the initial implementation. Native internal networks
   are not equivalent. The bridge/filter/direct-DNS candidate passed the listed
   probes, so it is not inherently impossible, but it needs additional lifecycle
   and policy coverage before enabling a shared API promise. Do not substitute a
   weaker or differently scoped network mode under the existing option names.
5. Leave container disk quotas out of this change; no container disk-quota or
   storage-driver migration was tested.

These choices favor the existing lifecycle transaction and small engine-specific
operations. They do not require a new resource framework, network controller or
runtime orchestration subsystem. They are implementation decisions from this
experiment, not claims that those features are already shipped.

## Cleanup and verification

All task containers were removed and both temporary runtime inventories were
empty before teardown. Test VMs, daemon, helper services, namespaces, firewall
tables, cgroups, temporary files and guest keys were removed. The temporary ZFS
pools were destroyed; only the partition tables and ZFS signatures created by this
run were cleared, returning the previously blank test disks to an unpartitioned
state. No test pool or mount remains.

Post-test checks confirmed unchanged host IP addresses, routes and firewall rule
structure, unchanged existing container inventory, and all pre-existing running
services still running, including SSH and Docker. Address comparisons excluded
normal IPv6 lease-lifetime countdowns; firewall comparisons excluded counters.
Fresh SSH connections succeeded after teardown.

No new engine capabilities were implemented or enabled by these probes. A
separate controller regression test checks the existing default, 30-day, 10-year
and permanent environment lifetimes through creation and an expiry sweep. That
check does not change the existing lifetime policy. Formatting, Clippy, workspace
tests/checks, report links and the final diff were checked locally.
