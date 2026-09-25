# TTstack audit

> Auto-managed by `/x-review` and `/x-commit`; registry edits are not code writes.
> Confirmed findings are appended under **Open**; a disproven finding is removed
> rather than kept as backlog; resolved history belongs in Git and the dated
> validation reports. Entry shape and severity: `.claude/docs/review-core.md` §5.
>
> The disposition below is this registry's initial record.

## Open

None.

## Won't Fix

None.

## Disposition, 2026-09-25

The original review covered `83b7e95`. Findings were rechecked against `6866295`
and the working-tree changes described below. This document records dispositions,
not a new behavioral specification. Maintained contracts are indexed in
[README.md](README.md); test scope and live observations are in the
[audit validation report](audit-validation-2026-09-25.md).

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

See the [validation report](audit-validation-2026-09-25.md) for actual checks,
resource bounds, cleanup, and the distinction between host tests and local tests.
