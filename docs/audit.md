# TTstack audit

> Managed by `$x-review`, `$x-overhaul`, and `$x-commit`; registry edits are not code writes.
> Confirmed findings are appended under **Open**; a disproven finding is removed
> rather than kept as backlog; resolved history belongs in Git and the dated
> validation reports. Entry shape and severity: [review core](ai/review-core.md#5-audit-registry) §5.
>
> Dated dispositions identify the reviewed revisions and their evidence.
> Design reviews are not registry entries. The
> [proposal review](proposals/audit.md) records draft status and adjacent
> contract notes; it does not change the dispositions below.

## Open

### [HIGH] deploy: API-key env file is truncated in place, not atomically placed
- **Where**: `crates/cli/src/deploy.rs` (`remote_setup_script` env snippet, `local_install_systemd` env write)
- **What**: the service env file holding `TT_API_KEY` is emptied in place (`install /dev/null` + `tee` remotely; truncate+write locally) with no fsync and no atomic rename, on every routine redeploy. Trigger: power loss or crash during/shortly after redeploy.
- **Why**: an empty env file starts the controller/agent with no key, serving `0.0.0.0` management listeners unauthenticated; manual repair needed. Violates the installed-files "private and atomically placed" invariant the binary installation already follows.
- **Suggested fix**: stage + chmod 0600 + rename + sync in both paths; regression test asserting the preserved-key property of the local atomic writer and the script shape.

---

### [MEDIUM] agent-runtime: reconcile silently erases unresolved operation errors
- **Where**: `crates/agent/src/runtime.rs` (`reconcile_vm`)
- **What**: a failed stop persists `stop unfinished: ...` on a Running record; a transient engine query failure overwrites it with `cannot query engine: ...` (then clears it on recovery), and the blanket `vm.error = None` after successful network recovery clears it directly on the next scan. Trigger: failed stop followed by a routine reconcile scan or a transient engine query failure.
- **Why**: the unresolved partial failure disappears from the VM and the controller mirror with no retry or confirmation, contradicting "retaining unresolved operation errors" in `docs/rest-api.md`.
- **Suggested fix**: treat reconcile-owned error strings as the only ones it may overwrite/clear, and preserve the documented Failed→Running stale-readiness clear; regression tests for both paths fail today.

---

### [MEDIUM] ctl: host detach demotes a `deleting` environment to `failed`, ending cleanup retry
- **Where**: `crates/ctl/src/db.rs` (`detach_host`)
- **What**: detach unconditionally sets `env.state = Failed` and replaces the retained cleanup error. Trigger: stuck delete across an offline host and a failing online agent, then the documented `detach` remedy.
- **Why**: the 60-second expiry sweep only retries `Deleting` envs, so remaining cleanup on the other host stops permanently and its error is no longer visible; contradicts "retains remaining records as deleting. The controller retries".
- **Suggested fix**: preserve `Deleting`, append the detach note to the existing error; regression test detaching under a partial-delete fixture.

---

### [MEDIUM] ctl: plan persisted but provisioning never spawned when a post-plan write fails
- **Where**: `crates/ctl/src/handler.rs` (`create_environment`)
- **What**: after `put_environment` commits, a failing host-reservation write or `environment_detail` read returns 500 before `finish_creation` is spawned. Trigger: transient DB I/O error in that window.
- **Why**: the environment stays `creating` with no error and no task forever (`lifetime: 0` disables expiry), contradicting "partial failures remain visible in `env.error`".
- **Suggested fix**: spawn provisioning immediately after the plan commit and fall back to an in-memory detail; keep the reservation writes best-effort next to it.

---

### [MEDIUM] guest-config: 31-file limit with SSH seed enforced only after clone and seed writing
- **Where**: `crates/agent/src/runtime.rs` (`create_vm`), `crates/ctl/src/handler.rs` (`create_environment`)
- **What**: both layers validate only caller files (32 allowed); the SSH seed makes 33, and `guest_config::write_disk` fails the merged map after identity/ports/resources are reserved, the seed is persisted, and the image is cloned. Trigger: Firecracker create with 32 valid files plus SSH keys — deterministic.
- **Why**: a guaranteed-failing request is accepted (400 expected for invalid parameters) and lands in a `failed` record with partial reservations instead of an up-front rejection; `docs/guest-images.md` documents the effective 31-file limit.
- **Suggested fix**: reject `len() >= MAX_FILES` whenever Firecracker SSH bootstrap applies, at agent admission and controller validation; regression tests assert rejection before any record/clone.

---

### [MEDIUM] storage: file provisioning accepts backing-chained qcow2, coupling clones to a shared source
- **Where**: `crates/core/src/storage/file.rs` (`resize_disk`)
- **What**: create/resize validate format and size via `qemu-img info` but never inspect `backing-filename`; backup admission rejects the same condition. Trigger: operator imports a documented "existing layout" image created with `qemu-img create -b`.
- **Why**: the writable clone silently reads shared source blocks; later image housekeeping corrupts guests and reservations undercount storage, violating "a writable clone must never resolve to a shared source disk".
- **Suggested fix**: reject non-null backing references in the same inspection used for create/resize; fixture-parse regression test; document flattening via `qemu-img convert`.

---

### [MEDIUM] net: FreeBSD TAP ownership check is a substring match
- **Where**: `crates/core/src/net/freebsd.rs` (`create_tap`, `destroy_tap`)
- **What**: ownership is tested with `contains("description: ttstack:{vm_id}")`; prefix-related VM IDs (`web`/`web2`, possible via direct-agent IDs) make the guard pass for the wrong device. Trigger: stale record plus TAP number reuse across the crash window the ownership check exists to cover.
- **Why**: the cross-VM safety guard behind "renamed interfaces retain a separately recorded device" is bypassed, renaming/destroying another VM's TAP; contradicts documented ownership checks.
- **Suggested fix**: parse the description line and compare exactly; regression test with prefix-related IDs.

---

### [MEDIUM] agent: resource-update state conflicts return 500 instead of the documented 409
- **Where**: `crates/agent/src/handler.rs` (`resize_vm` status mapping)
- **What**: `conflict: ...` errors from backup guards (busy writer, retained artifacts) miss every mapped substring and fall through to 500. Trigger: resource update while a backup operation is unfinished or artifacts remain.
- **Why**: the controller surfaces 502, which the API guide defines as an unknown outcome; the actual outcome is a clean pre-mutation rejection, so operators are sent into the wrong recovery ritual.
- **Suggested fix**: map the `conflict: ` prefix to 409; unit-test the classifier with the produced strings.

---

### [MEDIUM] ctl: unschedulable-capacity error echoes raw request, not the applied reservation
- **Where**: `crates/ctl/src/scheduler.rs` (`place_vm` error branch)
- **What**: `disk` prints `spec.disk.unwrap_or(default_disk())` (0 for Firecracker) while `mem` already prints the overhead-inclusive reservation. Trigger: disk-constrained Firecracker host with an omitted disk size.
- **Why**: misleading operator output on the primary capacity diagnostic — the message claims 0 MiB is needed on a host that failed a 1028 MiB check, hiding the failing dimension.
- **Suggested fix**: report the computed disk reservation from a supporting host; regression tests for the Firecracker and CPU-bound cases.

---

### [MEDIUM] deploy: binary upload via scp has no time bound
- **Where**: `crates/cli/src/deploy.rs` (`SshTarget::copy`)
- **What**: `exec` is bounded at 900 s with an actionable message; `copy` lacks a timeout and `kill_on_drop`. Trigger: established-but-stalled TCP during a remote upload.
- **Why**: deployment silently hangs for the kernel retransmission lifetime with none of the documented "inspect before retrying" guidance.
- **Suggested fix**: apply the same timeout wrapper pattern; note the coverage gap for a hang test.

---

### [MEDIUM] deploy: config with no controller and no agents reports success
- **Where**: `crates/cli/src/deploy.rs` (`deploy_distributed`)
- **What**: with an explicit/sidecar key the "no controller or agents" error never fires; the loops deploy nothing and print "distributed deployment complete". Trigger: a role-less deploy.toml with an existing key file.
- **Why**: misleading success output contradicting "provide at least one role to deploy".
- **Suggested fix**: reject role-less configs up front; regression test.

---

### [LOW] image-builder: download size bound is not enforced
- **Where**: `crates/cli/src/image_builder.rs` (`download_file`)
- **What**: `curl --max-filesize` only compares the announced size and does nothing when it is absent or false. Trigger: compromised/misbehaving mirror serving chunked or lying responses.
- **Why**: staging grows without bound until the 900 s timeout, risking image-directory disk exhaustion on the agent host; checksums (verified after download) do not cap bytes.
- **Suggested fix**: reject above the bound from staged-file metadata before publish; regression test of the checker.

---

### [LOW] image-builder: umount failure masks the primary build error
- **Where**: `crates/cli/src/image_builder.rs` (`create_firecracker`)
- **What**: when image population fails and the cleanup `umount` also fails, only the umount error is reported. Trigger: bad mirror download plus a busy leftover mount reference.
- **Why**: the operator is sent to debug mounts while the actual cause is invisible; violates "a cleanup failure must not mask the primary result".
- **Suggested fix**: compose both errors into the reported message with the retained-artifact instructions.

---

### [LOW] net: FreeBSD bridge setup mutates an unverified interface
- **Where**: `crates/core/src/net/freebsd.rs` (`setup_bridge`)
- **What**: any interface answering to `tt0` is brought up and aliased without verifying it is a bridge (the Linux path errors before mutation). Trigger: operator/legacy tooling pre-created a non-bridge `tt0`.
- **Why**: host networking is mutated on an ownership-unproven interface and the failure only surfaces at the first guest create; contradicts the stated parity of ownership checks.
- **Suggested fix**: verify bridge group membership before configuring; fixture-based classifier regression test.

---

### [LOW] ctl: registration refresh drops storage and warnings
- **Where**: `crates/ctl/src/handler.rs` (`register_host`)
- **What**: the same-ID refresh branch omits `host.storage`; the fresh-registration branch stores `error: None` even when the agent reports warnings. Trigger: re-register after an agent storage change; register a host with catalog warnings.
- **Why**: response and stored row disagree with the refresh contract used by `apply_snapshot` until the next heartbeat silently corrects them; placement preference reads the stale storage in that window.
- **Suggested fix**: update storage on refresh and surface warnings on fresh registration; regression test with a mock report carrying both.

## Won't Fix

### [LOW] ctl-backup: reservation and forwarding intent are not one durable write
- **Where**: `crates/ctl/src/backup.rs` (`mutation`)
- **What**: `put_host` (reservation) and `put_vm` (forwarding intent) are two autocommitted writes; a crash between them leaves an orphan over-reservation without intent.
- **Reason**: residue is conservative-direction only (never intent without reservation), bounded to one VM root, and self-heals on the next host snapshot via absolute recomputation; an offline host cannot receive placement anyway. The marginal gain does not justify churn in the backup mutation critical path; `put_backup_observation` remains the transactional discipline for the observation path.

## Disposition, 2026-09-28

The later [2026-09-29 offline conversion](validation/agent-v5-conversion-2026-09-29.md)
adds a supported operator path for retaining native v5 VM metadata when upgrading
to v6. It leaves the daemon's strict native-schema gate intact. The report
separates conversion/rehearsal evidence from subsequent deployment acceptance.

Review of `bff24fb` identified additional boundary failures, corrected in the
implementation revision identified by the [two-host validation report](validation/lifecycle-corrections-2026-09-28.md):

- Docker deletion could remain stuck after a failed pull created no container.
  Stop now accepts confirmed absence and still propagates query/stop failures.
- An old agent snapshot could erase unresolved controller resize intent. Snapshot
  merging now retains the target and disk reservation until confirmation; a fresh
  request explicitly rejected before mutation can release only its new intent.
- Image inspection ran on the host-info request path. A single background catalog
  worker now keeps slow inspection independent of health and lifecycle reads.
- Published-port DNAT also matched traffic addressed to other hosts. Rules now
  restrict destination matching to local host addresses. The access guide and CLI
  distinguish direct host-local guest access from incoming mapped-port access.

Focused fault regressions and the complete workspace checks passed. Both
authorized hosts passed the bounded live cases and cleanup checks recorded in the
report. Maintained behavior remains in the [REST API](rest-api.md) and
[networking guide](guest-images.md#networking-and-platform-scope).

## Disposition, 2026-09-25

The original review covered `83b7e95`. Findings were rechecked against `6866295`
and the working-tree changes described below. This document records dispositions,
not a new behavioral specification. Maintained contracts are indexed in
[README.md](README.md); test scope and live observations are in the
[audit validation report](validation/audit-validation-2026-09-25.md).

The original 24 entries combined defects, design boundaries, unverified hypotheses,
and incorrect conclusions. Unsupported claims have been removed. Original numbers
below identify the substantiated portions; a resolved portion does not imply that
all assertions in the original compound entry were correct.

### Implemented corrections

| Original finding | Confirmed problem and correction | Evidence |
| --- | --- | --- |
| 1 | A slow agent held a fleet-wide mutation lock. Operations now serialize per environment; planning remains atomic, stale snapshots are discarded, and expiry work has bounded concurrency. Creation rechecks expiry between VMs. Restart reserves stopped guests before concurrent placement. Confirmed stop/start results survive an unrelated refresh invalidation. | Controller mock-agent concurrency, snapshot and restart-reservation regressions. |
| 2 | Dead hosts pinned fleet tracking; corrupt PID text blocked process recovery. Offline host detach now returns the orphan VM records and explicitly removes only controller tracking. PID recovery searches for exact VM-specific process arguments before deciding cleanup is safe. | Controller detach test; process recovery test and live corrupt-PID stop/start. |
| 3 | Registration could replace an existing host identity/address and rejected valid repeats at the host cap. Conflicting bindings now return 409; repeat registration updates metadata without dropping outstanding reservations. Invalid database reads fail visibly. | Registration regressions; trusted-address prerequisite documented in the REST guide. |
| 4 | Deploy sent keys in SSH command arguments, printed keys, and recursively reowned guest storage. Scripts now use SSH stdin, output names the protected key file, and ownership changes target the service/controller directories. | Deployment source review and generated-script tests. Full deployment was not run against existing services. |
| 5 | Failed resume prevented paused Firecracker termination; unrelated cleanup failures prevented subsequent cleanup. Failed resume now falls back to termination. After confirmed process stop, independent cleanup steps are attempted and failures remain recorded. Missing TTstack NAT objects are idempotent cleanup cases. | Live paused-resume failure and delete after removal of the isolated test NAT table. |
| 6 | An existing `tt0` was accepted without checking its type, address or state. Setup now checks for a bridge, restores its address and brings it up; failure to enable forwarding is propagated. | Live creation with a deliberately incomplete bridge. Firewall coexistence remains host-policy dependent, as described below. |
| 7 | Root image mounts and remote deployment uploads used predictable staging paths. Both now use private random directories. Uploaded binaries receive checksum verification and atomic installation. Failed unmount preserves the mount and image for inspection. | File/download tests; actual Firecracker recipe build on the test host. |
| 8 | Persistent network names depended on an unspecified standard-library hash. The previous SipHash-1-3 derivation is now explicit and frozen, retaining existing Linux x86_64 names. | Golden identifiers and existing naming tests; no naming migration. |
| 9, 13 | Missing sandbox metadata could make a live jailed VMM look stopped; QEMU used an unnecessarily broad process marker. Missing metadata now yields an explicit error for a detected jailed process. QEMU uses its full PID-file argument. Empty identity data for a live recorded process fails conservatively. | Live missing-sandbox test retained the VMM and disk; exact-process recovery regression. QEMU was not live-tested in this run. |
| 10 | Placement underestimated image-sized Firecracker disks/config drives; missing agent VM rows released controller reservations. Agents report image capacities and a combined resource/VM snapshot. Placement reserves the full disk, rejects known undersized disks, and retains missing-row reservations. Unknown agent VMs are counted and logged. Docker receives an explicit memory-plus-swap limit equal to its memory limit. | Scheduler and missing-snapshot tests; live 128 MiB root plus 4 MiB configuration-drive accounting. |
| 11 | Capability labels were platform constants. Startup now probes engine commands, KVM access and relevant networking, cgroup and storage prerequisites before advertising support. | Source review and live capability/image-size inspection. These probes are not daemon or application health checks. |
| 12 | Recovered boot/query/network errors remained sticky; list-level environment errors omitted VM causes; catalog failure or startup recovery hid a reachable agent. Reconcile clears recovered errors, environment errors include causes, catalog failure degrades independently, and recovery runs after listener initialization. | Agent/controller regression tests and restart with an injected transient network failure. |
| 14 | A request could monopolize the agent with an excessive port list. Requests now cap port entries at 256 before allocation. | Shared validation and documented host-port ownership boundary. External port ownership and conntrack are qualified below. |
| 15 | Restart flushed published-port rules and network recovery was single-shot; cold-start TAP ownership required destructive replacement. NAT setup preserves guest entries and recovery retries. A stopped persistent TAP changes owner without deletion. A missing live TAP is reported as requiring stop/start. | Live agent restart, injected nft failure, repeated cold starts and externally reached published ports. |
| 16 | One corrupt record blocked unrelated recovery, and multiple service processes could write the same state directory. Recovery iterates readable rows, retains/logs corrupt rows and closes agent admission when allocations are unknown. Targeted VM reads avoid unrelated bad rows. Writer busy timeouts and lifetime state-directory locks are installed. | Corrupt-row recovery/admission tests; strict inventory reads still report corruption instead of silently hiding it. |
| 17 | CLI timeouts/defaults and credential handling were inconsistent; private SSH keys could be sent before validation. Create uses the mutation timeout and starts its deadline before submission. Read errors no longer imply a submitted mutation. Saved credentials are private and atomic; same-address reuse preserves keys, different-address overrides do not forward them. SSH keys are validated before sending. Dashboard blank disk/lifetime fields use server defaults. | CLI/configuration regressions and dashboard source review. |
| 18 | Deploy interpolated unchecked values, rewrote documented mount guards, and had init/failure-reporting mismatches. Values are validated before remote work; documented mount guards migrate to a drop-in, existing drop-ins remain intact, SSH execution is bounded, diagnostics retain stdout/stderr, systemd detection checks the running system, and OpenRC uses supervision. | Injection and mount-guard tests, generated unit/script review. OpenRC and remote deployment remain untested live here. |
| 19 | File clones could inherit public base-image permissions. Clone destinations are private before copying; base ownership/modes are not propagated. Directory disk selection is deterministic. | Public-base/private-clone regression, file-store tests and live Firecracker clones. |
| 20 | Recipe downloads lacked integrity verification. Pinned recipes now require pinned digests; rolling Debian/Ubuntu images require the matching upstream checksum entry. Downloads are size-bounded and published only after verification. Existing QEMU destinations are inspected before being accepted. | Digest/manifest/mismatch regressions and a real verified Firecracker image build. Rolling manifests share the download origin; independent signature verification is not claimed. |
| 21 | Documentation omitted guest-to-host API reachability. Deployment/guest guides now explicitly cover non-isolated guests reaching a wildcard management listener. | Corrected deployment and networking contracts; optional unauthenticated manual operation remains intentional. |
| 22 | Empty/invalid keys could be accepted or silently omitted; HTTP client construction could panic. Services and clients validate keys without echoing them, construction errors propagate, and API clients refuse redirects. Bearer scheme matching is case-insensitive. | Authentication/client regressions and mixed-case Bearer headers in live recovery probes. |
| 23 | QEMU images were checked too late, and equivalent key/port orderings broke idempotency. Non-container images and QEMU virtual sizes are checked before allocation; retained option sets are normalized. | Agent idempotency/input regressions and storage preflight source review. |
| 24 | Database errors returned success status, offline hosts inflated schedulable totals, host failures lacked a persisted cause, and several diagnostics/UI details were inaccurate. Responses preserve status, totals use online hosts, host errors are exposed, capability diagnostics require the matching engine, and the dashboard escapes quotes and refuses framing. Guides identify relevant validation runs and required ext4 tools. | Controller/API/authentication tests and documentation/link review. |

### Remaining limits and unverified hypotheses

These are not claimed as fixed or promoted to proven incidents:

- **External port ownership and conntrack (14).** The agent prevents duplicate
  allocation among its own records, but its availability probe does not reserve a
  socket against unrelated host processes. Operators must reserve the documented
  port pool. Delete does not flush per-guest conntrack state; old sessions may
  survive IP/port reuse. No cross-guest session-inheritance failure was reproduced
  in this run. TTstack does not claim independent conntrack zones.
- **Docker/firewalld coexistence (6).** Another runtime's forward-hook drop policy
  can override TTstack's accept rules. This run used an isolated network namespace;
  it does not establish coexistence with arbitrary host firewall rules. The guest
  guide states operator ownership of those rules and the namespace alternative.
- **Registration trust (3).** A shared bearer credential authenticates API callers,
  not the target server. A self-reported public host ID cannot establish trust.
  Registration still requires a trusted address and protected management transport;
  identity/address conflict checks do not turn plain HTTP into peer authentication.
- **Resource budgets (10).** Advertised CPU/memory are configured scheduling
  budgets, not measured free capacity. Operators must leave host/VMM headroom.
  QEMU does not acquire Firecracker's cgroup guarantee through this change.
- **Irrecoverable metadata (2, 9, 16).** Unreadable records and ambiguous live
  process identity require restoration/inspection. Their resources are retained;
  neither automatic deletion of unknown resources nor blind JSON repair is safe.
- **Backend coverage.** No fresh QEMU, Podman, ZFS/zvol, OpenRC, musl or production
  deployment result is inferred from the Firecracker/file run. Older reports keep
  their original tested revisions and scopes.

See the [validation report](validation/audit-validation-2026-09-25.md) for actual checks,
resource bounds, cleanup, and the distinction between host tests and local tests.
