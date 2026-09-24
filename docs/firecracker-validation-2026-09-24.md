# Firecracker configuration, shutdown and isolation validation — 2026-09-24

This record accompanies the change adding generic guest configuration, jailed
Firecracker execution, orderly shutdown and opt-in Linux network isolation.
It supplements the [earlier lifecycle validation](live-validation-2026-09-24.md);
it is a functional check, not a performance or security certification.

## Environment and load limits

- One authorized Ubuntu 24.04 x86_64 host with KVM and cgroup v2. The other two
  available hosts were inspected for availability but ran no test workloads.
- Firecracker and jailer 1.17.0 were downloaded into a temporary directory; no
  host package or permanent service installation was performed.
- Networking ran in a dedicated network namespace with its own bridge, TAP
  devices, NAT and isolation rules. Management used an SSH tunnel and a temporary
  API key. No public management listener was added to the host namespace.
- At most two guests ran simultaneously, each with one vCPU and 128 MiB guest RAM.
  Each VMM had a one-core CPU ceiling and a 256 MiB memory ceiling, including
  128 MiB headroom. Guests only served tiny HTTP responses and performed a few
  connectivity checks; no CPU loops, throughput tests or memory stress were used.
- The agent was limited to half one core and 256 MiB. Its VMM cgroups were separate
  and verified individually. An external test HTTP endpoint used a 10% CPU quota.
- The guest fixture extended a newly built `fc-alpine` with BusyBox's HTTP applet,
  a boot counter, a shutdown marker and a small connectivity probe. Application
  files existed only in the test fixture, not the generic image recipe.
- Binaries were compiled locally. Initial binary/image downloads were bandwidth
  limited; small Alpine packages were installed only inside the test guest image.

## Results

| Check | Result |
|---|---|
| Controller creates two isolated Firecracker guests with configuration files | Passed; both became running and their guest HTTP endpoints became ready |
| Configuration confidentiality | Responses contained only SHA-256 digests; no configuration text was returned |
| Guest configuration contract | Exact file content available from `/dev/vdb`; guest writes failed on the read-only mount |
| Normal shutdown | Both guests executed their shutdown hooks and persisted a clean marker; stopping the pair took about 7 seconds including API/SSH overhead |
| Disk persistence | After cold start, boot counters advanced from 1 to 2 and the previous shutdown marker remained present |
| Resource accounting | Stopped guests reserved zero CPU/RAM and retained 264 MiB disk total: two 128 MiB rootfs disks plus two 4 MiB config drives |
| Published ports | Host-side forwarded TCP ports served the correct guest responses |
| Routed egress | Guest reached a controlled HTTP endpoint outside the guest network namespace |
| Host access | Guest could not reach the agent via either the bridge address or the namespace's external interface address |
| Peer access | Guest could not reach the second isolated guest's HTTP service |
| Jailer | Distinct non-root UIDs 100002 and 100003; `/proc/PID/root` inode matched each sandbox root; seccomp filtering was active |
| Kernel resource ceilings | Per-VMM `cpu.max`, `memory.max` and cgroup placement matched the requested limits |
| Agent restart | Existing guests remained running, boot counters stayed at 2, and forwarding worked after firewall recovery |
| Controller restart | Existing environment/VM identity survived reopening the same database |
| Repeated create | Same request reused the existing VM; changed configuration was rejected without a second allocation |
| Invalid configuration | Path traversal rejected before allocation; validation errors did not echo configuration contents |
| CLI | `--guest-config`, `--isolated-network` and `--lifetime 0` created a reachable guest |
| Non-cooperating init | A separate guest without Ctrl-Alt-Del handling was forcibly stopped after about 30.5 seconds; fallback was logged and resources released |
| Deletion | Repeated deletion succeeded; tracked VM count and all reservations returned to zero |

## Test feedback and cleanup

The first test service used `ip netns exec`, which remounted sysfs and hid the
cgroup v2 mount. TTstack refused the launch and retained a cleanable failed record.
Deletion removed those partial resources. The test service then used `nsenter
--net=...`, preserving host cgroup visibility. This was a test-environment issue;
TTstack does not downgrade to an unjailed launch when prerequisites are missing.

Inspection confirmed no test VMM, clone disks, per-VM jails or isolation tables
remained after deletion. Temporary services, network namespace, veth devices,
downloaded binaries, images and credentials were removed. The local controller and
SSH tunnel were stopped. Existing host workloads were left in place.

## Automated checks and limits

124 workspace tests passed, including actual ext4 config-disk inspection without
root, configuration validation, capability-aware scheduling and memory headroom.
Formatting, Clippy with all targets/warnings denied, Rust 1.88 all-target checks and
a locked release build passed. The CLI also rejected an oversized config file
before making a network request.

The final review additionally bounded the CLI's encoded input and checked disk
size arithmetic for overflow; these changes passed the automated checks. The
remote runs exercised the same disk layout, jailer, shutdown and firewall paths.

This run did not reboot the physical host, test a legacy unjailed VMM upgrade,
exercise QEMU with the new isolation option, or perform adversarial packet fuzzing.
It did not verify public Internet connectivity, IPv6 applications, ARM, other host platforms,
ZFS, high concurrency or maximum capacity. MAC/IP/ARP and IPv6 filtering rules were
installed successfully but were not exhaustively attacked. Use host firewalls and
application authentication to restrict published ports; guest isolation is not a
complete tenant identity or authorization system.
