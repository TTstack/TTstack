---
name: ttstack-development
description: Implement and validate focused TTstack changes using the owning Rust crate, lifecycle contracts, and relevant checks. Use for TTstack code or documentation work; live host testing has a separate workflow.
---

# TTstack Development

Read [AGENTS.md](../../../AGENTS.md) and inspect the task's diff before choosing
checks. Package names differ from directory names: core is `ttcore`, CLI is `tt`,
agent is `tt-agent`, and controller is `tt-ctl`.

## Follow the owning boundary

- API/lifecycle: [REST API](../../../docs/rest-api.md), shared models, handlers,
  scheduler, and persisted state. Trace both controller and agent when ownership
  crosses the HTTP boundary; a retry is not a fresh request.
- Engines/storage/networking: [guest guide](../../../docs/guest-images.md), the
  affected engine, and cleanup paths. Stop and delete preserve different things.
- CLI/deployment: [deployment guide](../../../docs/deployment.md), CLI help, examples,
  and image/deploy code. Keep application identity and business policy outside the
  generic manager.
- Documentation: repair incoming and outgoing links and distinguish implemented
  behavior from dated verification. Do not claim fresh live coverage from an old
  report, and do not run a Rust build solely because a Markdown file changed.

## Apply the shared guides

- [lifecycle-patterns.md](../../docs/lifecycle-patterns.md) — invariants the change
  must preserve.
- [commit-protocol.md](../../docs/commit-protocol.md) — checks by change class and the
  final workspace gate.
- [workflow-policy.md](../../docs/workflow-policy.md) — ownership, atomic units, and
  preservation of unrelated work.

## Select useful checks

Run a focused regression first, for example
`cargo test --locked -p ttcore <test-filter>`. Add a test when it demonstrates the
changed behavior or a concrete failure, not merely to mirror the implementation.
Configuration-drive tests need the tools from `e2fsprogs`; a skipped external-tool test
is not a pass.

Finish with the workspace checks in
[commit-protocol.md](../../docs/commit-protocol.md). Dependency, edition, or
newer-standard-library usage also needs the declared minimum compiler. Explain
unavailable prerequisites and state what was actually verified.

Inspect the final diff and run `git diff --check`. Report behavior, evidence, and
remaining limits. Do not run `make deploy*` as a development check; remote tests use
the separate `/x-live` skill with the session's existing host authorization.
