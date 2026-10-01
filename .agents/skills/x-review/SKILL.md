---
name: x-review
description: Review a TTstack diff, commit range, or repository scope for concrete lifecycle, recovery, and operator defects, and update docs/audit.md without code repairs. Use for review-only requests or as the review phase of x-overhaul.
---

# TTstack Lifecycle Review

High-signal review. Code read-only; `docs/audit.md` is the registry exception.
A standalone review ends with findings and registry changes; it does not automatically
commit, push, or deploy. Review alone does not authorize code repairs or live host tests.
Use [$x-overhaul](../x-overhaul/SKILL.md) for review, repair, validation, and local
commits, or [$x-commit](../x-commit/SKILL.md) to commit defined worktree changes.

When reused by `x-overhaul`, keep this review phase code read-only and return its
scope, baseline, coverage, findings, and registry changes to the caller. The caller
owns subsequent repairs and commits; do not start a nested overhaul from this skill.

## Setup

Read [workflow policy](../ttstack-development/references/workflow-policy.md) for scope
and ownership, [pragmatic engineering](references/pragmatic-engineering.md), and
[review core](references/review-core.md) for the subsystem map and evidence standard.
Refute candidates with the [false-positive guide](references/false-positive-guide.md).
For lifecycle, persistence, networking, engine, or deployment changes, also read
[lifecycle patterns](../ttstack-development/references/lifecycle-patterns.md).
For public API, persisted state, or a documented default in scope, also read the
[REST API](../../../docs/rest-api.md) and [compatibility guide](../../../docs/compatibility.md).

## Input

Use at most one review scope supplied in the user's request:

| Input | Scope | Evidence |
|-------|-------|----------|
| *(empty)* | Uncommitted task changes | `git diff HEAD` + untracked |
| `N` | Last N commits | `git log HEAD~N..HEAD` + `git diff HEAD~N HEAD` |
| `staged` | Index | `git diff --cached` |
| `worktree` | Staged + unstaged + untracked | `git diff HEAD` + `git ls-files --others --exclude-standard` |
| `all` | Full repository | `git ls-files` ledger |
| `<rev>` | One commit | `git show <rev>`; merge → `git diff <rev>^1 <rev>` |
| `<a>..<b>` | Range | `git log <a>..<b>` + `git diff <a>...<b>` |

Resolve every rev (including `HEAD~N`) with
`git rev-parse --verify --quiet '<rev>^{commit}'`. An all-digit token is `N`; if it
also resolves as a commit, ask. Reject anything else; never guess. Scope selects what
to review — callers, tests, and guides are evidence, not permission to widen the report.
Historical scope: report only defects still present at `HEAD`.

## Protocol

### 1. Scope

1. Worktree baseline (`workflow-policy.md` §1).
2. Changed files, full diff, callers, tests. `worktree` includes untracked files.
3. Map via the Subsystem Map; load the named guides.
4. Mark generated, vendored, and out-of-scope rows in the ledger. Do not silent-drop them.

### 2. Evidence

Review a small single-subsystem scope directly. For `all`, use disjoint batches with
one owner per file. If delegation is available and authorized, keep reviewers
read-only and split only when it helps manage context. fmt/compile/clippy are tools,
not findings.

Cover what the diff touches: lifecycle and partial failure, persistence and
compatibility, networking, engines, placement, authorization, operator experience.
Record reviewed paths and invariants, including coverage gaps; a search or a passing
test alone does not establish depth.

### 3. Verify

Re-read each candidate and try to **refute** it (`false-positive-guide.md`). Keep only
what the code demonstrates; merge findings sharing a root cause. If authorized
delegation helps resolve an ambiguous candidate, use one independent verifier.
Agreement is not proof.

### 4. Completeness

Diff: every changed file, public interface, persisted contract, failure path, and
related test. `all`: ledger versus depth results; report coverage gaps instead of
papering over them.

### 5. Registry

Update `docs/audit.md` per `review-core.md` §5.

### 6. Report

Scope, coverage, findings (severity, location, trigger, outcome, compatibility, fix),
and what was left unfixed. Zero findings → say so and state what was covered.
