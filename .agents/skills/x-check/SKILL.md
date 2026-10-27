---
name: x-check
description: Select and run checks for a TTstack change, then report results and verification limits without editing or committing. Invoke explicitly as $x-check.
---

# TTstack Change Checks

Resolve repository paths from the Git root; Markdown links are relative to
this file. Read invocation arguments from the user prompt.

Select and run checks for the paths or scope in the user's request, or for the
current task's uncommitted changes when omitted. Validate only: no edits, no commit,
no deploy.

## Preflight

Run `git status --short --branch` and `git rev-parse HEAD` explicitly before
checks. Record the staged, unstaged and untracked baseline, branch and starting
HEAD for `workflow-policy.md` §1.

## Setup

Read first:
[workflow-policy.md](../../../docs/ai/workflow-policy.md) (ownership),
[commit-protocol.md](../../../docs/ai/commit-protocol.md) (change classes and
checks), and [AGENTS.md](../../../AGENTS.md) (workspace rules).

## Protocol

1. Classify the change: documentation/config, focused Rust, shared contract or
   cross-crate, dependency/edition/MSRV, packaging or release-dependent behavior.
2. Run the matching checks from `commit-protocol.md`. Reuse checks already passed on
   the same code state; do not repeat a gate without a reason.
3. Inspect every result. Distinguish unit-caused failures from pre-existing ones and
   report the latter with evidence.
4. Stop at the check. Do not fix, refactor, deploy, start guests, or rewrite
   dependencies beyond the user's authorized scope.

## Output

Checks run and their results · checks skipped or unavailable (`e2fsprogs`, engine or
host prerequisites) · pre-existing failures with evidence · what remains unverified.
