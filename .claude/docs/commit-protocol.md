# Atomic Commit Protocol

Validate → commit for `/x-commit` and for each unit any workflow fixes. Use with
`workflow-policy.md`. Check scope and support limits: `docs/compatibility.md`.

## Invocation ledger

Before the first edit, record:

- start `HEAD`, branch, and the three-way baseline (staged / unstaged / untracked);
- **frozen owned paths** (sorted) and the planned units;
- whether any unit changes a public interface, persisted state, or a documented
  contract the user has not accepted (`docs/rest-api.md`, `docs/compatibility.md`).

Keep the ledger across commits. Stage only the freeze set + this-invocation fix/format paths.

## Per-unit validate and commit

1. One issue/root cause/behavior change + its tests, docs, and registry entry only.
2. Checks by change class:
   - **Docs/config only:** `git diff --check` + link and structure sanity. Skip Rust gates.
   - **Focused Rust:** `cargo fmt --all -- --check`, then the smallest proving test —
     `cargo test --locked -p <pkg> <filter>` with `<pkg>` in
     `ttcore | tt-agent | tt-ctl | tt`. Reuse results on unchanged code.
   - **Shared contract / cross-crate:** package suites or `cargo test --workspace --locked`.
   - **Dependency, edition, or newer-std usage:** also
     `cargo +1.88.0 check --workspace --locked`.
   - Guest configuration-drive tests need `e2fsprogs`; a skipped external-tool test is
     not a pass. Say what was unavailable.
3. On failure: fix if unit-caused; otherwise report pre-existing with evidence.
4. Stage exact freeze + unit paths — never `git add -A`.
5. Inspect `git diff --cached --check` and `git diff --cached`: exactly one unit, no
   baseline or post-freeze paths, no whitespace errors.
6. English Conventional Commit message in the repository's voice (`fix(net): …`,
   `docs(claude): …`) using the configured identity; never invent an author, never
   amend a prior commit. Push only when the task or session already authorizes it,
   and never force-push.
7. Verify the commit; compare `git status --short` to the baseline.

## Final workspace gate

Once per stable code state, after the last behavior commit. CI runs exactly these
([.github/workflows/ci.yml](../../.github/workflows/ci.yml)); run the rows the change
can affect and reuse checks already passed on the same state.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked                  # needs e2fsprogs for config-drive tests
cargo check --workspace --locked
cargo +1.88.0 check --workspace --locked         # dependency/edition/MSRV touched
cargo build --release --workspace --locked       # packaging or release-dependent behavior
```

Docs-only: skip the Rust gates unless the caller is a full audit. Regression after a
commit → new atomic fix, then re-run the affected checks.

## Version, schema, and deployment

- The version lives in `[workspace.package] version` of the root
  [Cargo.toml](../../Cargo.toml); every crate inherits it. There is no changelog and
  no release convention — do not bump or tag unless the user asks.
- The real compatibility gates are `SCHEMA_VERSION`
  (`crates/ctl/src/db.rs`, `crates/agent/src/runtime.rs`) and the host capability
  strings advertised in `crates/agent/src/runtime.rs` and consumed by
  `crates/ctl/src/scheduler.rs`. Controller and agent upgrade together: an older
  binary rejects a newer schema, and unknown stored engine values are rejected rather
  than remapped (`docs/deployment.md`). A change to any of these carries its
  migration and guide update in the same unit.
- `make deploy*` requires root and changes live services; it is never a development
  check. Deployment behavior is documented in `docs/deployment.md`.
