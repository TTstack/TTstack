# Documentation

Reading paths for TTstack's maintained guides and its dated validation evidence.

## Maintained guides

Each behavior has one maintained description here. These describe current
behavior and are updated together with the code.

| Need | Document |
| --- | --- |
| Understand the project and start locally | [Repository overview](../README.md) |
| Install, deploy, or maintain a fleet | [Deployment](deployment.md) |
| Prepare guests and understand storage/network boundaries | [Guest images](guest-images.md) |
| Call the API or understand lifecycle recovery | [REST API](rest-api.md) |
| Create or restore an opt-in local disk recovery point | [VM disk backup](disk-backup.md) |
| Check supported platforms and validation limits | [Compatibility](compatibility.md) |
| Configure initial VM SSH and public ingress | [SSH](ssh.md) |
| Inspect the experimental FreeBSD restoration and its limits | [FreeBSD scope](compatibility.md#experimental-freebsd-restoration) |
| Configure distributed deployment | [Fleet template](../tools/deploy.toml.example) |
| Contribute or choose an AI workflow | [Repository instructions](../AGENTS.md) and [Claude workflows](../.claude/README.md) |

## Proposals

These retain design context; each document distinguishes implemented work from remaining proposals.
The [proposal review](proposals/audit.md) checks those claims against the code.
It is not the [defect registry](audit.md).

- [VM disk backup design](proposals/vm-disk-backup.md): rationale for implemented
  ZFS/reflink recovery points, safe replacement and asynchronous cleanup;
  qcow2 internal snapshots remain deferred. Current behavior is in the guide above.
- [VM SSH access and recovery evidence](proposals/vm-access-and-recovery.md):
  initial SSH implemented for QEMU/prepared Firecracker; host-side diagnostic
  additions remain proposed.
- [Unified capability tags](proposals/capability-tags.md): proposed vocabulary and
  design matrix for engine, storage and request support; nothing implemented.

## Validation evidence

Reports are newest first. Each describes only the code revision, hosts, tests and
limits it records, so an older report is not evidence for current behavior.
[Compatibility](compatibility.md) summarizes the support boundary those reports
establish. A new report belongs in `docs/validation/` and is linked here.

| Evidence | Document |
| --- | --- |
| Master synchronization, native FreeBSD regressions, bhyve backup/restore and Jail rejection | [FreeBSD master synchronization, 2026-09-29](validation/freebsd-master-sync-2026-09-29.md) |
| Opt-in ZFS/reflink disk backup, interrupted operations, exact replay and isolated cleanup on two hosts | [VM disk backup, 2026-09-29](validation/disk-backup-validation-2026-09-29.md) |
| Master rebase, native database/ownership regressions and FreeBSD Jail/bhyve lifecycle | [FreeBSD master port, 2026-09-28](validation/freebsd-master-port-2026-09-28.md) |
| 0.5.1 version-only deployment, retained state and observed caller-driven VM interruption | [0.5.1 upgrade, 2026-09-28](validation/version-0.5.1-upgrade-2026-09-28.md) |
| Native agent database initialization, reopen and non-mutating format rejection | [Native agent schema, 2026-09-28](validation/native-agent-schema-2026-09-28.md) |
| Configuration-disk accounting, resource ownership, restart recovery and cleanup on two hosts | [Lifecycle audit fixes, 2026-09-28](validation/audit-fixes-2026-09-28.md) |
| Native SSH/SCP, full guest sudo and cold restart on QEMU/Firecracker | [Expert SSH, 2026-09-28](validation/expert-ssh-2026-09-28.md) |
| FreeBSD 15.1 bhyve zvol lifecycle, deletion retries and engine-specific egress restrictions | [FreeBSD zvol and egress validation, 2026-09-28](validation/freebsd-zvol-validation-2026-09-28.md) |
| Native FreeBSD build, Jail/bhyve lifecycle, SSH and PF cleanup after PR #12 corrections | [FreeBSD restoration validation, 2026-09-28](validation/freebsd-validation-2026-09-28.md) |
| Container cleanup, unknown resize outcomes, heartbeat availability and scoped port forwarding on two hosts | [Lifecycle correction validation, 2026-09-28](validation/lifecycle-corrections-2026-09-28.md) |
| Implemented QEMU resize, partial-growth recovery and lifecycle on file/ZFS | [QEMU resize validation, 2026-09-26](validation/qemu-resize-validation-2026-09-26.md) |
| Native QEMU resize, Docker/Podman resource and network feasibility | [Engine capability probes, 2026-09-26](validation/engine-capability-probes-2026-09-26.md) |
| Offline stopped-VM resource updates on file and ZFS storage | [Offline resource update, 2026-09-25](validation/offline-resource-validation-2026-09-25.md) |
| Native caller stop/resume and zvol lifecycle | [Native caller lifecycle, 2026-09-25](validation/native-caller-validation-2026-09-25.md) |
| Audit correction regressions and recovery | [Audit correction, 2026-09-25](validation/audit-validation-2026-09-25.md) |
| Linux lifecycle on three hosts | [Linux live validation, 2026-09-24](validation/live-validation-2026-09-24.md) |
| Firecracker config drive, jailer, shutdown and isolation | [Firecracker configuration, shutdown and isolation, 2026-09-24](validation/firecracker-validation-2026-09-24.md) |
| Firecracker creation-time disk sizing and accounting | [Firecracker creation-time resource sizing, 2026-09-24](validation/resource-sizing-validation-2026-09-24.md) |
| Firecracker cold-start application readiness | [Firecracker cold-start entropy, 2026-09-24](validation/firecracker-entropy-validation-2026-09-24.md) |
| Firecracker restart on a dedicated ZFS file host | [Firecracker restart on a dedicated ZFS host, 2026-09-24](validation/firecracker-restart-validation-2026-09-24.md) |
| Firecracker zvol lifecycle and file fallback | [Firecracker zvol, 2026-09-24](validation/firecracker-zvol-validation-2026-09-24.md) |
| Linux engine upgrade and lifecycle regression | [Linux host upgrade, 2026-09-24](validation/linux-host-upgrade-validation-2026-09-24.md) |
| Historical SQLite engine comparison and dependency-refresh probes | [SQLite assessment, 2026-09-24](validation/sqlite-engine-assessment-2026-09-24.md) |

Guides describe maintained behavior. Reports describe only their recorded code,
hosts, tests, and limits. The [audit registry](audit.md) records a point-in-time
review and its dispositions, not a behavior guide. Keep those roles distinct and
link to the canonical guide instead of copying its defaults into workflow
instructions.
