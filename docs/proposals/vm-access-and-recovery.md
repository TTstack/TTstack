# Native VM SSH and recovery evidence

Status: Initial SSH implemented and [validated](../validation/expert-ssh-2026-09-28.md)
on Linux QEMU and prepared Firecracker guests. The maintained [SSH contract](../ssh.md)
owns current behavior. Additional recovery diagnostics remain proposed.
This document retains the original rationale and outstanding proposals; it is not
a second maintained behavior guide. The implementation-gap observations below
describe the preimplementation baseline.

## Preimplementation baseline

At `c8101e9`, VM creation/options already contained `ssh_keys` and TCP ports, but
validation rejected SSH keys for Firecracker. QEMU injected root keys with cloud-init;
Firecracker ignored its SSH-key argument. `port_map` did not supply an SSH user,
host identity or a verified public endpoint. See the caller's
[source and network inspection](https://github.com/openmathmodel/omm-workspace/blob/main/docs/test-reports/2026-09-28-ssh-design-inspection.md).

## Provisioning decision, now implemented

The following requirements record the original design decision; consult the
[SSH guide](../ssh.md) for the implemented options and image requirements.

Accept an initial public key and login account at creation, and return SSH
connection information with VM metadata. Apply the same contract to QEMU,
Firecracker and future VM engines; bootstrap details belong to the image/adapter.
OMM supplies account `user`, with unrestricted passwordless sudo inside its guest.
This is full guest administration, not a protected-service or limited-sudo design.
TTstack does not implement OMM frontend approval, package policy or user-key custody.

The caller retains the downloadable login private key; TTstack receives only its
public part. Prepare the ordinary account, its authorized key and independently
started sshd. Generate a unique host key per VM. Persist bootstrap completion with
the retained disk: normal start/recovery must not rewrite user SSH configuration
or restore an initial key the user removed. There is no managed key-update,
rotation, expiry, theft-detection or reset subsystem.

Return the login user, reachable mapped address/port, initial public host identity
and SSH service readiness. Readiness does not guarantee that a user-modified key
or configuration still permits login. Report missing image support honestly.
Container images need their own preparation; ordinary VM support must work on
both QEMU and Firecracker.

## Networking rationale and caller boundaries

The inspected Linux DNAT ran in the agent's network namespace. The caller's agent
used a private namespace, so `Host.addr` plus `port_map` was not a public endpoint.
Configured resource-host ingress and forwarding for guest SSH were subsequently
implemented; current options and boundaries belong in the [SSH guide](../ssh.md#reachable-endpoint).

Guest sudo does not confer host, agent or controller authority. Keep hypervisor,
network and resource isolation outside the guest. Do not add in-guest OMM service
isolation or restricted package installation for this expert-access feature.

Application-down SSH repair and App-independent SSH activity remain caller
lifecycle requirements, with missing observations recorded in the inspection.
Users with sudo can stop sshd, OMM or guest networking; TTstack does not promise
to prevent or repair those changes.

## Remaining recovery proposal and acceptance criteria

Host-controlled stop/start and the SSH acceptance cases now have dated evidence.
Additional exit/OOM diagnostic observations described here remain proposals.

Reuse host-controlled stop/start, including bounded escalation for a hung guest.
Confirm the owned VMM has exited before restarting it; preserve the disk and user
configuration. Provide bounded exit/OOM/reachability evidence with timestamps.
Workspace owns automatic-recovery policy and user-facing reports, not TTstack.

Verify ordinary-user SSH/SCP and guest `sudo -n` on QEMU and Firecracker, stable
initial login after restart, and preservation of manual key changes. Verify host
and peer-VM isolation and honest metadata. Recovery may restore a stopped runtime;
it is not rollback of arbitrary changes made by a guest administrator.

Return to the [documentation index](../README.md).
