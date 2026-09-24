# Documentation

This index is the reading path for TTstack's maintained guides and dated evidence.
The existing guide paths remain stable; this repository does not need an empty
directory for every document category.

| Need | Document |
| --- | --- |
| Understand the project and start locally | [Repository overview](../README.md) |
| Install, deploy, or maintain a fleet | [Deployment](deployment.md) |
| Prepare guests and understand storage/network boundaries | [Guest images](guest-images.md) |
| Call the API or understand lifecycle recovery | [REST API](rest-api.md) |
| Check supported platforms and validation limits | [Compatibility](compatibility.md) |
| Inspect Linux lifecycle evidence | [Linux validation, 2026-09-24](live-validation-2026-09-24.md) |
| Inspect Firecracker configuration, isolation, and jailer evidence | [Firecracker validation, 2026-09-24](firecracker-validation-2026-09-24.md) |
| Inspect Firecracker disk sizing and resource accounting | [Resource sizing validation, 2026-09-24](resource-sizing-validation-2026-09-24.md) |
| Inspect Linux engine upgrade and lifecycle regression | [Linux host upgrade validation, 2026-09-24](linux-host-upgrade-validation-2026-09-24.md) |
| Inspect Firecracker application cold-start entropy | [Cold-start validation, 2026-09-24](firecracker-entropy-validation-2026-09-24.md) |
| Inspect dedicated ZFS host and Firecracker restart evidence | [Restart validation, 2026-09-24](firecracker-restart-validation-2026-09-24.md) |
| Inspect Firecracker zvol lifecycle and file fallback | [Zvol validation, 2026-09-24](firecracker-zvol-validation-2026-09-24.md) |
| Review design and implementation gaps at one revision | [Audit, 2026-09-25](../audit.md) |
| Configure distributed deployment | [Fleet template](../tools/deploy.toml.example) |
| Contribute or choose an AI workflow | [Repository instructions](../AGENTS.md) and [Claude workflows](../.claude/README.md) |

Guides describe maintained behavior. Reports describe only their recorded code,
hosts, tests, and limits. The audit is a point-in-time review of one revision,
not a behavior guide. Keep those roles distinct and link to the canonical
guide instead of copying its defaults into workflow instructions.
