# FreeBSD 15.1 zvol and bhyve egress validation — 2026-09-28

**Live-tested only on FreeBSD 15.1-RELEASE-p3 (amd64). Other FreeBSD versions
have not been live-tested.** This follow-up extends and corrects the
[initial PR #12 validation](freebsd-validation-2026-09-28.md). Maintained behavior
and PF prerequisites are in [compatibility](../compatibility.md#experimental-freebsd-restoration).

## Revision and artifacts

Tested implementation: `420333c7935c599f1d97b239cc0d3fdeffa1607e`, following
`68a02eceb5e361dcbdd76985549cd2dbe9dcacd4`. SHA-256 of
`git diff --binary 68a02ec 420333c -- Cargo.toml Cargo.lock crates`:

`07497baeab00810d9a7499ecbce909a54f2274fcb41e63f545ab07a452de5973`.

All 39 tracked crate files were compared with the native build tree by SHA-256
and matched. Native builds used Rust 1.98.1, locked vendored dependencies and the
debug profile. The native binaries were:

| Binary | SHA-256 |
| --- | --- |
| `tt` | `7aa46832c1de84564b2c9973dbc15f4c46e953197d271096843b2929bd12678e` |
| `tt-agent` | `38611e0b4c85c5b3be917434cb4de1aa8bcee3fc14231252844a3bced4ac387f` |
| `tt-ctl` | `1b61fa2b97827ae41e74abecbc9e0c914b0e3daee4dd697bc6541e5acdec530f` |

## Isolation and bounds

The authorized machine had four logical CPUs and 12568940544 bytes of physical
memory. Its existing ZFS pool was healthy, with about 237 GiB free. A dedicated
8 GiB-quota dataset tree held only the test image and runtime volumes. The tree
was delegated to a separate VNET jail; no existing dataset was repurposed and no
pool or physical disk was provisioned. Private devfs rules exposed the test
zvols and required PF/TAP/VMM devices.

Agent/controller state, ports, binaries, SSH keys and roots were task-owned.
An isolated epair connected the test jail to a controlled host-side client and
TCP/UDP endpoints. PF changes were confined to that VNET. The gateway jail
provided international downloads; its configuration was retained.

This was functional validation only. Builds and the test jail used CPU affinity
to three of four CPUs. At most one bhyve guest ran, at 1 vCPU / 512 MiB; a final
Jail forwarding regression ran after the VM was deleted. No stress, throughput
or concurrency benchmark was performed.

The 1 GiB UFS guest fixture used the official 15.1 base archive and matching host
kernel, with static guest networking and a temporary SSH key. The base archive
SHA-256 matched the earlier verified release artifact. Raw bytes were imported
into a **sparse zvol**, and the base was made read-only before provisioning.
This was not a regular disk file stored on a ZFS filesystem.

## Zvol results

| Case | Observed result |
| --- | --- |
| Native volume creation | The device appeared as a FreeBSD character device. The catalog discovered the base through the agent's `zvol` backend. |
| Controller/agent provisioning | TTstack created `@ttsnap` on the base and a runtime clone whose `origin` referenced that snapshot. The clone was writable while the base remained read-only. |
| Real guest boot | bhyve booted the cloned zvol and accepted mapped SSH. `procstat -f` showed the VMM holding the runtime `/dev/zvol/...` character device. |
| Stop/start retention | A synced marker remained readable after cold restart. The clone and its snapshot origin were retained on stop. |
| Shutdown | With the process-query correction, the stop call took approximately seven seconds. The guest console reported filesystem synchronization, all buffers synced and ACPI poweroff. No forced-termination fallback was observed for this stop. |
| Running agent restart | The VMM kept its PID, SSH remained accessible, and the agent reported the VM as `running`. |
| Deletion blocked by a dependency | A task-created snapshot and dependent clone prevented ZFS destruction. The VMM stopped, but the source clone and a `deleting` VM/environment record remained with the dependency error. |
| Restart while deletion was blocked | Restarting both isolated services retained the `deleting` state and both volumes. |
| Retry after removing the dependency | Removing only the task-owned dependent clone allowed automatic cleanup to remove the original runtime clone and its temporary snapshot. The environment disappeared and VM reservations returned to zero. |
| Base preservation | Guest deletion retained the read-only base and its `@ttsnap`, without remaining dependent clones. The base was removed only during final fixture cleanup. |

The declared bhyve disk reservation remains zero; these tests do not establish
logical disk admission control. The separate ZFS quota bounded the experiment.
Bhyve creation-time disk sizing and offline resizing remain unsupported.

## Process identity correction

The prior query used `ps -axo pid=,stat=,comm=,args=`. Native FreeBSD interpreted
the text after the first `=` as one column header and returned only PID values.
The old parser skipped those rows, allowing a live VMM to appear stopped.

The correction uses separate `-o` arguments and unlimited output width. Empty or
malformed inventories are errors, not proof of exit. A regression executes the
real native command, in addition to checking process-title parsing. A live test
replaced the VM PID file with a task-owned control process PID: stop terminated
the actual VM and left that unrelated process alive.

## Engine-specific outgoing restrictions

`deny_outgoing` is restored for **bhyve**, gated by the
`bhyve_deny_outgoing` agent capability. **Jail remains unsupported** and its API
request returned HTTP 400. Linux QEMU/Firecracker behavior is unchanged.

Two PF details caused the earlier unsuccessful probes:

- A `quick` modifier on the wildcard anchor stopped traversal before subsequent
  child anchors. The corrected prerequisite is `anchor "ttstack/*"` without
  `quick`; individual guest filter rules retain `quick`. The agent refused the
  old hook before guest boot, retained the failed record, and allowed cleanup.
- Floating NAT state alone did not admit the guest's reply at the internal
  interface when ingress blocking was enabled. Explicit interface-bound states
  on the inbound and outbound forwarding legs preserve those replies.

A guest created through the CLI with `--deny-outgoing` passed these checks:

| Probe | Result |
| --- | --- |
| Mapped inbound SSH | Login and command replies worked. |
| New TCP connection to the controlled external endpoint | Refused; the same endpoint was reachable without the option. |
| UDP probe to the controlled external endpoint | Did not arrive and incremented the deny-rule counter. A newline-terminated positive-control probe without the option reached the listener. |
| Connection to the agent's bridge address | Worked, retaining the host-local access contract. |
| Agent restart | Same VMM PID; incoming SSH and denied TCP initiation remained correct. |
| Guest stop/start | The marker, incoming SSH and denied TCP initiation survived the cold restart. |
| Additional unrelated child anchor | Did not bypass the corrected deny rule or break incoming SSH. |
| Final guest deletion | Both filter and translation rules, the deny anchor, TAP and runtime metadata were removed. |

Rule-level probes also exercised bridge member/bridge filtering both enabled
and disabled. The end-to-end API run used the host's original disabled settings.
The updated forwarding rules additionally passed a real Jail SSH/create/delete
regression on file storage. The nested test jail's devfs delegation was adjusted
only for that fixture; an initial rejected mount was cleaned up before retry.

This validates routed **IPv4** initiation and published TCP replies. It does not
claim IPv6 blocking, source-spoofing prevention, peer isolation or revocation of
pre-existing PF states. Operator NAT and firewall prerequisites still apply.

## Checks and cleanup

Linux passed **169 tests** (CLI 21, agent 32, controller 49, core 67), formatting,
workspace check, all-target Clippy with warnings denied, Rust 1.88 workspace
check and locked release build. Native FreeBSD passed the workspace build and
**161 tests** (CLI 21, agent 31, controller 49, core 60). FreeBSD-target core/test
Clippy also passed. Linux-only ext4 cases ran on Linux.

Final agent inventories and VM reservations were empty. Runtime clones, guest
snapshots, processes, TAPs, PF child anchors and metadata were gone before fixture
cleanup. The test dataset tree, base volume/snapshot, isolated services and jail,
epair, private device rules, temporary modules, toolchain and task files were
removed. The original pool remained healthy; the gateway jail, VPN, SSH service,
interfaces, device rules and kernel-module inventory were preserved.

No other FreeBSD release, host reboot, storage exhaustion, memory pressure,
Linux guest on bhyve, UEFI boot, native release build or performance limit was
validated by this run.
