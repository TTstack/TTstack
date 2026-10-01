# Codex Workflows

[AGENTS.md](../AGENTS.md) defines the shared repository instructions. Repository
skills live in `.agents/skills/`, where Codex discovers them from the repository
root and its subdirectories. Each skill has a `SKILL.md` with `name` and
`description`, plus `agents/openai.yaml` for its UI metadata and invocation policy.
The layout and invocation rules follow the [official OpenAI skills guide](https://learn.chatgpt.com/docs/build-skills).

These workflows support focused changes and bounded functional validation. Using
a skill does not grant permission to commit, push, deploy, or touch a live host
beyond the current task's authorization.

## Skills

In Codex, invoke a skill with `$skill-name` and put any scope in the same request.
The `x-` workflows retain their explicit-only policy through
`policy.allow_implicit_invocation: false` in each `agents/openai.yaml`.

| Skill | Purpose |
| --- | --- |
| [$ttstack-development](../.agents/skills/ttstack-development/SKILL.md) | Implement and validate focused code or documentation changes |
| [$x-review](../.agents/skills/x-review/SKILL.md) | Review a scope for concrete lifecycle, recovery, and operator defects; code read-only, registry excepted |
| [$x-check](../.agents/skills/x-check/SKILL.md) | Select and run the checks appropriate to a change, and report their limits |
| [$x-commit](../.agents/skills/x-commit/SKILL.md) | Validate and commit the requested change, preserving unrelated work |
| [$x-live](../.agents/skills/x-live/SKILL.md) | Plan and run bounded functional tests on an authorized host, recording platform limits |

`ttstack-development` allows implicit invocation for TTstack code and documentation
tasks. The `x-` workflows can also be selected explicitly from the skill selector.

For example:

```text
$x-review staged
$x-check docs/workflows.md
$x-commit Commit the intended skill migration changes.
$x-live Validate stop/start persistence on the already-authorized test host.
```

If a newly added skill does not appear, restart Codex. No user-level skill
installation or configuration change is required for these repository skills.

## Shared guides

Each guide has one maintained copy under its owning skill's `references/`.
Other skills link to that copy when needed. These guides govern the workflows;
product behavior remains documented in the [documentation index](README.md).

| Guide | Contents |
| --- | --- |
| [workflow-policy.md](../.agents/skills/ttstack-development/references/workflow-policy.md) | Preflight, ownership, atomic units, validation failure, dispositions |
| [commit-protocol.md](../.agents/skills/ttstack-development/references/commit-protocol.md) | Per-unit and final checks, staging, commit message, version/schema rules |
| [review-core.md](../.agents/skills/x-review/references/review-core.md) | Subsystem map, review depth, evidence standard, audit registry rules |
| [lifecycle-patterns.md](../.agents/skills/ttstack-development/references/lifecycle-patterns.md) | Lifecycle, placement, networking, state, and secret invariants |
| [false-positive-guide.md](../.agents/skills/x-review/references/false-positive-guide.md) | What is deliberately not a defect here |
| [host-validation.md](../.agents/skills/x-live/references/host-validation.md) | Authorization, budget, isolation, evidence, and cleanup for live hosts |
| [pragmatic-engineering.md](../.agents/skills/x-review/references/pragmatic-engineering.md) | The engineering lens the skills apply |

Findings are registered in the [audit registry](audit.md). Paths in shell examples
and code-formatted repository paths are relative to the repository root; Markdown
links are relative to the file containing them.
