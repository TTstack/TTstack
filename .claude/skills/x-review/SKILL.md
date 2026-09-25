---
name: x-review
description: Review a TTstack scope for concrete lifecycle, recovery, and operator defects, and update the docs/audit.md registry. Use only when the user explicitly invokes /x-review.
argument-hint: "[N | all | staged | worktree | <rev> | <rev1>..<rev2>]"
disable-model-invocation: true
---

# TTstack Lifecycle Review

High-signal review. Code read-only; `docs/audit.md` is the registry exception.
Never commit, push, or deploy. Fixes belong to `/x-commit`. Review alone does not
authorize fixes, deployment, or live host tests.

## Setup

Read `.claude/docs/workflow-policy.md`, `.claude/docs/pragmatic-engineering.md`,
`.claude/docs/review-core.md` (Subsystem Map), `.claude/docs/lifecycle-patterns.md`,
`.claude/docs/false-positive-guide.md`. Public API, persisted state, or a documented
default in scope → also `docs/rest-api.md`, `docs/compatibility.md`.

## Input

`$ARGUMENTS` — at most one scope:

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

Small single-subsystem → review directly. Read-only agents only when a context split
helps; `all` → disjoint batches, one owner per file. fmt/compile/clippy are tools, not
findings.

Cover what the diff touches: lifecycle and partial failure, persistence and
compatibility, networking, engines, placement, authorization, operator experience.
Record reviewed paths and invariants, including coverage gaps; a search or a passing
test alone does not establish depth.

### 3. Verify

Re-read each candidate and try to **refute** it (`false-positive-guide.md`). Keep only
what the code demonstrates; merge findings sharing a root cause. One independent
verifier only if a candidate stays ambiguous. Voting is not proof.

### 4. Completeness

Diff: every changed file, public interface, persisted contract, failure path, and
related test. `all`: ledger versus depth results; report coverage gaps instead of
papering over them.

### 5. Registry

Update `docs/audit.md` per `review-core.md` §5.

### 6. Report

Scope, coverage, findings (severity, location, trigger, outcome, compatibility, fix),
and what was left unfixed. Zero findings → say so and state what was covered.
