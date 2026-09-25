# Workflow Safety and Atomic Commit Policy

SSOT safety for `/x-review`, `/x-check`, `/x-commit`, `/x-live`. Skills must not
weaken it. See also `pragmatic-engineering.md`.

**Hard rules:** mutation work is user-invoked · local commits only · no history
rewrite · one independent issue per commit · no version bump or tag unless the user
asks · `make deploy*` is never a check.

## 1. Preflight

Before mutate/commit:

1. Record `git status --short`, branch, `HEAD`.
2. Separate staged / unstaged / untracked baseline.
3. Stop on merge/rebase/cherry-pick or detached HEAD unless the user resolves it.
4. Define this invocation's owned files/hunks; baseline stays with its author.
5. **Commit workflows:** freeze owned paths (+ planned units) before review edits.
   Stage only the freeze set + this invocation's fix/format paths — never paths that
   appeared later from concurrent work.

Dirty tree OK; clear ownership required.

## 2. Preserve existing work

- No `stash` / `clean` / `checkout --` / `restore` / destructive `reset` to fake a clean tree.
- Never touch unrelated baseline (revert, overwrite, stage, commit).
- `docs/audit.md` is the registry exception: any skill may update it, including a
  read-only review. That is not a code write. Merge; do not revert unrelated edits there.
- Live systems: never reset host networking, stop another guest or service, or undo
  work owned by another session. Track and clean up only task-created resources.
- If a needed fix overlaps baseline and cannot be separated safely → stop and report.
- Review agents read-only. Parallelism: investigation only. Edits and commits on one
  tree: sequential.

## 3. Atomic commit units

One issue / root cause / behavior change → one commit.

- Bundle only its tests, docs, and registry update.
- Multiple symptoms only if same root cause.
- No drive-by cleanup, format churn, or refactors.
- Stage exact paths/hunks (`git add -A` forbidden). Inspect `git diff --cached` before every commit.
- New commits only — no amend, rebase, or force-push.

## 4. Validation and failure

- Smallest relevant checks per unit; workspace gate once after the last behavior change.
- Change classes and commands: `commit-protocol.md`.
- Distinguish unit-caused failures from pre-existing ones; report the latter with evidence.
- Same failure repeats with no progress → stop and report.
- A timeout, a skipped external-tool test, or an unverified backend is not a pass.
  State what was actually verified.

## 5. Audit dispositions

| state | meaning |
|-------|---------|
| Open | confirmed, actionable |
| Won't Fix | real; safe fix currently disproportionate |

Disproven entries are removed, not retained in a Rejected section. Record the
refutation in the review output or commit history; routine noise needs no entry.

Registry rules and entry shape: `review-core.md` §5.
