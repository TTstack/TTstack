---
name: x-check
description: Run the checks appropriate to a TTstack change and report their limits. Use only when the user explicitly invokes /x-check.
argument-hint: "[paths | check scope]"
disable-model-invocation: true
---

# TTstack Change Checks

Select and run the checks for `$ARGUMENTS`, or for the current task's uncommitted
changes when omitted. Validate only: no edits, no commit, no deploy.

## Setup

Read `.claude/docs/workflow-policy.md`, `.claude/docs/commit-protocol.md` (change
classes and the workspace gate), and `AGENTS.md` for the workspace rules.

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
