---
name: x-check
description: Select and run checks for a TTstack change, then report results and verification limits without editing or committing. Use when the user requests validation of a change or check scope.
---

# TTstack Change Checks

Select and run checks for the paths or scope in the user's request, or for the
current task's uncommitted changes when omitted. Validate only: no edits, no commit,
no deploy.

## Setup

Read [workflow policy](../ttstack-development/references/workflow-policy.md) for
ownership, [commit protocol](../ttstack-development/references/commit-protocol.md)
for change classes and checks, and [AGENTS.md](../../../AGENTS.md) for workspace rules.

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
