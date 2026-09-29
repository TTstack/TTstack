# Unified technical capability tags

Status: implemented. The maintained behavior and wire contract are
in [technical capabilities](../capabilities.md). This document records the design
decisions and corrections to the original PR #13 proposal; it is not live
validation evidence or a replacement for the maintained guides.

## Problem

Request validation, placement, agent admission and offline resizing encoded
similar capability decisions independently. The agent advertised a flat list of
engine-specific compatibility strings. The dashboard had a fourth support matrix:
it disabled prepared Firecracker SSH keys while allowing Jail outgoing denial.
That drift obscured the difference between an unsupported operation, a missing
host prerequisite and a particular guest/storage instance that was ineligible.

## Corrected design

The implementation keeps three questions separate:

1. A pure registry in `ttcore` defines supported contracts for every engine/storage
   pair, including experimental FreeBSD. It does not inspect the controller's OS.
2. Existing agent probes produce an engine/storage-scoped, versioned report with
   enabled states, denial reasons and evidence. The legacy list remains available
   to older consumers.
3. The operation checks current state, image, capacity, backing dependencies and
   ownership before mutation. A static tag never replaces those checks.

One typed requirement derivation covers fresh creation; placement and direct
agent calls consume it. Offline resource updates share the same capability check.
The authenticated matrix endpoint, CLI and dashboard consume the registry rather
than maintaining independent engine lists for request options. Configuration-disk
accounting stays in one shared function, not in a tag's descriptive text.

### Corrections to the original matrix

- QEMU's internal cloud-init seed does not implement opaque caller `guest_config`.
- `deny_outgoing` does not imply network isolation. bhyve supports outgoing
  restrictions; it does not implement Linux guest isolation. Jail supports neither.
- Jail accepts root authorized keys without implementing the QEMU/Firecracker
  account/bootstrap/SSH-observation contract.
- FreeBSD remains experimental. bhyve backup support is stopped zvol only; file
  reflinks and Jail backups remain unsupported.
- A host-global canonical tag cannot grant a capability to every engine. New
  canonical statuses live exclusively in scoped reports, not in the legacy union.
- `firecracker_zvol` still gates compatible agent behavior. A static matrix cell
  cannot replace an older agent's implementation/version check.
- `disk_backup_v1` remains a wire-contract identifier for inspection, mutation,
  exact replay and recovery. Mechanism qualification is separate from the flag
  enabling new creation. Disabling the flag must not strand restore or cleanup.
- No interactive-console or other unimplemented capability is exposed merely for
  completeness. The vocabulary describes APIs the manager actually implements.

### Compatibility and failure handling

An absent report selects the explicit legacy mapping. A present report is
authoritative: unknown versions, missing/duplicate matching entries and storage
mismatches fail closed for new operations. Unknown tag strings remain available
for inspection and round trips but do not satisfy typed requirements.

The controller replaces whole reports during refresh and retains the last one
while a host is offline. New optional JSON fields need no SQLite schema bump.
No automatic one-release removal of legacy identifiers is scheduled.

Fresh capability admission never replaces durable operation intent. Existing
create requests retain their exact retry semantics; a pending resize retries its
recorded target even if a capability is subsequently withdrawn. Backup read,
restore, cleanup and exact replay keep their existing protocol and ownership
checks. Tags remain technical metadata, never authorization or application policy.

## Verification responsibilities

Regression coverage exercises engine/storage boundaries, implicit Firecracker
SSH configuration-drive requirements, legacy gates, cross-engine grant rejection,
authoritative denials, unknown/ambiguous reports, whole-report replacement,
offline retention and direct agent refusal before resource allocation. Lifecycle
regressions cover pending resource updates after capability withdrawal. Existing
backup state/retry and platform-specific tests continue to apply.

Run Linux and FreeBSD target checks independently: metadata for a mixed fleet
must remain available on both, while host implementations retain their OS gates.
Live acceptance belongs in a dated report covering the tested code, isolated
resources, actual guest readiness, retry/recovery outcomes and cleanup. A probe,
static matrix row or successful unit test alone is not live evidence.

Return to the [documentation index](../README.md).
