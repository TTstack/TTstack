# VM SSH access and recovery evidence

Status: Proposed requirements/design; not implemented or validated. Current
[API](../rest-api.md), [guest image](../guest-images.md) and
[compatibility](../compatibility.md) guides still describe shipped behavior.

## Engine-independent requirement

SSH key injection and a usable SSH connection are generic VM capabilities. QEMU
and Firecracker must expose the same caller contract; future VM engines must
implement an adapter. An unsupported image is a preparation error, not permission
to require a different login/key workflow for each virtualization engine.

TTstack owns provisioning, key installation/removal, connection metadata and
host-side recovery evidence. Callers own user identity, grants, stream lifetime
and automatic-restart policy. No SAIR subjects, OMM builds, Web terminal or general
guest RPC service belong here. Use OpenSSH and finite provisioning helpers.

## Existing behavior and gaps

`VmSpec` has `ssh_keys`, but `validate_vm_options` rejects nonempty keys for
Firecracker and Docker. QEMU injects root keys through cloud-init. Firecracker
uses a prepared rootfs and an opaque read-only configuration drive; it currently
has no managed SSH. TCP mappings exist at creation, with no general retained-VM
port-update endpoint.

QEMU/Firecracker host-controlled stop already attempts graceful shutdown and then
terminates the owned VMM. Stop/start retains disks. Reuse these primitives for
recovery rather than requiring guest SSH or adding application-specific lifecycle
APIs. Hung-guest behavior still needs dedicated acceptance.

Docker/Podman are containers, not another VM bootstrap. Keep their unsupported
result explicit until a prepared-container SSH adapter exists; injecting a key
cannot start sshd in an arbitrary image. This does not exempt either VM engine
from the common SSH requirement.

## Common contract

An SSH-enabled VM has a login user, guest SSH port, unique stable host key and a
managed public-key set. Each key record has an opaque ID, validated public key,
login user and optional absolute expiry. Preserve existing create-time `ssh_keys`
as the initial operator set; schema migration must not silently discard them.

The private administrative API must support these proposed operations:

| Operation | Contract |
| --- | --- |
| Provision SSH at creation | Prepare engine-specific seed, key files and TCP mapping; reject incompatible images explicitly |
| Add/remove a key by ID | Idempotent persisted intent, distinguish requested revision from confirmed installation |
| Read SSH status | VM identity, user, endpoint, pinned host key, applied key revision and SSH readiness |
| Enable SSH on a retained stopped VM | Add SSH seed/mapping without replacing its root disk or identity |
| Read diagnostic snapshot | Fresh host/VM observations and bounded evidence, independent of the workload |

These are semantics, not shipped endpoint names. Existing TTstack administrator
authentication remains required. Owner labels are not tenant authorization; callers
check the actual user. Private provisioning keys never appear in VM lists, status,
SSH descriptors or logs.

## Bootstrap and key updates

Keep initial provisioning in engine adapters: cloud-init for QEMU, a versioned SSH
bootstrap in prepared Firecracker images. Firecracker SSH material is generic
TTstack data, separate from opaque application `guest_config`; rotating SSH keys
must not rewrite an application's `workspace.json`. sshd starts independently of
the workload application.

For live key updates, use ordinary agent-to-guest SSH with a per-VM provisioning
identity held privately by the resource agent. Inject its public key separately
from caller-managed login keys. A finite helper updates only the managed
authorized-key file from validated records on stdin, with fixed ownership/modes
and atomic publication. No public arbitrary-command API or resident management
daemon is needed. Removing a caller key cannot remove the provisioning key used
for later rotation and repair.

Provision/pin a unique SSH host key through trusted bootstrap; do not clone a host
private key from the shared image or accept a changed key on reconnect. Protect
provisioning private material in agent state. Guest root can change its own sshd;
managed SSH is not a containment boundary against guest root. The agent's private
provisioning key remains outside that guest.

Persist desired key revision before delivery and confirm the applied revision
before success. Reconcile lost replies using the same revision; reject conflicting
updates until resolved. Reconcile desired keys after cold start so expired keys
cannot reappear. If sshd is broken, retain pending state and report installation
unavailable; use host-side recovery or operator repair, not fabricated success.

[OpenSSH authorized-key expiry](https://man.openbsd.org/sshd#expiry-time) rejects
new authentication after the deadline. Expiry/removal does not terminate existing
sessions or detached processes. A caller needing connection deadlines must own
and close the transport; full workload termination requires VM stop. Do not label
key deletion as complete workload revocation.

## Retained VMs and networking

A stopped-VM access update adds SSH bootstrap and a generic TCP mapping without
deleting/recloning the root disk. Persist intent before network/seed changes,
serialize with lifecycle/resources, and reconcile interrupted updates. Images
without the bootstrap require explicit operator preparation. Do not rewrite opaque
application settings or replace an existing rootfs merely to obtain SSH.

The agent uses the host-local guest address. Remote callers use the actual host
mapping; loopback DNAT must not be assumed. Preserve isolation and accounting.
Published mappings are not automatically private: deployment must restrict ingress
to the designated gateway/management network. TTstack's admin key is never an
end user's SSH credential.

## Recovery evidence

Callers compose durable stop/start. Even when guest shutdown fails, the agent must
stop the owned runtime within a bounded deadline. Verify process identity before
termination and exit before starting another VMM. PID reuse or a missing control
socket is not proof of exit. Retain disk, VM ID, allocation, network and SSH host
identity. Agent unreachability is an unknown outcome, not successful shutdown.

Provide a diagnostic snapshot independent of application readiness:

- Observation time and fresh/cached status; agent reachability, engine, persisted
  VM state and observed VMM process identity/state.
- Agent-to-guest SSH reachability, distinguishing guest failure from a remote
  caller's broken network path. A stopped VM has no expected SSH response.
- Last observed VMM exit, matching process/cgroup OOM evidence and operation
  errors, with attribution to this VM and an evidence source.
- Bounded VM-specific boot/runtime excerpts where available. Missing evidence
  stays missing. Guest-kernel and host/VMM OOM are different findings; a timeout
  alone establishes neither.

Diagnostic collection has deadlines and cannot indefinitely delay stop. Restrict
raw reports to administrative callers, which sanitize user-facing summaries.
Never expose arbitrary host files, global logs or other VMs' records. Structured
logs carry VM and caller operation IDs for correlation without secrets or full
environment dumps.

TTstack supplies evidence/primitives, not a second automatic-reboot policy. Callers
know desired runtime state, intentional maintenance and current user authority.

## Delivery and acceptance

Advertise managed-SSH/key-update/access-update and diagnostic capabilities only
when implemented. Check image readiness separately. Historical `ssh_keys` and
metadata rows are not proof of installed support.

Implement the common model/adapters for both VM engines, live key updates and
retained-VM onboarding, then independent diagnostics. Acceptance includes:

1. QEMU and Firecracker on file/ZFS: inject, log in with real OpenSSH, rotate/remove/
   expire a key and cold-reconnect with the same disk and host key, without OMM.
2. Incompatible images, locked login accounts, invalid keys and unavailable sshd
   produce preparation/install errors.
3. Lost controller/agent replies reconcile persisted key/access updates without
   duplicate mappings or reporting unapplied keys as usable.
4. Host-controlled recovery of a hung guest has bounded escalation, no overlapping
   VMMs and retained state. Host outage stays distinct from guest failure.
5. Real exit/OOM evidence yields accurate attribution; absent evidence yields an
   unknown cause. Verify bounded capture and redaction.

Return to the [documentation index](../README.md).
