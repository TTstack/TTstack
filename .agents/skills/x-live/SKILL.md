---
name: x-live
description: Plan and run bounded TTstack lifecycle, guest, networking, or restart checks on an authorized host, then record results and cleanup. Use when the user requests live functional validation; Linux x86_64 is supported and FreeBSD is experimental.
---

# Lightweight TTstack Live Validation

Functional acceptance on a real host. Not benchmarking, not production deployment,
not a security certification.

## Input

Use the behavior, defect, or environment supplied in the user's request. Empty → the
uncommitted task change; with no change either, ask instead of guessing a scope.

## Setup

Read [host validation](references/host-validation.md), the
[guest guide](../../../docs/guest-images.md), and the most relevant
[dated report](../../../docs/README.md#validation-evidence) for useful cases and the
limits of earlier evidence. Prior reports are not permission to reuse a host or
repeat every test. Report experimental FreeBSD combinations and verification limits
according to the [compatibility guide](../../../docs/compatibility.md).

## Protocol

1. **Bound the experiment** (`host-validation.md`): authorization, host prerequisites,
   the half-host budget, isolation, and the list of created resources.
2. **Verify the relevant behavior**: select cases from the actual change rather than
   running the whole list. Distinguish normal shutdown from forced termination, and
   guest or application readiness from a live VMM process.
3. **Close the loop**: clean up task-owned resources, confirm pre-existing services
   still hold their expected state, and write a dated report when evidence is retained.
4. A reproducible defect feeds a focused change; rerun only the affected checks.

## Output

Host and tested revision · cases observed and omitted · results including which
shutdown kind was seen · cleanup state · limits and untested scope · keys and private
configuration excluded.
