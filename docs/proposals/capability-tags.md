# Unified capability tags for engines, storage and requests

Status: proposal. Nothing below is implemented. Maintained behavior stays in
[compatibility](../compatibility.md), [guest images](../guest-images.md),
[REST API](../rest-api.md) and [disk backup](../disk-backup.md); this document
replaces neither those guides nor their acceptance criteria, and it is not
authorization to implement. It proposes one vocabulary and one match path for
questions those guides currently answer in prose and the code answers in
scattered branches.

Baseline: written 2026-09-29 against `ec64c45`. Source inspection only; no host,
engine, storage or guest experiment was run, and no capability is added, removed
or re-verified by this proposal. The [proposal review](audit.md) has not yet
covered this document.

## 1. The problem

Five different kinds of statement share one flat namespace of eleven host
strings:

| Today's string | Kind of statement | Consumers |
| --- | --- | --- |
| `disk_backup_v1` | Protocol version | Backup admission only |
| `disk_backup_zvol_v1`, `disk_backup_reflink_v1` | Protocol plus which storage primitive qualified | **None** — produced and documented, never read |
| `firecracker_zvol` | An engine/storage combination is usable at all | [`place_vm`](../../crates/ctl/src/scheduler.rs) |
| `isolated_network`, `firecracker_jailer` | A host prerequisite (tools, cgroup controllers) was detected | `place_vm` |
| `ssh_bootstrap`, `guest_config`, `qemu_resources`, `firecracker_resources`, `firecracker_disk_resize` | A caller-visible operation with engine-specific limits | `place_vm`, [`crates/ctl/src/handler.rs`](../../crates/ctl/src/handler.rs) |

The clearest evidence is that the same facts are encoded **three times** as
separate support matrices with separate error vocabularies:

- [`validate_vm_options`](../../crates/core/src/model.rs) rejects `--disk` for
  Docker, `--deny-outgoing` for Docker and SSH keys for Docker with three
  hand-written messages.
- [`guest_config::validate`](../../crates/core/src/guest_config.rs) independently
  rejects configuration files for anything but Firecracker and isolation for
  anything but QEMU/Firecracker.
- [`place_vm`](../../crates/ctl/src/scheduler.rs) decides the same questions
  again from host capability strings, and the agent's probe decides them a third
  time by never advertising the tag.

A request can therefore pass one gate and fail another, and each gate explains
itself differently.

Concretely, the same question — *can this engine, on this storage, do this
operation?* — is answered in four places that are maintained separately:

1. **Agent probe strings.** [`detect_capabilities`](../../crates/agent/src/runtime.rs)
   builds nine of the strings in one function, and
   [`agent_info`](../../crates/agent/src/runtime.rs) appends the three backup
   strings in a different one, because their availability also depends on a
   runtime flag (`--enable-disk-backup`) rather than only on probes. There is no
   statement of a tag's meaning, its scope, or its limits at the definition site.
