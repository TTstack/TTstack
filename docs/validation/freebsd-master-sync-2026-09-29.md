# FreeBSD master synchronization — 2026-09-29

The tested implementation is `c17413f2f8d8f3966f1439a8cb1af6f392e05e98`,
which merges master `ec64c45eabc744dc80cf1045f3fd94f691e11402` into PR #12.
The branch retains its existing version 0.6.0. Historical reports retain their
original revisions and scope.

## Scope and bounds

This update inherits master's interrupted-stop retries and disk-backup protocol,
including agent schema v6 and controller schema v5. It enables shared ZFS backup
operations for bhyve, reserves bhyve raw disk capacity during placement and agent
admission, rejects Jail backup requests, and reports FreeBSD file reflinks as
unsupported. The maintained contracts are in the [backup guide](../disk-backup.md)
and [FreeBSD scope](../compatibility.md#experimental-freebsd-restoration).

The authorized host ran FreeBSD 15.1-RELEASE-p3 on amd64, with four logical CPUs
and 12568940544 bytes of physical memory. A dedicated VNET jail, private devfs
ruleset, epair, ZFS dataset, service state and port ranges isolated the experiment
from the existing gateway jail. Test services and native tests were restricted to
two CPUs; tests used two threads and a 2-GiB virtual-memory limit. Services had
1-GiB virtual-memory limits. At most one 1-vCPU/512-MiB bhyve guest or one Jail
with a 128-MiB scheduling reservation ran at a time. Observed free memory remained
above 9 GiB. No stress test or service replacement was performed.

Rust 1.98.1 binaries were cross-built on Linux with Clang/LLD 18 and a FreeBSD
15.1 sysroot, then executed on FreeBSD. The native runs used the debug profile.
The transfer archive was verified by SHA-256. Curl and its dependencies were
fetched into a private package directory and extracted for CLI tests; no packages
were installed. Guest fixtures used host binaries and a 384-MiB raw UFS disk.

| Native executable | SHA-256 |
| --- | --- |
| `tt` | `a7d2e9ec67bf7359bb8189a0be5407ce8b0f1d10a9ed9aa26f3c2e9ff6cbdfc5` |
| `tt-agent` | `bcab4728a85f6e847bcf1b02de1a02de87d900a887443ff6dbcaf88fbd3b505b` |
| `tt-ctl` | `1b0eb4fba8614985e4667697d48d11407910c3643c7740fa727c91f063bfbdec` |

## Results

| Case | Observation |
| --- | --- |
| Linux workspace | 208 tests passed: CLI 21, agent 51, controller 55, core 81. Formatting, all-target Clippy, workspace check, Rust 1.88 check and locked release build passed. Configuration-drive tests ran with e2fsprogs available. |
| FreeBSD native suites | 201 tests passed: CLI 21, agent 51, controller 55, core 74. FreeBSD-target all-target Clippy passed. The suites cover strict raw disk sizing, background catalog reads, backup eligibility, writer-lock inheritance, interrupted-stop retries and native engine regressions. |
| Capacity and eligibility | The bhyve fixture reserved 384 MiB at placement and creation. Running-VM backup creation returned 409. File storage advertised the backup protocol without advertising a qualified reflink backend. |
| Interrupted bhyve stop | A task-only command wrapper failed the first VMM device destruction after process exit. Reconciliation observed a stopped VM while retaining `stop unfinished:`. Repeating stop completed cleanup and cleared that error. |
| Zvol backup and exact replay | A stopped guest produced a current ZFS recovery point. Replaying the same operation ID and revision retained the same generation. |
| Service restart and restore | After a synced guest marker changed, stopping the guest and restarting the isolated agent/controller retained the generation. Restore and its exact replay left the VM stopped and retained the backup. Cold start and mapped SSH read the original marker. |
| Refresh and cleanup retry | A hold on the task's old snapshot did not prevent publishing a refreshed generation. Explicit removal failed while the hold remained. After releasing the hold, the exact same request removed the backup and retired records. |
| Destructive VM deletion | A newly created current backup was removed through environment deletion. Runtime zvols, guest runtime artifacts and resource reservations were released. |
| Jail eligibility and lifecycle | A Jail accepted an injected root public key through its assigned PF port. Controller and direct agent backup creation both returned 400 with an explicit Jail reason and unchanged backup state. Stop/start retained a synced marker; deletion released its root and reservations. |
| Shutdown evidence | Final bhyve lifecycle runs exited within the ACPI grace period; guest console output showed filesystem synchronization. An earlier incomplete boot fixture required forced termination during setup cleanup. Jail used its fixture shutdown script. |

The initial isolated setup needed a CA bundle, complete loader files, an explicit
UFS root-device setting and devfs rules exposing each task-owned zvol path level.
Failed guest creations remained inspectable and were deleted before retrying with
corrected fixtures. Those setup failures are not counted as successful lifecycle
checks. Missing curl initially prevented completion of the CLI suite; all 21 tests
passed after the private prerequisite was supplied.

## Platform build follow-up

Commit `93039e8` subsequently tightens host-code compilation without changing the
bhyve/Jail implementations exercised above. Docker execution and nftables SSH
ingress now compile only on Linux; FreeBSD exports its native bhyve/Jail engines.
The shared engine names and container-runtime metadata remain available to both
controller targets. An engine-factory regression rejects the opposite platform's
engines without probing tools or touching host resources. FreeBSD ingress removal
also rejects a retained Linux namespace configuration.

Cargo metadata reports no package feature switches in the four workspace crates;
OS selection uses `cfg(target_os)` and the target-specific `nix` dependency.
Linux passed 209 tests, workspace checking with no default features, all-target
Clippy with all features, Rust 1.88 all-target checking and a locked release build.
FreeBSD-target all-target/all-feature Clippy passed. The affected native suites
were rerun on the same authorized host: core 73, agent 50 and controller 55 tests
passed. Linux-only Docker and nftables tests are intentionally absent from these
FreeBSD counts; the new factory test runs on both targets. The earlier CLI result
and live guest results above remain evidence for `c17413f`, not additional live
coverage of this follow-up. Follow-up binaries and logs were removed from the host.

## Cleanup and limitations

Both test agents ended with empty inventories and zero CPU, memory and disk
reservations. Task-owned guests, snapshots, datasets, runtime artifacts, services,
VNET jail, epair, devfs rules, mounts, test-only kernel modules and remote task
files were removed. The original interface/module inventory and gateway jail
were preserved. Fresh direct SSH and international-jail commands remained usable.

This run covers only FreeBSD 15.1-RELEASE-p3 amd64. It does not establish native
FreeBSD release-build or MSRV coverage, host-reboot/power-loss durability, another
FreeBSD release, untrusted-tenant isolation or capacity limits. FreeBSD file
reflinks and Jail backups remain unsupported. Linux engines were checked locally;
existing Linux live reports are not new live validation of this merge.
