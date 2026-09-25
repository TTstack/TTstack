# Claude Workflows

[CLAUDE.md](../CLAUDE.md) loads the shared [repository instructions](../AGENTS.md).
These entry points support small, reviewable changes and lightweight functional
validation. They do not configure automatic hooks, pre-approve tools, or imply
permission to deploy.

## Skills

`x-` entry points are user-invoked only: they commit history, run requested checks,
write the audit registry, or touch a live host, so nothing loads them implicitly.

| Skill | Purpose |
| --- | --- |
| [/x-review](skills/x-review/SKILL.md) | Review a scope for concrete lifecycle, recovery, and operator defects; code read-only, registry excepted |
| [/x-check](skills/x-check/SKILL.md) | Select and run the checks appropriate to a change, and report their limits |
| [/x-commit](skills/x-commit/SKILL.md) | Validate and commit the requested change, preserving unrelated work |
| [/x-live](skills/x-live/SKILL.md) | Plan and run bounded functional tests on an authorized Linux host |

[ttstack-development](skills/ttstack-development/SKILL.md) is the one skill the
model may load on its own: it maps a change to its owning crate and to the shared
guides, and it changes nothing by itself.

## Shared guides

The skills cite these instead of restating them. They are policy for the workflows,
not product documentation.

| Guide | Contents |
| --- | --- |
| [workflow-policy.md](docs/workflow-policy.md) | Preflight, ownership, atomic units, validation failure, dispositions |
| [commit-protocol.md](docs/commit-protocol.md) | Per-unit and final checks, staging, commit message, version/schema rules |
| [review-core.md](docs/review-core.md) | Subsystem map, review depth, evidence standard, audit registry rules |
| [lifecycle-patterns.md](docs/lifecycle-patterns.md) | Lifecycle, placement, networking, state, and secret invariants |
| [false-positive-guide.md](docs/false-positive-guide.md) | What is deliberately not a defect here |
| [host-validation.md](docs/host-validation.md) | Authorization, budget, isolation, evidence, and cleanup for live hosts |
| [pragmatic-engineering.md](docs/pragmatic-engineering.md) | The engineering lens the skills apply |

Findings are registered in the [audit registry](../docs/audit.md). Supporting product
facts remain in [docs/](../docs/README.md). Paths in shell examples are relative to the
repository root.

The file layout follows the [Claude Code skills reference](https://code.claude.com/docs/en/skills).
The short `CLAUDE.md` import follows the [project memory reference](https://code.claude.com/docs/en/memory).
