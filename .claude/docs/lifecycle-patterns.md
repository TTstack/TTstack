# TTstack Lifecycle and Contract Patterns

The invariants changes and reviews must preserve. Maintained behavior lives in
`docs/rest-api.md`, `docs/deployment.md`, and `docs/guest-images.md`; this file is
the review lens over them.

## Lifecycle

- **A timeout is not a failure.** A timed-out create may still be running. Inspect
  existing state before retrying, and never treat a retry as a fresh request.
- **Stop retains disks, not VM memory.** Stop/start preserves written data; anything
  held only in guest memory is gone. Do not promise more.
- **Delete is destructive** and releases storage. Partial failure must not report
  success, and must not delete a resource whose ownership is unproven.
- **Cleanup steps are independent.** After a confirmed process stop, attempt each
  cleanup step and record the failures instead of aborting the rest. A cleanup failure
  must not mask the primary result.
- **Preserve artifacts for inspection** when a destructive step fails — a failed
  unmount keeps the mount and image.
- **Unknown resources are retained, not silently released.** A missing agent row, an
  unreadable record, or ambiguous live-process identity is reported; automatic
  deletion is not a repair.
- **Process readiness is not application readiness.** Say which one was observed.

## Placement and resources

- Advertised CPU and memory are configured scheduling budgets, not measured free
  capacity; operators keep host and VMM headroom.
- Reservations are retained while their owner is uncertain and counted for unknown VMs;
  placement subtracts full disk sizes, including image-sized Firecracker disks and
  configuration drives.
- A stale snapshot must not place a VM; recheck between planning and mutation.
- Host capability strings gate engine features: `isolated_network`,
  `firecracker_jailer`, `guest_config`, `firecracker_disk_resize`,
  `firecracker_resources`, `firecracker_zvol`, `qemu_resources`. A shared API change
  that needs an agent capability requires the capability check, not an assumption.

## Networking

- TAP and bridge ownership is explicit: a stopped persistent TAP may change owner, a
  missing live TAP requires stop/start, and cold start must not delete a TAP before its
  replacement exists.
- NAT and port-forward rules are idempotent, preserve guest entries, and are safe to
  retry; a missing rule is a cleanup case, not an error.
- Port allocation covers the agent's own records; unrelated host processes are the
  operator's responsibility, as the guest guide documents.
- Guest network isolation is independent of a guest's own firewall.
- Never reset host networking or another service's rules to make a case pass.

## Persistent state

- SQLite holds runtime state, and a lifetime lock keeps one writer per state directory.
  A second writer is refused, not merged.
- `SCHEMA_VERSION` in `crates/ctl/src/db.rs` and `crates/agent/src/runtime.rs` and the
  stored engine values (`qemu`, `firecracker`, `docker`) are compatibility gates: older
  binaries reject newer schemas, unknown stored values are rejected rather than
  remapped or deleted, and controller and agent upgrade together.
- One corrupt row must not block unrelated recovery; strict inspection paths still
  report corruption instead of hiding it.

## Authorization and secrets

- The API key is a shared administrator credential. Clients refuse redirects; keys are
  never echoed, logged, or sent over an unverified channel.
- Credentials and private keys stay out of logs, reports, and commit messages.
- Deployment secrets travel over stdin, payloads are checksum-verified, and installed
  files are private and atomically placed.
- Owner fields in API payloads are labels, not authorization.
