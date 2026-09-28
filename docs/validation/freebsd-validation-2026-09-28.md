# FreeBSD restoration validation — 2026-09-28

PR #12 was rebased from `6b7439af0c6fec4f7075ec76c859f7abd745389e` onto
master before the corrections, then updated to `9b767cb1f43cc28872a188b6ec84c170161b179b`
after its documentation-only change.
The tested implementation is `6c966c43ed3487bb22534dc578ab01e927c10187`.
The source of all 39 tracked crate files was compared by SHA-256 with the native
build tree; every file matched. The last rebase changed no Rust source. This report does not extend the Linux support
commitment. Maintained FreeBSD behavior and prerequisites are in
[compatibility](../compatibility.md#experimental-freebsd-restoration).

## Host, isolation and artifacts

The explicitly authorized host ran FreeBSD 15.1-RELEASE-p3 on amd64, with four
logical CPUs and 12568940544 bytes of physical memory. Its existing gateway
VNET jail supplied international downloads. Native builds used a temporary
Rust 1.98.1 toolchain and vendored dependencies; curl test prerequisites were
extracted into the task directory rather than installed into the system.

A separate VNET jail held the agent, controller, PF rules, bridge and guest
roots. A private epair connected it to a controlled host-side client. Guest
clones, service databases, ports, temporary SSH keys and VMM metadata were
exclusive to this run. Only the test VNET's PF ruleset was enabled/configured;
the host PF ruleset remained disabled. No gateway configuration or service was
replaced. A temporary private devfs ruleset exposed the required devices.

This was functional testing only. Builds and the test jail were restricted to
three of four CPUs (75%, below the user's 80% ceiling). At most one bhyve VM
ran, at 1 vCPU / 512 MiB, or two lightweight Jails with 128 MiB scheduling
reservations each. Jail reservations are not enforced memory limits. There was
no load generation, concurrency benchmark or throughput test.

Native artifacts used the debug profile; Linux release packaging was checked
separately. Native SHA-256:

| Binary | SHA-256 |
| --- | --- |
| `tt` | `fcf44e2a6dc5b39afc65d2d8cd8210f1f5a13aecd02c41a40e7dd819f5db0a56` |
| `tt-agent` | `2977e8dd2a64f1d2003d377a5562af2156f92dc4a002efeb2f3eff653aafc46f` |
| `tt-ctl` | `3160d598adaa0a0cdad5886bebd5107919bf2ca6bbd65b5163a81e76d65e6b82` |

## Findings and results

| Case | Observation and correction |
| --- | --- |
| Rebase integration | The current resource-update API needed explicit handling for Bhyve/Jail. Their zero disk-accounting contract had to be retained during scheduling. Current Linux recovery, accounting and deletion protections were preserved. |
| Native compilation | The restored runtime called a Linux-only TAP query. A FreeBSD implementation and appropriate platform gates corrected the native build. |
| Image URL | The original recipe requested nonexistent `15.3-RELEASE` on a 15.1 host. The exact `15.1-RELEASE` URL under `amd64/amd64` was reachable. |
| Image failure/retry | An injected fetch failure left no published root. Repeating the recipe with the verified cached archive produced a complete root, enabled sshd, and a subsequent invocation recognized it. Two Jails booted that resulting root and accepted injected SSH keys. |
| Jail partial creation | Insufficient parent devfs delegation produced a retained failed record. Explicit deletion succeeded. Test-only delegation was corrected before the normal lifecycle run. |
| Jail lifecycle | Real rc startup, mapped SSH, stop/start and a synced marker survived. Agent/controller restart retained the guests and mapped access. The runtime cold-creates the retained root instead of trying to modify an already removed jail. |
| Multiple PF mappings | Two Jails each had SSH plus another mapped port. Both SSH mappings remained usable. Deleting the first left only the second guest's two anchors and its SSH connection still worked. Final deletion removed all guest anchors. |
| PF cleanup | Native `pfctl -s Anchors` returned qualified names. Cleanup now recognizes that form and matches the exact guest/port rather than an address substring. A native regression covers qualified names and near-colliding addresses. |
| Bhyve boot | Renaming a TAP interface did not rename `/dev/tapN`; passing the renamed interface to bhyve failed device initialization. Recording the character-device name fixed boot. A prepared FreeBSD UFS raw image accepted mapped SSH and reported one CPU and 496623616 bytes of physical memory. |
| Bhyve recovery | A running bhyve process kept its PID across agent restart. Stop/start retained a synced disk marker. The process and VMM device were removed before disk deletion, and final cleanup removed TAP/device metadata. |
| Stale bhyve PID | A task-owned control process PID was written into the VM PID file. Stop terminated the actual bhyve process and left the control process alive. |
| Startup failure reporting | The native device-initialization failure was reported as a failed VM with a retained log and resources, rather than a successful running VM. Deletion cleaned the failed instance. |
| Egress option | Shared-IP Jail could still initiate a connection with the old deny rule. A bhyve ingress-block variant broke replies to mapped SSH; removing it restored access. The final API/CLI reject `deny_outgoing` for both FreeBSD engines with HTTP 400. This run does not claim FreeBSD egress isolation. |
| State locking | A second agent using the same data directory was rejected before listening. Unit coverage also verifies release on drop. |

The base archive SHA-256 was checked against the official release `MANIFEST`:
`3768988b151c20f965679062b065c63a977d6bbb9f47fd83695ec2c40790c18f`.
The recipe test used a private fetch wrapper that checked the exact requested URL
and served this cached archive, avoiding a repeated large download. It is evidence
for failure/retry, extraction and publication, not a completed end-to-end download
through the host's slower international connection. The bhyve fixture was built
from that base with the matching host kernel, prepared networking and a task-only
SSH key; it is not a new TTstack image recipe.

## Checks and limits

Linux: **168 tests** (CLI 21, agent 32, controller 48, core 67), formatting,
Clippy for all targets with warnings denied, workspace check, Rust 1.88 workspace
check and locked release build passed. Linux ext4/configuration-drive tests ran
with their required tools.

FreeBSD: native workspace build and **158 tests** (CLI 21, agent 31, controller 48,
core 58) passed. The Linux-only Firecracker/ext4 lifecycle cases run on Linux,
not as skipped passes on FreeBSD. Core/test targets also passed FreeBSD-target
Clippy with warnings denied from Linux. Native tests ran outside the pre-existing
gateway jail because its loopback address was unavailable to those tests.

Stop confirmed process exit and retention of explicitly synced data. Jail uses
`rc.shutdown`; bhyve requests ACPI poweroff and has a bounded forced-termination
fallback. The captured guest console did not establish an orderly shutdown of
every guest application; this is not an application consistency guarantee.

No FreeBSD zvol lifecycle, host reboot, memory pressure, untrusted-tenant
isolation, Linux guests on bhyve, UEFI boot, fleet performance or native release
build was tested. Existing Linux live reports retain their original scope.

## Cleanup

The final API inventory was empty, with zero VM, CPU, memory and disk
reservations. Guest roots/disks, bhyve processes/devices, TAP interfaces,
per-guest PF anchors and runtime metadata were removed. The isolated services,
test jail, epair, temporary device rules, test-only kernel modules and task files
were then removed. The original gateway jail, VPN, SSH service and host
interfaces remained present; fresh direct and gateway-jail SSH commands worked.
