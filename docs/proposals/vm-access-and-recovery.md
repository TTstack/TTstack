# Native VM SSH and recovery evidence

Status: Initial SSH implemented and [validated](../validation/expert-ssh-2026-09-28.md)
on Linux QEMU and prepared Firecracker guests through the API, CLI, and agent.
The dashboard does not expose the full account/sudo/Firecracker SSH contract.
The maintained [SSH contract](../ssh.md) owns current behavior. Additional recovery
diagnostics and the lifecycle refinements below remain proposed.
The [review](audit.md) rechecked that split.
This document retains the original rationale and outstanding proposals; it is not
a second maintained behavior guide. The implementation-gap observations below
describe the preimplementation baseline.

Design follow-up: 2026-09-29, source checked at `df3281f`; no new guest experiment
or live validation. The following recommendations do not change current behavior.

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
configuration. Provide bounded observations with an explicit evidence source and
unknown/unavailable values, as specified below; do not infer exit cause from elapsed
time or promise an engine-independent OOM history that the current code cannot read.
Workspace owns automatic-recovery policy and user-facing reports, not TTstack.

Verify ordinary-user SSH/SCP and guest `sudo -n` on QEMU and Firecracker, stable
initial login after restart, and preservation of manual key changes. Verify host
and peer-VM isolation and honest metadata. Recovery may restore a stopped runtime;
it is not rollback of arbitrary changes made by a guest administrator.

### Shutdown behavior and evidence

The inspected [QEMU stop path](../../crates/core/src/engine/qemu.rs) sends
`system_powerdown` and polls for about 10 seconds. The
[Firecracker stop path](../../crates/core/src/engine/firecracker.rs) resumes a
paused guest first, requests `SendCtrlAltDel` on x86_64, and can wait 30 seconds.
Both eventually use the shared termination helper, with up to 5 seconds each for
SIGTERM and SIGKILL. Docker/Podman's `stop -t 10` targets container PID 1 and has
different semantics.

A 30-second QEMU guest window is a reasonable candidate, not an agreed or proven
requirement. The [10.062-second stop observation](../validation/lifecycle-corrections-2026-09-28.md)
could not distinguish a late guest exit from forced termination. It establishes
an evidence gap, not the correct replacement timeout. Increasing the guest window
does not prove guest data was flushed, and currently lengthens the host runtime
mutex hold for a slow stop. Keep this separate from disk-backup implementation.

For a dedicated shutdown change, recommend:

- Add observable request/escalation/exit phases and validate representative guest
  behavior before choosing a new QEMU window. Preserve immediate return when the
  owned VMM exits; do not expand the shared termination waits or Docker timeout.
- Handle paused QEMU explicitly: attempt a verified resume before requesting
  guest shutdown, or report forced termination if resume is unavailable. Test the
  paused state and resume failure; sending only `system_powerdown` is insufficient
  to establish that a paused guest could process it.
- Record whether escalation was attempted and whether process exit was confirmed.
  Call an exit before escalation an observation, not proof of a graceful guest
  shutdown: another actor or an OOM killer may have stopped the process.
- Keep lifecycle cleanup gated on confirmed process identity/exit. A diagnostic
  read failure may make the cause unknown but must not make a live disk deletable.

### Sources for recovery diagnostics

Prefer a bounded latest lifecycle observation per VM/run to a new event-history
service. Record source, VM/run identity, observation time, known value, and an
explicit unknown/unavailable reason. Retain useful observations across service
restart without pretending to reconstruct unobserved events. Host observation
timestamps do not necessarily equal the guest's actual failure time.

| Observation | Available source or required addition | Interpretation limit |
| --- | --- | --- |
| Shutdown request and escalation | Instrument the owned engine stop/terminate path. | A signal/request sent is not proof of guest handling or data flushing. |
| Process exit | Existing process identity checks; child wait status only while the agent actually owns and reaps that process. | QEMU's daemonizing launcher status is not the guest VMM's later exit status; absence after restart supplies no historical exit code. |
| Firecracker OOM evidence | Sample the owned per-VM cgroup's `memory.events` counters before removing/recreating it, with a baseline for that run. | Counter deltas concern host processes in that cgroup, not a guest application's OOM; a missing baseline or recreated cgroup makes attribution unknown. |
| QEMU OOM evidence | No dedicated TTstack per-VM cgroup currently exists. | Report unavailable until a separately reviewed per-VM source exists; do not use host/service-wide counters as VM attribution. |
| Container exit/OOM evidence | If included, qualify the bound Docker/Podman runtime's inspect fields for the exact container/run. | Do not assume equal runtime semantics or add container diagnostics to a VM-only change implicitly. |
| SSH reachability | Existing banner/key observations and `ssh.checked_at`. | Separate from process readiness, exit cause, external routing, and application health. |

Linux [cgroup v2 documentation](https://docs.kernel.org/admin-guide/cgroup-v2.html)
defines `oom` and `oom_kill` as different counters; `memory.events` can include
descendant events. Choose and record the appropriate scope rather than treating a
nonzero counter as proof that this VMM just died from OOM. Do not infer OOM from
SIGKILL alone. A missing source is not a zero count. Preserve these limitations in
API/CLI output; no automatic restart policy, host log harvesting, or metrics
subsystem is required.

### Disk restore and SSH initialization

Ordinary cold restart retains disk initialization state. Disk restore can rewind
the guest marker, keys, and cloud-init records while retaining the host seed and
the agent's later observations. These operations must not share an unconditional
"already initialized" assumption.

The maintained disk-backup guide owns the implemented
[restore and SSH observation policy](../disk-backup.md#ssh-observations-after-restore),
including conditional seed dispatch and invalidation of `ready`, `checked_at`,
and `initialized`. The backup proposal retains the design rationale; ordinary
restart behavior remains distinct.
Guest key adoption/rotation and dashboard SSH controls remain separate work.

### Additional acceptance cases

- Cooperative shutdown returns early; a non-cooperative guest reaches the bounded
  escalation path; paused QEMU and failed resume have explicit observed outcomes.
- An uncertain/failed identity probe retains the runtime and disk. Missing exit
  status after agent restart remains unknown, never a fabricated successful exit.
- Cgroup recreation or missing OOM baselines cannot attribute another run's event
  to the current VM. Use fixtures first; any live OOM test needs separate bounded
  authorization and must not exhaust host memory.
- UI omission is documented accurately; successful API/CLI SSH tests do not imply
  Firecracker account/sudo controls exist in the dashboard.
- Disk restore covers the linked proposal's marker/dispatch/identity cases, and
  ordinary stop/start still preserves deliberate guest SSH changes.

Return to the [documentation index](../README.md).
