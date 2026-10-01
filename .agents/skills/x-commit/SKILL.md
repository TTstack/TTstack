---
name: x-commit
description: Review requested TTstack worktree changes, fix confirmed defects, validate, and create atomic local commits. Use when the user requests committing a defined set of changes.
---

# Self-Reviewing Commit for TTstack

Review owned worktree changes → fix confirmed defects → validate → local commits.
No deploy; no version bump or tag unless the user asks. New commits only
(no amend, rebase, or force-push).

## Input

Use optional Git pathspecs or a commit summary supplied in the user's request. Given
pathspecs → only matching changes are candidates; everything else is baseline.
Empty → all intended changes. Unknown flags → reject; never guess. Pathspecs matching
no change → nothing is intended.

## Setup

Read [workflow policy](../ttstack-development/references/workflow-policy.md) and
[commit protocol](../ttstack-development/references/commit-protocol.md) for preflight,
ownership, validation, and staging. Use [review core](../x-review/references/review-core.md)
and the [false-positive guide](../x-review/references/false-positive-guide.md) to assess
defects. For lifecycle, persistence, networking, engine, or deployment changes, read
[lifecycle patterns](../ttstack-development/references/lifecycle-patterns.md).
Complete preflight and record the ledger before the first edit.

## Protocol

### 1. Scope

1. `git status --short`, full diffs, intended untracked files (`git diff HEAD` misses them).
2. Nothing intended → "nothing to commit"; stop.
3. Freeze owned paths; later, stage only that set plus this invocation's fix and
   format paths.
4. Split coherent units: one issue, root cause, or behavior each. Tests, docs, and the
   registry entry stay with the unit.
5. Unrelated overlap in the same hunk → stop. No stash, revert, or absorb.

### 2. Review and fix

1. Map paths via the Subsystem Map; read the full changed functions, callers, error
   paths, and tests.
2. Check the mapped invariants in `lifecycle-patterns.md`; refute candidates through
   `false-positive-guide.md`.
3. Fix confirmed defects completely, with a regression test when it proves behavior
   or a concrete failure. Documentation-only fixes need link, example, and diff checks.
   An unaccepted public or persisted-contract break stays uncommitted and Open.
4. A real but disproportionate fix → Won't Fix with a reason recorded in
   `docs/audit.md`, never only in chat. No progress on a repeat pass → stop and report.

If delegation is available and authorized, investigation may run in parallel;
edits and commits on the shared worktree are sequential.

### 3. Validate and commit

`commit-protocol.md` per unit: format, focused test, exact staging, inspect the cached
diff, one new commit with an English Conventional Commit message.

### 4. Final gate

`commit-protocol.md` once per stable code state. Push only when the task or session
already authorizes it.

## Output

Files and subsystems · fixes and registry changes · validations · hashes and subjects ·
untouched baseline · what remains unverified.
