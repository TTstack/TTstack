# Native VM SSH and recovery evidence

Status: Proposed additions; not implemented or validated. Current [API](../rest-api.md)
and [guest image](../guest-images.md) guides describe shipped behavior.

## Inspected implementation

At `c8101e9`, `VmSpec.ssh_keys` and `VmOptions.ssh_keys` already carry public keys,
but `validate_vm_options` rejects them for Firecracker. QEMU's
`generate_seed_iso` uses first-boot cloud-init; Firecracker's `create` ignores its
SSH-key argument. VM records expose `port_map`, not an SSH login descriptor, host
key or authenticated-session activity. Controller/agent routes offer resource
updates, but no key or general mapped-port update operation. See the caller's
[inspection record](https://github.com/openmathmodel/omm-workspace/blob/main/docs/test-reports/2026-09-28-ssh-design-inspection.md).

## One SSH contract for every VM engine

A caller supplies an initial public key at VM creation and receives SSH login
metadata with the VM description. OMM Workspace generates the pair and delivers
the private part to its owner; TTstack receives only the public part. QEMU, Firecracker and future VM engines
must support the same contract. Image preparation and key injection belong to the
engine adapter. Use OpenSSH; an unsupported image reports a preparation error.

TTstack owns the VM, key provisioning, TCP mapping and observations. The caller
owns user authentication and VM ownership checks. OMM developer roles, approval
flows, temporary grants, application build adoption and Web terminals do not belong
in this contract. Existing private TTstack API authentication remains unchanged;
end users receive no host or TTstack administrator credentials.

## Provisioning and VM metadata

- Accept the caller's initial public key and explicit login user. OMM uses the
  ordinary `workspace` account; generic QEMU root injection is not the OMM access
  policy. Support non-root account bootstrap for both VM engines. Keep private keys with the
  caller; never return private material through VM metadata.
- Start guest sshd independently of its application. QEMU can use cloud-init;
  prepared Firecracker images consume generic SSH bootstrap data separately from
  opaque application configuration. Both must work with ordinary SSH/SCP/SFTP.
- Return guest SSH readiness, login user, externally reachable mapped address and
  port, initial public-key fingerprint and the VM's initial public host key. A running
  VMM alone is not proof that SSH is ready. Do not advertise an inaccessible guest
  address as the remote endpoint.
- Give each VM its own host key and preserve it with its retained disk. Never
  clone one host private key into every guest image.
- Install the initial key once and persist bootstrap completion with the VM/disk.
  Retrying initial creation must not generate another key or duplicate installation.
  Normal start and recovery must not rewrite the user's `~/.ssh/authorized_keys`, including
  when it is empty or missing. User edits are authoritative after bootstrap.
- No key-update/rotation endpoint, expiry policy, stolen-key detection, desired-key
  reconciliation or reset workflow is required. Users manage SSH keys inside their
  VM. SSH readiness does not assert that an initial key still authorizes login.

Current Linux `net::add_port_forward` creates DNAT in the agent's network
namespace; it does not publish a host-root-network endpoint. The inspected OMM
agent runs in a private namespace. The mapping contract must therefore include a
configured reachable ingress and its forwarding rules, not infer public access
from `Host.addr`. Expose only the guest's SSH mapping, never the entire shared
port pool (which also contains application ports). Keep management
ports and other guests isolated. Mappings must follow VM stop/delete and must not
be accidentally reused while the old runtime still owns them. A prepared retained
VM can receive one-time SSH preparation/mapping without replacing its disk;
existing user SSH configuration must not be overwritten. Otherwise report what
preparation is missing. No application-specific installer or general guest
command API is needed.

Authenticated SSH-session observation is an additional requirement: there is no
current agent API for it. Establish a small guest-sshd observation mechanism
independent of OMM and verify it before using it to prevent idle shutdown; packets to
a public port or an SSH handshake are not proof of an authenticated session.
Removing a public key blocks new logins, not existing sessions. Full runtime
revocation uses the caller's VM stop/isolation policy. For OMM, root login is
disabled; image policy protects system SSH/control configuration while the ordinary
user manages their own authorized keys. TTstack does not add a user-key lifecycle
manager or distribute host administrative privileges.

Docker/Podman images need their own prepared-container SSH support. Do not claim
that injecting a key into an arbitrary container starts sshd. This does not change
the common requirement for supported VM engines.

## Guest privilege boundary

OMM's image must separate its trusted connection/control runtime from the SSH
user and user jobs. TTstack provisions the requested ordinary account, but does
not implement OMM plugin execution policy or package-management permissions.

The current Firecracker jailer UID and `memory.max`/`pids.max` in
`crates/core/src/engine/firecracker/sandbox.rs` protect the host from the VMM.
They do not isolate the OMM process from another UID or workload inside the guest.
Caller images must provide protected ownership/supervision and in-guest budgets;
shared host limits alone cannot establish the requested connection availability.

## Recovery evidence

Reuse host-controlled stop/start: attempt graceful shutdown, then bounded
termination of the owned VMM if the guest is hung. Confirm the old runtime has
exited before starting another. Preserve disk, VM identity, allocation and SSH host
key. Agent unreachability is an unknown outcome, not a successful stop.

Provide a bounded diagnostic snapshot with observation time, agent reachability,
VM/VMM state, independent guest SSH reachability, last observed exit and matching
OOM evidence when available. Distinguish host/VMM OOM from guest-kernel OOM. A
network timeout alone establishes neither. Restrict raw host evidence to the
management caller, which prepares the user-facing report.

TTstack supplies observations and lifecycle primitives. Workspace owns desired
runtime state, automatic-recovery limits and account authority; there is no second
automatic restart policy in TTstack.

## Acceptance

On QEMU and Firecracker, verify non-root SSH/SCP with the published metadata, stable
initial login after restart and SSH access with the application stopped. Then
replace/remove the initial authorized key manually and confirm restart never
restores it. No private login key may appear in TTstack metadata or guest seeds. Root SSH must
be denied in OMM images. Check ownership isolation through the caller and authenticated-session
activity with the App absent. Verify host-controlled restart with both guest HTTP
and SSH unavailable, bounded diagnostics and correct exit/OOM/unknown-cause labels.

Return to the [documentation index](../README.md).
