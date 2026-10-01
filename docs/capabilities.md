# Technical capabilities

TTstack distinguishes design support, observed host prerequisites and eligibility
of a particular operation. Capability tags describe technical contracts, not
user authorization, application readiness or scheduling entitlements.

For component ownership and request flow, start with [architecture](architecture.md).
For an operation's fields and errors, use the [API reference](rest-api.md).

## Inspect support

```sh
tt capabilities
tt capabilities --json
tt host show HOST_ID
```

`GET /api/capabilities` on the controller returns the versioned design matrix.
The same [typed registry](../crates/core/src/capability.rs) drives request
validation, scheduling, agent admission, offline resource checks, CLI output and
the dashboard's creation controls. It includes Linux engines and **experimental
FreeBSD** engines regardless of the controller's own operating system.

The matrix contains `version`, tag `definitions`, `create_fields` and all
engine/storage `combinations`. Each combination includes its platform,
experimental status and explicit support/limits for every defined tag.
`create_fields` maps request fields to alternative supported tags; for example,
SSH keys can use managed bootstrap or Jail root-key injection. It is a rendering
hint, not a replacement for complete request validation.

The matrix describes implemented contracts only. Internal QEMU cloud-init data
is not the caller-facing `guest_config` API. Outgoing denial and network
isolation are separate operations: bhyve implements the former, not the latter.
Jail root keys do not promise custom accounts, sudo or managed SSH metadata.
Stopped resource changes remain limited to QEMU and Firecracker. Read the
[guest guide](guest-images.md), [SSH contract](ssh.md) and
[FreeBSD limits](compatibility.md#experimental-freebsd-restoration) for the
underlying behavior.

## Host reports

Agent `/api/info` and controller host responses add an optional
`capability_report`. Version 1 has:

- `storage`: the agent's configured VM storage backend.
- `engines`: one entry per known engine, each with canonical tag statuses.
- Each status has `tag`, `enabled`, an optional denial `reason`, and `evidence`
  identifying the engine/probes behind an enabled result.
- `backup_admission`: the separate opt-in creation/refresh setting.

An enabled status is scoped to **this engine and this storage backend**. It cannot
be borrowed from another engine on the same host. Docker's own image store
remains independent of the host's VM storage setting. The legacy flat
`capabilities` list continues to carry the existing protocol and compatibility
identifiers; it does not carry an ambiguous union of the new canonical tags.

Reports reflect startup engine/tool probes and runtime-filesystem qualification.
They are not continuous checks of binaries, privileges, firewall configuration,
images or live guest processes. An evidence entry identifying an implementation
does not claim that every host or guest prerequisite was probed. Relevant engine,
network and storage checks still run when the operation executes. Restart an agent
after changing prerequisites to refresh its report.

Host refresh replaces the entire report. An offline host retains its last report,
labelled as historical in CLI/dashboard output, and cannot receive placement.
Unknown tag strings survive transport and persistence but never satisfy a known
requirement. Unknown report versions, a mismatched storage backend, missing
required entries or duplicate matching entries reject fresh operations. A present
canonical report is authoritative even if the legacy list still contains a grant.

## Decisions and recovery

A fresh request must satisfy the shared engine/storage design, its scoped host
requirements, resource/image checks and the operation's current runtime conditions.
Failures identify the canonical capability and distinguish unsupported design
from an unavailable host prerequisite. Per-instance failures retain their existing
storage and lifecycle diagnostics: a supported host mechanism does not establish
that a particular qcow2 image, zvol, running VM or ownership record is eligible.

For example, Docker disk sizing is unsupported by design; a Firecracker host
without the configuration-drive prerequisite reports an unavailable capability;
a running bhyve VM on qualified zvol storage must still stop before backup.

Already accepted work retains its recovery contract. Exact creation retries are
matched against the saved request before fresh admission. A recorded resource
update can retry its exact target after a host capability disappears, while a
new resource update is rejected. Stop, delete and ordinary retained-disk start
continue through their lifecycle checks instead of a fresh-create capability gate.

Backup inspection and exact replay keep the existing `disk_backup_v1` wire gate.
Disabling admission prevents new backup creation/refresh; it does not prevent
accepted work, restore or cleanup of existing artifacts. The canonical
`disk.backup.point` status describes mechanism qualification, independently of
`backup_admission`. Read [disk backup](disk-backup.md) for instance qualification,
revision headers, exclusions and unknown outcomes.

## Compatibility

An agent that omits `capability_report` uses the explicit legacy mapping in the
registry. Existing SSH, isolation, Firecracker jailer/zvol, resource-update and
bhyve outgoing-policy gates remain engine-specific. A canonical tag inserted into
a legacy flat list does not satisfy an old gate. There is no fallback to legacy
grants when a present canonical report denies or cannot interpret a requirement.

The report is additive JSON metadata and does not change database schemas or add
an in-agent state migration. [Deployment compatibility](deployment.md#persistent-state-schema-gate)
owns the current versions and upgrade paths. Older readers may
ignore the optional report; current agents retain the legacy identifiers for that
compatibility path. Removing them requires an explicit future compatibility
transition, not a timed removal after an arbitrary release. The backup protocol
identifier remains versioned because inspection, idempotent mutation and recovery
must be negotiated together.

Build selection is separate from runtime tags. `cfg(target_os)` selects native
host implementations; tags cannot enable Linux host code on FreeBSD or FreeBSD
host code on Linux. Shared models and the matrix remain readable on both targets
for mixed fleets. See [compatibility](compatibility.md) for tested scope and
[validation evidence](README.md#validation-evidence) for exact revisions.