2. **Controller conjunctions.** The `supports` closure inside `place_vm` mixes
   `spec.engine != Engine::Firecracker` clauses with `has("...")` clauses, and its
   failure path restates the same list as English prose ("`guest_config`,
   `isolated_network`, jailed Firecracker, disk sizing or Firecracker zvol
   storage"). Engine names appear both inside tag names and beside them.
3. **Point checks at call sites.** `crates/ctl/src/backup.rs` checks
   `disk_backup_v1` directly; `crates/ctl/src/handler.rs` re-derives an
   engine→capability mapping with its own `match vm.engine` and its own error
   text.
4. **Implementation branches and prose.** The engines and storage backends know
   what they can do and reject the rest with ad-hoc strings (Docker SSH keys,
   container disk growth), while [compatibility](../compatibility.md),
   [guest images](../guest-images.md), [REST API](../rest-api.md) and
   [disk backup](../disk-backup.md) each restate part of the resulting matrix in
   hand-written tables.

Four failures follow from that structure, and they are the reason to unify:

- **Adding one capability means editing four places.** Omitting one produces a
  silent mismatch: placement admits a request an engine then rejects mid-create,
  or no host is eligible and the operator receives a prose list that no longer
  matches the code.
- **Three different questions are conflated.** *Supported by design* for
  (engine, storage) — `firecracker_zvol`; *enabled on this host* — a probe found
  `zfs`; *eligible for this instance* — this pool, this image. The
  [disk-backup guide](../disk-backup.md#supported-storage) already has to warn
  that "a host capability alone does not validate every disk image or underlying
  storage configuration", and the third question is answered by a separate
  `supported` flag from inspection. One flat `Vec<String>` cannot express which
  of the three answered "no", so operators get "upgrade agent and controller"
  when the true answer is "this filesystem does not support reflink".
- **The matrix lives in prose.** No caller can ask what
  `engine=firecracker storage=file` supports; the answer exists only in
  hand-maintained tables and in `match` arms.
- **Naming drifts.** `qemu_resources` and `firecracker_resources` name the engine
  and hide the operation; `firecracker_disk_resize` and
  `firecracker_resources` overlap without saying how.

### 1.1 Evidence of drift today

Each row is a fact that exists in more than one place, or is stated in a
location that cannot enforce it.

| Fact | Copies |
| --- | --- |
| Firecracker reserves a 4 MiB configuration disk | `VmOptions::config_disk_mib`, re-derived inline in `scheduler.rs`, `agent/runtime.rs` and `agent/backup.rs` |
| Backup storage primitive is zvol or reflink | The agent emits `disk_backup_zvol_v1`/`disk_backup_reflink_v1`, **no code reads them**, and `agent/backup.rs` re-derives storage→`Backend` in two more places |
| SSH keys need QEMU or a prepared Firecracker image | Server validation, agent probe, and dashboard JavaScript that disables the field for anything but `qemu` |
| Discs, outgoing filtering and SSH keys are unavailable on Docker | `validate_vm_options` (three messages), dashboard JavaScript, and the agent probe that never advertises the tags |
| Isolation requires QEMU or Firecracker | `guest_config::validate`, the scheduler's tag check, and the dashboard, which exposes the toggle regardless |
| A host "supports" a combination | `detect_capabilities` for eight tags, `agent_info` for three, and the request-field names collide with tag names (`isolated_network`, `guest_config`) |

The backup row is the sharpest: a capability is advertised, documented, and
never consulted, while the decision it was meant to carry is made again by the
storage backend. A tag nothing enforces is worse than no tag, because it looks
like a gate.

## 2. Model: three layers, one vocabulary

The proposal separates the three questions instead of continuing to encode them
in one namespace, and gives each layer one home.

| Layer | Question | Owner | Lifetime |
| --- | --- | --- | --- |
| **Design matrix** | Does (engine, storage, format) support this operation, and with which limits? | `ttcore`, static data | changes only with code |
| **Host enablement** | Is it enabled on this host, and if not, why not? | the agent's existing startup probes | changes per host and per agent restart |
| **Instance eligibility** | Does this request's image, pool, capacity and state qualify? | the operation's preflight, as today | changes per request |

A request then matches when all three agree, and every rejection can name the
layer that refused it.

### 2.1 Tag grammar

- Lowercase ASCII, dot-separated, `<domain>.<subject>.<qualifier>[:<variant>]`.
- Domains: `vm`, `disk`, `net`, `guest`, `host`, `image`.
- The tag names an **operation or contract**, never an engine, a tool, or a
  probe. `firecracker_disk_resize` becomes `vm.disk.grow.filesystem`; the fact
  that it is Firecracker that offers it belongs to the matrix.
- The engine and the storage backend are **dimensions of the matrix**, not
  segments of the tag. This is what lets one row describe three engines and
  makes the combinations enumerable.
- A tag splits into variants only when the **caller contract differs**, not when
  only the mechanism differs. QEMU cloud-init and a prepared Firecracker image
  both satisfy `vm.ssh.bootstrap`; the mechanism belongs to a limit note, not to
  a second tag.
- Tags are constants in one module. Matching takes typed values, so a scattered
  string literal cannot silently diverge from the vocabulary — it does not
  compile.

### 2.2 Vocabulary

| Tag | Scope | Meaning | Limits carried by the matrix entry |
| --- | --- | --- | --- |
| `vm.ssh.bootstrap` | create | Initial account and authorized key provisioned, endpoint reported | Prepared guest image required; no key rotation, expiry or reset |
| `vm.cpu.offline` | stopped | vCPU count changed on a stopped VM | Cold boot required for the guest to see it |
| `vm.mem.offline` | stopped | Memory changed on a stopped VM | Cold boot required |
| `vm.disk.grow.virtual` | create, stopped | Virtual disk enlarged | QEMU expands the virtual disk only; the guest must grow partitions/filesystems |
| `vm.disk.grow.filesystem` | create, stopped | Guest ext4 filesystem also grown | Requires the prepared-image contract and `e2fsck`/`resize2fs` |
| `guest.config.drive` | create | Guest configuration delivered on a read-only drive | Size and file/byte limits from the image guide; managed SSH seed counts against them |
| `net.isolation` | create | Per-VM isolated network and firewall policy | Linux only; blocks peers, guest-initiated host access, private/link-local destinations and IPv6 |
| `net.port.publish` | create, runtime | Guest TCP port reachable on the host | Routed ingress is a deployment concern, not a guest guarantee |
| `host.jailer` | create | Jailed VMM with cgroup CPU/memory/pids limits | Requires the cgroup v2 controllers to be delegated |
| `disk.backup.point` | stopped | Stopped-VM root-disk recovery point | Snapshot/reflink shares the source's failure domain; not disaster recovery |
| `disk.backup.restore` | stopped | Restore a recovery point over the active disk | Destructive to later writes on that disk |
| `vm.console.interactive` | runtime | Interactive console | Declared for completeness; no engine enables it today |

Two things deliberately do **not** become tags:

- **The backup protocol version** (`disk_backup_v1`) is a versioned wire
  contract, not a capability. Proposed as an explicit field
  (`backup_protocol: 1`) so a version mismatch is reported as a version, not as
  a missing ability.
- **`firecracker_zvol`** is the answer to "is this engine/storage combination
  usable at all". It becomes matrix coverage for `(Firecracker, Zvol)`; a
  combination absent from the matrix is unsupported by design, with no tag
  needed to say so.

### 2.3 Design matrix

The matrix is static data in `ttcore`: one entry per (tag, engine, storage),
each with the scope, a one-line meaning, and an optional limit sentence. The
rows below restate what the current tags, branches and guides already imply;
they are the proposal's claim to verify against the code, not new support.

| Tag | QEMU + file (qcow2) | QEMU + zvol (raw) | FC + file | FC + zvol | Docker |
| --- | --- | --- | --- | --- | --- |
| `vm.ssh.bootstrap` | yes | yes | yes (prepared image) | yes | no |
| `vm.cpu.offline`, `vm.mem.offline` | yes | yes | yes | yes | no |
| `vm.disk.grow.virtual` | yes | yes | yes | yes | no |
| `vm.disk.grow.filesystem` | no | no | yes | yes | no |
| `guest.config.drive` | yes (cloud-init) | yes | yes | yes | no |
| `net.isolation` | yes | yes | yes | yes | no |
| `net.port.publish` | yes | yes | yes | yes | yes |
| `host.jailer` | no | no | yes | yes | no |
| `disk.backup.point` | reflink-qualified | yes (ZFS snapshot) | reflink-qualified | yes | no |
| `disk.backup.restore` | reflink-qualified | yes | reflink-qualified | yes | no |

`reflink-qualified` is the honest cell: for file storage the design supports
recovery points, but only on a storage instance that passes the qualification
the [disk-backup guide](../disk-backup.md#supported-storage) defines. That is a
**host enablement** answer, not a matrix answer, which is exactly why the flat
namespace could not express it.

The matrix is total over engine × storage, and every tag has an entry for every
pair — `no` is a value, not an omission. A unit test asserts totality, unique
tag names, and that every tag string in the tree resolves to a matrix row or a
parser constant.

An entry may also carry the accounting the operation implies, so that the
configuration-disk reservation stops being re-derived in four places: the
`guest.config.drive` row is where "Firecracker reserves 4 MiB" belongs.

The [disk-backup proposal](vm-disk-backup.md#9-ownership-compatibility-and-rollout)
already requires capability identifiers that "represent the complete
create/restore/remove/recovery contract, not merely the presence of a binary",
plus per-VM eligibility reported separately from host capability. Those are the
first and third layers of this model. Its choice of
`disk_backup_zvol_v1`/`disk_backup_reflink_v1` as *identifier* names is the one
place this proposal differs: the version moves to a protocol field and the
storage primitive to the host-enablement evidence, so that one tag names one
contract instead of encoding a version and a backend in a single string. If the
review prefers the literal identifier, the matrix can carry both without
changing the layers.

### 2.4 Host enablement

The agent keeps its startup probes and reports, per tag, whether the host
enables it and what the evidence or denial was:

- **Enabled** carries evidence: `tool:zfs version`, `sysfs:/dev/kvm`,
  `cgroup:cpu,memory,pids`, `storage:/var/lib/ttstack reflink probe`,
  `config:--enable-disk-backup`.
- **Denied** carries a reason from a closed set: engine not installed, platform
  unsupported, prerequisite tool missing, storage instance unqualified, disabled
  by agent flag, or unknown.

The wire keeps `capabilities: Vec<String>` with canonical tag ids so the enabled
set stays readable by an older consumer, and adds an evidence list that only
operator surfaces need. A tag from a newer agent that the controller does not
know is retained and displayed, never treated as satisfying a requirement.
Enablement and evidence follow the existing snapshot lifecycle: the controller
replaces the whole report when a host reports in, so an offline host shows its
last known enablement, labelled as such, rather than a partially merged set.

### 2.5 Request requirements

One function derives the requirement set from the request instead of scattering
conjunctions:

```
requirements(env_spec, vm_spec) -> Vec<Requirement { tag, because }>
```

- `vm.ssh.bootstrap` — when the environment or VM carries public keys.
- `guest.config.drive` — when `guest_config` is non-empty (and for the managed
  SSH seed on Firecracker).
- `net.isolation` — when `isolated_network` is true; `deny_outgoing` implies the
  isolation prerequisite rather than a second tag.
- `vm.disk.grow.virtual` / `.filesystem` — when `disk` is set, with the variant
  chosen by what the request needs, not by the engine name.
- `vm.cpu.offline`, `vm.mem.offline` — for resource updates; the current
  `match vm.engine` in `handler.rs` disappears.
- `disk.backup.point` — for backup admission, replacing the direct
  `disk_backup_v1` check.

Matching is then one predicate over the three layers, and request validation,
placement, admission and the resize path all call it. Failure output names each
unmet tag, the layer that refused it, and the matrix row's limit sentence —
derived from the data instead of the hand-written prose list in `place_vm` and
the three separate Docker messages in `validate_vm_options`. A request that
fails validation and a request that finds no eligible host then describe the
same limitation in the same words.

### 2.6 Reported surfaces

- `GET /api/capabilities` returns the design matrix with its version, so callers
  and the dashboard can render support without reading sources or guides.
- `Host` keeps `capabilities` and gains the denial reasons, so an operator can
  see *why* a host does not offer `disk.backup.point` — "file storage
  `/var/lib/ttstack` does not support reflink" instead of "upgrade agent and
  controller".
- `tt capabilities` prints the grid; `tt hosts show <id>` prints the host's
  enablement against it.
- The guides stay the maintained behavior descriptions and cite the matrix; the
  hand-written engine rows in [compatibility](../compatibility.md) and
  [guest images](../guest-images.md) are re-derived and checked in the same unit
  that changes a matrix row, per the repository's one-description rule.

## 3. Compatibility and evolution

The host capability strings are one of the real compatibility gates alongside
`SCHEMA_VERSION`, and controller and agent upgrade together. The transition
therefore preserves the old names as an explicit translation:

- A **legacy map** translates the eleven current strings to canonical tags, and
  canonical tags back, with a test asserting the map preserves today's placement
  and admission decisions for every legacy host fixture.
- The agent emits canonical tags plus the legacy strings for one release; the
  controller accepts either. Reads never fail on an unknown string.
- **Fail closed:** an unknown tag never satisfies a requirement, and a
  requirement whose tag is absent from the host's report is a denial, never an
  implicit yes. Renaming or removing a tag is a capability-set version change,
  announced in deployment notes, not a silent edit.
- The legacy strings and the map are removed one release after the last
  supported agent emits canonical tags, in the same unit that bumps the
  capability-set version.

## 4. Migration units

Each unit is one commit with its tests and its guide updates, per the repository
commit protocol. Unit 1 changes no behavior; unit 2 changes only the wording of
validation errors and keeps every accept/reject decision.

1. **`ttcore`: vocabulary and design matrix.** Constants, matrix data, totality
   and uniqueness tests, the legacy translation table, and the requirement
   derivation as a pure function with table-driven tests. No caller changes;
   this unit fixes the vocabulary before anything consumes it.
2. **`ttcore`: request validation from the matrix.**
   `validate_vm_options` and `guest_config::validate` become lookups against the
   matrix for the fields they already gate (`disk`, `deny_outgoing`, `ssh_keys`,
   `guest_config`, isolation), keeping every current accept/reject decision and
   their tests. Only the message text changes, which this unit's commit message
   must mention.
3. **`ttagent`: canonical report with evidence.** Merge `detect_capabilities`
   and the backup push in `agent_info` into one probe path emitting canonical
   tags with evidence and denial reasons; keep emitting legacy strings. Covered
   by unit tests over probe fixtures; no new live evidence is claimed, since the
   probe set is unchanged.
4. **`ttctl`: placement and admission through the matcher.** Replace the
   `supports` closure and the direct `disk_backup_v1` check with
   `requirements` + matcher. The three existing scheduler tests
   (`firecracker_zvol_requires_an_upgraded_agent`,
   `isolation_and_configuration_are_never_scheduled_on_legacy_agents`,
   `docker_placement_does_not_prefer_vm_storage`) must pass unchanged.
5. **`ttctl`: resize path through the matcher.** Remove the `match vm.engine`
   engine→capability mapping and the ad-hoc "resource updates require QEMU or
   Firecracker" text in favour of an unmet-requirement report.
6. **Surfaces.** `GET /api/capabilities`, host denial reasons, CLI grid,
   dashboard rendering, and the guide updates that cite the matrix. The
   dashboard's client-side availability rules are replaced by rendering the
   matrix for the selected engine and storage, which also removes the current
   case where the form forbids SSH keys for a prepared Firecracker image that
   the server accepts.
7. **Cleanup.** Drop the legacy strings, map and capability-set version bump
   once no supported agent emits them.

## 5. Acceptance criteria

- Matrix totality: every tag × engine × storage pair is classified, tag ids are
  unique, every tag carries a meaning sentence, and every engine/storage-
  specific limit is present rather than implied.
- Legacy equivalence: for each of the eleven current strings, the translation
  reproduces the current placement, admission and resize decisions on the
  existing test fixtures.
- Validation parity: the same requests that `validate_vm_options` and
  `guest_config::validate` accept or reject today are accepted or rejected from
  the matrix, with their existing tests retained rather than rewritten.
- Requirement derivation: table-driven tests over the request fields that drive
  requirements (`ssh_keys`, `guest_config`, `isolated_network`, `deny_outgoing`,
  `disk`, resource updates, backup), asserting the derived set and the resulting
  decision.
- Three distinguishable refusals, each with one test: unsupported by design
  (Docker disk growth), supported but not enabled on this host (no `nft`), and
  enabled but not eligible for this instance (plain filesystem, unqualified
  image).
- No tag literal outside the vocabulary module: the matcher takes typed values,
  so a divergent string cannot compile; a test additionally greps the tree for
  the legacy names after unit 6.
- Units 1–5 preserve every accept/reject decision, changing only version-gate
  metadata and error text. They need the workspace checks from the commit
  protocol, not live validation. Any change to the probe set or to a decision
  needs the corresponding host evidence under the existing
  [live-validation workflow](../../.claude/skills/x-live/SKILL.md); this
  proposal claims none.

## 6. Open questions

- **Variant granularity.** The rule above (split on contract, not mechanism) is
  proposed, not settled. The first candidate for splitting further is
  `vm.disk.grow.filesystem`, where creation-time growth and stopped-VM growth
  may read differently to a caller.
- **Where `deny_outgoing` belongs.** It is currently a request field validated
  per engine. It is arguably a qualifier of `net.isolation` rather than its own
  tag; the proposal keeps it as a requirement implication, which should be
  confirmed against the validation path.
- **Protocol versions as fields.** Whether the backup protocol version should be
  a tag after all, for uniform reporting, at the cost of mixing versions into a
  capability list.
- **Dashboard depth.** Whether denials and evidence should be visible on the
  fleet page or only per host, given the API key remains a shared administrator
  credential.
- **Docs generation.** Whether the compatibility and image-guide matrices should
  be generated from the design matrix, or stay hand-written and verified against
  it. Generated tables remove drift but fit the repository's documentation
  style less well.

## 7. Limits

- Capability tags describe **technical support**, never authorization. Owner
  labels and tags are not entitlements; the API key remains a shared
  administrator credential.
- A matrix row is a statement about an engine and a storage backend, not about a
  particular image, pool, or guest. Instance eligibility stays a separate check
  and keeps its explicit `unsupported` result.
- A tag says nothing about application readiness. VM process readiness is still
  not application readiness, and the backup row is still not disaster recovery.
- This document is design context. It proposes no support change, no schedule,
  and no live evidence; the maintained guides remain the descriptions of current
  behavior until a unit above is implemented and validated.

Return to the [documentation index](../README.md).
