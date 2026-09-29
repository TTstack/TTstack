# Capability tags and mixed-fleet validation — 2026-09-29

The tested implementation is `1f2f6a9268fcced3ab94e58f2565839941590834`
on PR #13, based on master `33ee2892c8add821a7428b1fba3b612e5f43f9fd`
after the experimental FreeBSD merge. The maintained contract is
[technical capabilities](../capabilities.md); the corrected design rationale is
in the [proposal](../proposals/capability-tags.md). This report records functional
evidence for that revision, not additional platform support.

## Hosts, isolation and bounds

One isolated Linux controller managed agents on two authorized Linux hosts and
one authorized FreeBSD host. Linux A used file storage on a private XFS/reflink
loop device; Linux B used a private file-backed ZFS pool and zvol guest disks.
The FreeBSD host first used delegated ZFS/zvol storage for bhyve, then a separate
file-storage agent for Jail. The empty zvol agent was unregistered before the
file agent was registered; these were two configurations of the same third host.

| Host | Native environment | Test budget and observation |
| --- | --- | --- |
| Linux A | Ubuntu, Linux 6.8, x86_64; 32 logical CPUs, approximately 123 GiB RAM | Aggregate task slice: 4 CPU cores, 6 GiB RAM, 1024 tasks. Recorded slice memory peak: 1,719,848,960 bytes. |
| Linux B | Ubuntu, Linux 6.8, x86_64; 32 logical CPUs, approximately 123 GiB RAM | Same aggregate limits. Recorded slice memory peak: 733,532,160 bytes. |
| FreeBSD | FreeBSD 15.1-RELEASE-p3, amd64; 4 logical CPUs, 12,568,940,544 bytes RAM | Live services and guests pinned to one CPU; at most one 512-MiB bhyve guest or one Jail with a 128-MiB scheduling reservation. Native suites used bounded concurrency. Free memory remained above 9 GiB. |

Existing load was inspected before setup; these limits left total usage below
the requested half-host budget. No stress or throughput test ran. Linux agents,
VMMs, fixture services, Docker and containerd ran under the task slice; actual
VMM/container cgroup membership was checked. Both ends of each Linux test veth
were inside private network namespaces. Firewall, bridge and published-port
changes remained in those namespaces. Mount namespaces isolated the fixed guest
runtime path, loop mounts and jailer cgroup view.

FreeBSD used a dedicated VNET jail, epair, delegated task ZFS subtree, private
devfs rules, service state and ports. PF operations were inside that VNET jail.
The pre-existing gateway jail and SSH service remained available. Test API keys
and guest keys were temporary and are excluded from this report. No host
packages, existing services or host firewall policies were replaced.

Linux used release binaries. FreeBSD debug binaries and test executables were
cross-built with Rust 1.98.1, Clang/LLD 18 and a FreeBSD 15.1 sysroot, then executed
on the native host. Public CA files and privately extracted curl dependencies
supplied the isolated FreeBSD runtime prerequisites.

| Artifact | Linux release SHA-256 | FreeBSD debug SHA-256 |
| --- | --- | --- |
| `tt` | `268020b9a085384571b8f5233c5e657871985529e1d99d098e981c88b8e08f12` | `999d0943e16f722e1bd102f27adf77fdc204c2a070a0a26efc3ca2d1617bdd93` |
| `tt-agent` | `b0da2148a71dee4f99d3092729ea907100c5270bad0e8aa1a740dcd801203dd7` | `4c682991ddfbe9eb915c9e39410540cbd450e27862c30e2ef76862f8bc05f58b` |
| `tt-ctl` | `e7940448d9fc11795a4fad8311d92ea1621cacd9dd45b0b82d49f1da40c87e83` | `97d9134f834e435ab32f8b9d183986fae8905a29e4c296a13a0feb72505adffc` |

## Build and contract checks

| Check | Result |
| --- | --- |
| Linux workspace tests | 221 passed: core 89, agent 54, controller 57, CLI 21. Configuration-drive tests executed with e2fsprogs available. |
| FreeBSD native test execution | 211 passed: core 80, agent 53, controller 57, CLI 21. Linux-only engine/network tests were excluded by target configuration. |
| Compiler gates | Formatting, all-target/all-feature Clippy for Linux and FreeBSD, Linux Rust 1.88 checking and Linux locked release build passed. All six GitHub CI checks passed on the tested commit. |
| Platform selection | Workspace crates introduce no OS feature switches. Native engine factories and networking use `cfg(target_os)`; shared models and the design matrix remain usable by either controller. Opposite-platform engine construction is rejected before host probing. |
| Report interpretation | Regression suites cover engine/storage scope, unknown tags and report versions, duplicate/missing entries, storage mismatch, authoritative denials, explicit legacy mapping and requirements derived from SSH options. |
| Recovery boundaries | Regression suites cover accepted create/resource-update retries after capability withdrawal, backup admission versus protocol support, retained state and exact replay. These are distinct from fresh-operation admission. |
| API, CLI and dashboard | An authenticated live matrix endpoint exposed all ten engine/storage combinations. Unauthenticated access returned 401. CLI JSON matched the API; dashboard smoke checks exercised matrix-driven controls, including Firecracker SSH and unsupported Jail fields. |

## Native lifecycle and failure cases

