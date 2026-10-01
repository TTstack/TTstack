---
name: x-overhaul
description: Review a TTstack scope, fix confirmed in-scope defects, validate, and create atomic local commits. Use when the user requests this review-and-fix workflow; no scope selects the full repository.
---

# TTstack Review, Repair, and Commit

Review the scope, resolve confirmed defects safely, validate, and create local
commits. An explicit invocation authorizes these in-scope repairs and commits.
Push only when the task or session already authorizes it. No deployment, live host
tests, version bump, or tag without authorization covering that action.

## Input and ownership

Empty input selects `all`: the full repository. Explicit input uses the scopes and
revision checks in [$x-review](../x-review/SKILL.md#input): `all`, `N`, `staged`,
`worktree`, a revision, or a range. Accept at most one scope; reject `--fix` and
unknown flags. Pass the resolved scope to the review phase explicitly because
standalone `x-review` defaults to uncommitted task changes.

For a diff scope, fix only still-present defects rooted in that diff. Callers,
tests, and contracts are evidence, not permission for unrelated repairs. Leave
unrelated Open entries and pre-existing work intact. `all` accounts for every
tracked file; do not silently absorb untracked baseline files into commits.

`staged` and `worktree` select changes the user intends this invocation to commit.
Freeze their paths and hunks before edits using [$x-commit](../x-commit/SKILL.md#1-scope).
For `staged`, unstaged and untracked work stays baseline. Isolate any repair that
overlaps unrelated work; if it cannot be separated safely, report the blocker and
continue independent units without staging the overlap.

## Setup

Read [workflow policy](../ttstack-development/references/workflow-policy.md),
[commit protocol](../ttstack-development/references/commit-protocol.md), and
[$x-review](../x-review/SKILL.md) with its scope-relevant Setup guides. Complete
preflight and record the starting HEAD, branch, baseline, scope, and owned paths
before the first edit. Freeze each newly discovered repair unit before changing it.

This skill owns one invocation ledger, per-unit commits, and one final workspace
gate. Reuse the review and commit protocols below without starting nested standalone
workflows or repeating their finalization. The review phase remains code read-only;
this skill owns the subsequent repairs and publication within the authorized scope.

## 1. Review

Apply `x-review` Protocol steps 1–5 once: scope and coverage ledger, evidence,
adversarial verification, completeness, and the audit registry. A full audit reads
every maintained source file in depth and accounts for configuration, tests, and
documentation, explicitly marking generated or excluded files and coverage gaps.
Passing checks and keyword searches alone do not establish review coverage.

Keep the resolved scope and baseline for the repair phase. Reuse a review already
completed in this invocation; recheck only evidence invalidated by later changes.
Reassess existing in-scope Open and Won't Fix entries under
[review core](../x-review/references/review-core.md#5-audit-registry).

## 2. Repair and commit

Resolve confirmed in-scope findings in severity order, one root cause per unit.
For `staged` or `worktree`, fix defects within their intended units before committing
those units. Apply [$x-commit](../x-commit/SKILL.md#2-review-and-fix) review and repair
steps to each unit, including callers, cleanup paths, partial failure, retries, and
public or persisted contracts. Add a regression test when it proves the changed
behavior or a concrete failure; documentation repairs need link and example checks.

Use the per-unit checks and exact staging in `commit-protocol.md`. Commit each
validated unit with its tests, guide updates, and registry disposition in an English
Conventional Commit. Re-review the invocation's complete diff for interactions;
new defects introduced by a repair need a new atomic correction and affected checks.

Remove a fixed registry entry only when validation supports the fix. A disproven
entry is removed with its refutation reported. A real defect whose safe repair is
disproportionate stays Won't Fix with a reason. Blocked or unverified repairs and
unaccepted contract breaks remain Open with the blocker recorded; do not relabel
them to clear the registry. Continue independent safe units and report remaining
Open entries and coverage gaps. Commit any remaining registry-only changes as a
separate documentation unit; do not create an empty commit.

## 3. Final gate and delivery

Run `commit-protocol.md`'s final workspace gate once per stable code state. A full
audit runs the gate even if it found no Rust repair; a narrower documentation-only
scope needs documentation checks. Reuse passed checks on unchanged code. A later
regression needs a new atomic fix and the affected checks again.

Inspect all invocation commits and remaining diffs against the starting baseline,
including intended untracked files. Preserve unrelated work and validated local
commits if validation or access is blocked. Report partial completion honestly;
neither an empty working-tree diff nor zero Open entries proves full coverage.

If pushing is already authorized, verify the target and push only the intended
validated commits, without force-push or unrelated commits. Otherwise finish with
local commits. Deployment and live validation use the separate
[$x-live](../x-live/SKILL.md) workflow and require an authorized host.

## Output

Scope and reviewed baseline, coverage and gaps, findings by severity, fixed /
Won't Fix / remaining Open dispositions, checks and limitations, compatibility or
migration impact, commit hashes and subjects, publication result when authorized,
and preserved baseline work.
