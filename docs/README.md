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
| Check supported platforms and validation limits | [Compatibility](compatibility.md) |
| Configure distributed deployment | [Fleet template](../tools/deploy.toml.example) |
| Contribute or choose an AI workflow | [Repository instructions](../AGENTS.md) and [Claude workflows](../.claude/README.md) |

## Validation evidence

Reports are newest first. Each describes only the code revision, hosts, tests and
limits it records, so an older report is not evidence for current behavior.
[Compatibility](compatibility.md) summarizes the support boundary those reports
establish. A new report belongs in `docs/validation/` and is linked here.

| Evidence | Document |
| --- | --- |
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

Guides describe maintained behavior. Reports describe only their recorded code,
hosts, tests, and limits. The [audit registry](audit.md) records a point-in-time
review and its dispositions, not a behavior guide. Keep those roles distinct and
link to the canonical guide instead of copying its defaults into workflow
instructions.