| Case | Observed result |
| --- | --- |
| One environment across three hosts | QEMU/file on Linux A, Firecracker/zvol on Linux B and bhyve/zvol on FreeBSD reached working mapped SSH and persisted guest markers. Firecracker supplied a custom sudo user and opaque configuration content. |
| Backup and resource contracts | Running backup creation returned 409. Stopped QEMU/Firecracker updates changed supported resources; over-budget updates returned 409. bhyve resource updates returned 400. Stopped backups and exact replay retained the same generation. |
| Restore across the fleet | Guests booted and changed a synced marker, stopped, restored the retained generation, and cold-started with the original marker. Restore replay remained idempotent. Environment deletion removed guests, backups and reservations. |
| Alternate Linux storage combinations | Firecracker/file on Linux A and QEMU/zvol on Linux B completed SSH, stop/start, backup, restore, CPU/memory updates and later disk growth. Disk growth with a retained backup returned 409; deleting the backup allowed growth. Firecracker implicit configuration-disk accounting survived restore. |
| Network isolation and configuration | Both alternate Linux guests reached a controlled routed peer endpoint while access to the controlled host-gateway endpoint was denied. Published SSH replies worked. Firecracker configuration remained read-only. |
| Controller and agent recovery | Restarting the isolated controller preserved VM IDs. Stopping the Linux B agent left its QEMU guest accessible over SSH; the controller retained historical host capabilities and reservations, marked the host offline and rejected new placement with 422. Agent restart recovered the retained guest. |
| Canonical and legacy compatibility | A test-only FreeBSD report proxy supplied an unknown version or an explicit scoped egress denial: both rejected fresh placement with 422 and no allocation. Unknown tags survived transport. Omitting the report exercised the legacy mapping and successfully placed bhyve with outgoing denial and working mapped SSH. bhyve isolation remained rejected. |
| Backup admission withdrawn | Restarting the FreeBSD agent with backup admission disabled still allowed an accepted operation's exact replay and restoration of an existing backup. New backup creation returned 400. Deletion released all reservations. |
| File-backed Jail | The third host's file agent advertised root-key injection and rejected outgoing denial, guest configuration and isolation. Explicit default-root SSH options normalized correctly and produced working SSH. Controller and direct-agent backup requests and resource updates returned 400 without changing backup state. A synced marker survived cold start; agent restart preserved the live Jail. Deletion removed it. |
| Missing host prerequisite | A task-only failing `mkfs.ext4` wrapper, followed by Linux A agent restart, disabled the Firecracker configuration capability with a reason. Fresh SSH-bootstrap requests returned controller 422/direct-agent 400 without a new VM record. An existing Firecracker guest still cold-started. Removing the wrapper and restarting restored the capability. |
| Per-disk qualification | A task-owned qcow2 backing-chain fixture booted on a host qualified for file backups. Backup creation nevertheless returned 400 because the disk was not standalone. Guest contents remained unchanged and restart succeeded. |
| Docker lifecycle | Unsupported disk sizing, SSH keys and outgoing denial returned 400. A container on the dedicated runtime served HTTP through its assigned port, ran inside the task cgroup, retained a filesystem marker across stop/start, rejected backup creation and was removed by environment deletion. |
| Automatic expiry | A separate running Docker environment with a 20-second lifetime disappeared within the bounded 90-second observation window. Its container and published mapping were removed. |

Firecracker agent logs contained no forced-stop fallback messages; tested bhyve
stops likewise completed without the forced-termination message. Jail used its
fixture shutdown path. QEMU stop success and retained data were checked, but this
run does not independently classify every QEMU shutdown as orderly guest shutdown.
Docker environment deletion and expiry exercised the engine's force-remove path.
None of these checks promise preservation of guest memory.

## Fixture corrections

Initial Docker setup exposed two fixture problems. A separate Docker daemon
automatically selected the system containerd socket, whose mount context could
not execute the isolated image. The failed task environment was inspected and
deleted through the API; only the task-imported image was removed. Dedicated
containerd state, socket and namespaces, sharing the test daemon's private mount
namespace, corrected that setup without restarting system Docker or containerd.

The Alpine fixture's BusyBox also lacked `httpd`. Its failed creation remained
inspectable and was deleted before replacing the fixture command with a small
static HTTP server. The final Docker lifecycle and expiry run passed. These
fixture failures are not counted as successful application-readiness tests or
product defects. No TTstack behavior change was needed after the tested commit.

## Cleanup and limits

The controller and all four agent configurations ended with empty inventories
and zero disk reservations. Task guests, backups, containers, services, storage,
mounts, cgroups, network namespaces, forwarding tunnels and temporary keys were
removed. FreeBSD's delegated dataset, VNET jail, epair, devfs rules, extracted
tools and task-loaded kernel modules were removed; its original module list was
restored. Fresh direct SSH and commands through the original gateway jail worked.

On each Linux host all 25 services that were running at setup were still active.
System Docker, containerd and SSH retained their original process IDs. Root
interfaces matched the pre-test inventory. Stateless root firewall rules matched
the snapshot taken after the initial mixed-fleet phase. Initial raw firewall
hashes contained timestamps/counters and were not used to claim a byte-identical
pre-test policy comparison. No task reset or modified root firewall policy.

This is bounded functional validation, not capacity testing or a security
certification. It covers these two Linux hosts and FreeBSD 15.1-RELEASE-p3 amd64,
not Podman, other architectures/FreeBSD versions, native FreeBSD MSRV or release
builds, host reboot/power-loss durability, or untrusted-tenant isolation. The
FreeBSD clock differed from the Linux controller; host clocks were not changed,
and expiry was validated using controller-owned timing. FreeBSD remains
**experimental**; file reflinks and Jail backups remain unsupported.
