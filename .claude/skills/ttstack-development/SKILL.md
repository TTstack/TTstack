---
name: ttstack-development
description: Implement and validate focused TTstack changes using the owning Rust crate, lifecycle contracts, and relevant checks. Use for TTstack code or documentation work; live host testing has a separate workflow.
---

# TTstack Development

Read [AGENTS.md](../../../AGENTS.md) and inspect the task's diff before choosing
checks. Package names differ from directory names: core is `ttcore`, CLI is
`tt`, agent is `tt-agent`, and controller is `tt-ctl`.

## Follow the owning boundary

- API/lifecycle: [REST API](../../../docs/rest-api.md), shared models, handlers,
  scheduler, and persisted state. Trace both controller and agent when ownership
  crosses the HTTP boundary; do not treat retries as fresh requests.
- Engines/storage/networking: [guest guide](../../../docs/guest-images.md), the
  affected engine, and cleanup paths. Check stop/start preservation separately
  from deletion and release resources only after the relevant outcome is known.
- CLI/deployment: [deployment guide](../../../docs/deployment.md), CLI help,
  examples, and image/deploy code. Keep application identity and business policy
  outside the generic manager.
- Documentation: repair incoming and outgoing links and distinguish implemented
  behavior from dated verification. Do not claim fresh live coverage from an old
  report or run a build solely because a Markdown file changed.

## Select useful checks

Run a focused regression first, for example `cargo test --locked -p ttcore
<test-filter>`. Add a test when it demonstrates the changed behavior or a concrete
failure, not merely to mirror the implementation.

For Rust changes, finish with the applicable workspace checks from CI:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
```

Dependency, edition, or newer standard-library usage changes also need the
declared minimum compiler check:

```sh
cargo +1.88.0 check --workspace --locked
```

Use a release build when validating packaging, deployment artifacts, or a change
whose behavior depends on release compilation. Configuration-drive tests need
the tools from `e2fsprogs`; a skipped external-tool test is not a pass. Explain
unavailable prerequisites and what was actually verified. Check all targets when
the modified surface requires it; avoid repeating passing checks without a reason.

Inspect the final diff and run `git diff --check`. Report behavior, evidence, and
remaining limits. Do not run `make deploy*` as a development check. Remote tests
use the separate [live-validation skill](../ttstack-live-validation/SKILL.md)
and the session's existing host authorization.
