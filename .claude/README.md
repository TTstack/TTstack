# Claude Workflows

[CLAUDE.md](../CLAUDE.md) loads the shared [repository instructions](../AGENTS.md).
These entry points support small, reviewable changes and lightweight functional
validation. They do not configure automatic hooks, pre-approve tools, or imply
permission to deploy.

## Commands

| Entry point | Purpose |
| --- | --- |
| [/tt-review](commands/tt-review.md) | Review the selected diff for concrete lifecycle and usability defects; no edits by default |
| [/tt-check](commands/tt-check.md) | Select and run appropriate local checks for the change |
| [/tt-commit](commands/tt-commit.md) | Validate and commit the requested change, preserving unrelated work |

## Reusable skills

| Skill | When to use |
| --- | --- |
| [ttstack-development](skills/ttstack-development/SKILL.md) | Implement or validate a Rust, API, CLI, engine, or documentation change |
| [ttstack-live-validation](skills/ttstack-live-validation/SKILL.md) | Plan and run bounded functional tests on an authorized Linux host |

Skill bodies load when relevant and can also be invoked by their slash names.
Commands provide explicit task entry points; there are no duplicate command and
skill names. Supporting product facts remain in [docs/](../docs/README.md).
Paths in shell examples are relative to the repository root.

The file layout follows the [Claude Code skills reference](https://code.claude.com/docs/en/skills).
The short `CLAUDE.md` import follows the [project memory reference](https://code.claude.com/docs/en/memory).
