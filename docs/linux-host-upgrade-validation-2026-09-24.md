# Linux host upgrade validation — 2026-09-24

This record validates the consolidation of host support on Linux, on top of
`cb648c5`. The accompanying commit contains the implementation and this report.
The retained engines are QEMU/KVM, Firecracker, and Docker/Podman. This is functional
upgrade evidence, not a performance or maximum-capacity test.

## Local verification

- All **128 workspace tests** passed with locked dependencies, covering every
  workspace target. Clippy passed with warnings denied, formatting passed, and
  the Rust **1.88** check passed for all workspace targets.
- Locked release builds of CLI, controller and agent passed. Rust documentation
  generated with warnings denied; the documentation-test target also passed.
- Added state tests cover all three retained engine values, agent database reopen,
  and rejection of unknown engine values without changing or dropping their rows.
  Canonical JSON values and CLI aliases retain their previous meanings.
- A generated deployment script was executed with a simulated unsupported host
  OS and rejected it before user, directory, or service setup.
- Release CLI help and image recipes contained only current choices. Removed
  engine selectors and an obsolete recipe were rejected; recipe rejection did
  not create files.
- A Chromium check exercised the dashboard against mock API responses. The engine
  selector contained exactly three choices; SSH-key, disk and outgoing-network
  controls and submitted creation payloads matched each engine. No JavaScript
  errors occurred.
- Source, manifest, documentation and Claude workflow scans found no obsolete
  platform implementation or references. Local documentation links resolved.
  Restricting `nix` to its used signal feature also removed two unused transitive
  packages from the lockfile without changing retained dependency versions.

## Live environment and limits

One authorized Ubuntu x86_64 host with KVM, cgroup v2, 32 logical CPUs and roughly
123 GiB RAM was used. The other two authorized machines were not needed. Controller
and agent ran as temporary systemd units with dedicated SQLite directories. VM
networking, bridge, TAPs, forwarding and isolation rules lived in a separate network
namespace. API listeners were not exposed on the host's public interface.

QEMU 8.2.2, its helper packages, Firecracker/jailer 1.17.0 and kernel 6.1.186 were
placed under a temporary directory; no host package installation was performed.
The isolated QEMU fixture initially lacked VGA/PXE firmware lookup paths. Linking
its extracted firmware inside that temporary directory resolved the launch errors;
failed test environments were explicitly deleted before retrying.

| Workload | vCPU | Guest/container memory | Root disk |
| --- | ---: | ---: | ---: |
| Alpine 3.21.7 QEMU | 1 | 384 MiB | 2048 MiB qcow2 |
| BusyBox Firecracker HTTP fixture | 1 | 256 MiB | 192 MiB ext4, grown from 128 MiB |
| Python HTTP Docker fixture | 1 | 128 MiB | Retained container writable layer |
| Additional new-release Firecracker | 1 | 256 MiB | 256 MiB ext4 |

At most four workloads ran simultaneously: 4 vCPU and 1280 MiB of memory
reservations including Firecracker overhead. The agent service had a four-core,
4-GiB ceiling; Firecracker and Docker retained their own per-instance limits.
Only boot, a few HTTP/SSH requests and tiny file writes were exercised.

## Upgrade and lifecycle results

1. Release `cb648c5` created QEMU, Firecracker and Docker workloads. QEMU SSH key
   access, mapped SSH/HTTP ports, Docker HTTP access, and persistence markers passed.
2. Agent/controller binaries were replaced with the new release while preserving
   their databases and guest disks. All three workloads retained their VM IDs and
   process IDs; application access and markers remained intact. The Firecracker
   boot counter stayed at one, confirming the upgrade did not reboot it.
3. Both the controller and agent returned HTTP 422 for removed engine selectors
   before allocation; VM count remained unchanged.
4. Stopping all three released CPU/RAM and retained 2244 MiB of disk reservations,
   including the configuration drive. Starting them preserved the QEMU/Docker
   markers and advanced the Firecracker boot counter to two.
5. New-release Firecracker creation grew the ext4 clone and served HTTP. Repeating
   the same direct-agent request reused its VM; changing the request was rejected.
6. New-release CLI creation, application access and deletion passed separately for
   QEMU and Docker. QEMU still added the default SSH port and injected its key.
7. All environments and agent VM records were deleted. Resource reservations and
   VM count returned to zero. Controller and agent SQLite integrity checks passed.

## Cleanup and scope

Temporary units, network namespace/veth, tool packages, images, runtime directories,
credentials and empty runtime/cgroup parents were removed. No test VMM remained.
The existing Docker container and image inventories exactly matched their baseline.
Existing host services were preserved; no permanent deployment was left behind.

This run did not exercise Podman, ZFS volumes, host reboot, other architectures,
large resource configurations or performance. ZFS remains a generic Linux QEMU
storage option; Firecracker's jailer remains part of its isolation boundary.
Old state with unsupported engine values needs an explicit pre-upgrade cleanup
using a compatible release; it is not silently migrated to another engine. See
[upgrade compatibility](deployment.md#upgrade-compatibility).

Return to the [documentation index](README.md).
