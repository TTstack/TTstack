# FreeBSD master port validation — 2026-09-28

PR #12 was rebased from `76423e54e00d829e81755a03be0231f42a44b235`
onto master `05c5a432e5647245058a5c74d9fe5cb7064a59d0`. The tested
implementation is `c48a075` (`fix(freebsd): port master lifecycle and ownership
safeguards`). The branch retains its existing workspace version **0.6.0**.
Historical reports retain their original tested revisions and limitations.

## Port scope

The branch inherits master's native agent database format, persistent container
runtime/network ownership, per-VM reconciliation, scoped controller refresh,
Firecracker SSH-disk accounting, strict disk resolution and CLI access/expiry
output. The rebase preserves the FreeBSD lifecycle and PF implementations.

The port restores the previous merge's shared SSH normalization: Jail root keys
do not request Linux SSH bootstrap or require its capability. Bhyve uses the
fallible disk resolver, while Jail retains a root-directory path. Regression
coverage checks missing, ambiguous and linked bhyve file disks. FreeBSD rejects
Linux outer SSH network namespaces before accepting network configuration, and
network-binding tests no longer depend on Linux `/proc` paths.

The database initialization policy and its pre-production rationale are inherited
from master; this work adds no migration or automatic cleanup. See the
[original transition record](native-agent-schema-2026-09-28.md).

## Host and bounds

The authorized machine ran FreeBSD 15.1-RELEASE-p3 on amd64, with four logical
CPUs and 12568940544 bytes of physical memory. The user allowed an approximate
80% memory ceiling and subsequently removed the CPU ceiling. No CPU affinity or
stress workload was used. Memory samples remained above 9 GiB free; no OOM was
observed. Tests used two test threads, and service/test processes had a 3 GiB
virtual-memory limit. Functional cases used at most one 1-vCPU/512-MiB bhyve guest
or two lightweight Jails with 128-MiB scheduling reservations each. Jail
reservations are not enforced memory limits.

A dedicated VNET jail, epair, private devfs ruleset, PF rules, databases, images,
keys and port range isolated the run from the existing gateway jail and services.
Only this VNET's PF configuration was changed. Minimal guest fixtures used host
binaries and a raw UFS disk with the matching FreeBSD kernel. They are test
fixtures, not new image recipes.

To avoid a slow remote toolchain download, Rust 1.98.1 executables were cross-built
on Linux with Clang/LLD 18 and a FreeBSD 15.1 sysroot, then **executed on FreeBSD**.
The native runs used the debug profile. Archives and installed test binaries were
checked by SHA-256. Curl and its dependencies were extracted into the task
directory for CLI tests; no system packages were installed.

| Executable | SHA-256 |
| --- | --- |
| `tt-agent` | `f3f45e6e75e17266337d02466f75f5e2dd307c6794357792da4244c31fe779db` |
| `tt-ctl` | `b7c4310f8b4bb7c56a2ad6b43eab76ac5ad398098a7ea21a9c6704eb8e5f9448` |
| `tt` | `83fa776ae4901e4ba289935de1eae4be9348cc757a920feaa0578f58499e8d47` |

## Results

| Case | Observation |
| --- | --- |
| Linux workspace | 188 tests passed: CLI 21, agent 42, controller 51, core 74. Formatting, all-target Clippy, workspace check, Rust 1.88 check and locked release build passed. Linux ext4/configuration-drive cases ran with their prerequisites. |
| FreeBSD execution | 180 tests passed: CLI 21, agent 42, controller 51, core 66. This includes native database rejection, runtime/network bindings, per-VM reconciliation, disk-path validation and platform-specific regressions. FreeBSD-target workspace/all-target Clippy also passed. Linux-only cases were not counted as FreeBSD passes. |
| CLI prerequisite | The first CLI run could not complete without curl. After extracting the dependency privately, the complete 21-test CLI suite passed. |
| Jail SSH and lifecycle | Two Jails accepted injected root keys through distinct PF mappings in the configured port range. Stop/start retained a synced marker; restarting agent and controller retained mappings and access. No Linux SSH-bootstrap capability or managed SSH metadata was required. |
| Persistent port ownership | Restarting the agent with a different port range while Jail records remained was rejected. Restoring the saved configuration recovered both guests without recreating them. |
| Stopped recovery | After externally stopping a task-owned Jail and restarting the agent, reconciliation reported it stopped and cleared the obsolete network-recovery error. Starting the retained root restored SSH and the marker. |
| Deletion retry | Native Jail removal temporarily returned alias-removal/`dying` errors. The API retained deletion state and reservations for retry. Subsequent cleanup completed, and the surviving sibling's SSH/marker remained usable. Final inventory and reservations were empty. |
| Bhyve raw file | A 1-vCPU/512-MiB FreeBSD guest booted and accepted mapped SSH. Agent restart retained its live VMM PID and marker. Stop/start retained disk contents and ports; deletion removed the disk, VMM device, TAP and metadata. |
| Shutdown evidence | The captured bhyve console recorded filesystem synchronization and ACPI poweroff during stop. This establishes orderly shutdown of this fixture, not arbitrary application consistency. Jail used its fixture's shutdown script. |
| Native CLI | `env list` displayed the permanent environments' explicit non-expiring/deletion semantics. |

The minimal fixtures initially lacked normal service files, SSH helper binaries,
loader files and bhyve devmem visibility. Failed creations remained inspectable
and were explicitly deleted after correcting the fixtures. The bhyve fixture also
needed its normal root-filesystem remount step before writing test data. These
setup corrections changed no TTstack implementation. The maintained FreeBSD guide
now explicitly names `/dev/vmm.io/*` among bhyve's delegated devices.

## Cleanup and limits

Final agent inventory and CPU/memory/disk reservations were zero. Runtime clones,
VMM devices/processes, TAPs and guest PF anchors were absent. Test services, the
VNET jail, epair, private devfs ruleset, mounts, test-only kernel modules and remote
task files were removed. The original interface/module inventory and gateway
jail remained intact. Fresh direct SSH and international-jail commands worked;
the installed system package count was unchanged.

This run does not establish FreeBSD release-build/MSRV coverage, another FreeBSD
version, host-reboot recovery, capacity limits, untrusted-tenant isolation, or
fresh zvol/egress-policy coverage. Earlier
[zvol and egress evidence](freebsd-zvol-validation-2026-09-28.md) retains its own
revision and scope. Linux engines were checked locally here; the earlier Linux
live reports are not presented as new live tests of this rebased branch.
