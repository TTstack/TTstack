# Audit correction validation, 2026-09-25

This report records local regression checks and bounded Linux/Firecracker recovery
experiments for the [audit corrections](../audit.md). Maintained behavior is in the
[REST API](rest-api.md), [deployment](deployment.md) and [guest guide](guest-images.md).

## Tested source

The base revision was `6866295128bc03ec2ea1af64e4a9d8b11c77de38`, with uncommitted
corrections. SHA-256 of `git diff --binary HEAD -- Cargo.toml Cargo.lock crates`:

- Live-run source: `b92eed89254c699625047be8ac62af9c7b6684004c3c163a3b9716df975674e9`.
- Final locally checked source: `e9e0ded27d31b3ded2a5f2baf612b83835e13110ac872da65b0bcae26b7645ff`.

The final local revision adds the controller reservation guard for concurrent
restart/create and its regression, plus an adjustment to a startup test that
previously assumed `/proc/PID/cmdline` could never be temporarily empty. The live
experiment did not repeat after those changes. No host engine, storage or
networking implementation changed after the successful host tests.

## Local checks

The final tree passed:

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -j 4 -- -D warnings
cargo test --workspace --locked -j 4
cargo +1.88.0 check --workspace --all-targets --locked -j 4
cargo build --release --workspace --locked -j 4
git diff --check
```

Builds used a separate temporary `CARGO_TARGET_DIR`; the repository's pre-existing
target symlink was preserved. Workspace tests: **154 passed**, no failed/ignored
tests (CLI 19, agent 28, controller 43, core 64). Real ext4/configuration-drive tests
ran with e2fsprogs. The local environment did not have QEMU installed.
Additional release-binary checks confirmed that both services reject empty API
keys and a second controller cannot open an already locked state directory. The
temporary controller and its state were removed. All 106 local Markdown links
and anchors across 23 Markdown files were checked successfully.

Focused coverage includes unrelated operations during a stalled delete, discarded
stale snapshots, confirmed actions during concurrent refresh invalidation, pending
restart reservations, offline detach, image/config-drive placement accounting,
missing VM reservations, corrupt-row recovery with admission closed, late readiness,
normalized creation options, process discovery after PID corruption, fixed network
identifiers, private clones/config files, invalid credentials, deployment input
rejection, mount-guard preservation, exact checksum selection and rejection of
mismatched downloads without replacing an existing image.

The final full test run initially exposed a startup-test timing assumption: a live
new child can temporarily have no readable argv. The implementation correctly
retained resources; the old test unwrapped that deliberate error. The test now
accepts either an unavailable identity or a known mismatch, reaps the child, and
still verifies that boot waits for readiness without treating either as process
exit. The complete suite passed after that correction.

## Host setup and bounds

Both explicitly authorized hosts received read-only prerequisite/load inspection.
Only one hosted guests; the other received no test allocation. Each had 32 logical
CPUs and approximately 123 GiB RAM, with low starting load and approximately
3.3 GiB RAM in use. No pre-existing TTstack service was active; the test host's
Docker daemon had no running containers.

The experiment used private task directories, separate SQLite databases, transient
systemd services, a dedicated network namespace and veth pair, and an isolated
TTstack bridge/firewall. It did not install host packages, replace installed service
binaries, reset host networking, or change Docker's firewall policy. Management
credentials were held in private files and request headers, not printed or placed
in SSH arguments.

At most two guests ran together, each with **1 vCPU and 128 MiB RAM**, plus the
normal 128 MiB Firecracker VMM allowance. The agent was limited to half of one CPU
and 256 MiB; the controller to one quarter CPU and 128 MiB. Combined configured
limits were 2.75 CPUs and 896 MiB, well below the authorized 50% host ceiling and
leaving ample headroom for the existing load. No stress or throughput test ran.

Firecracker/jailer 1.17.0 binaries were extracted into the private task directory
from the official release archive after SHA-256 verification. The CLI built an
`fc-alpine` fixture using verified kernel/minirootfs downloads. Each guest reserved
128 MiB rootfs plus a 4 MiB read-only configuration disk, for 264 MiB total disk
reservation across both guests. A small BusyBox HTTP response exposed only a
non-secret configuration marker and a persistent boot counter.

## Observations

| Case | Observed result |
| --- | --- |
| Partially initialized bridge | A deliberately down/addressless `tt0` was repaired; both environments became active. |
| Image-sized disk accounting | Agent image catalog reported 128 MiB; controller and agent reserved 132 MiB per guest including configuration. |
| Published-port readiness | A client outside the network namespace reached both isolated guests through their assigned host ports and read the configuration marker. |
| Stop/start persistence | Both guests cold-started successfully and advanced a root-disk boot counter; memory persistence was not expected. |
| Agent restart with transient nft failure | Both original VMM PIDs survived. Reconcile retried the injected failure and cleared VM errors without guest restart. Published HTTP remained usable afterward. |
| Missing sandbox metadata | Stop returned an explicit error. The live VMM and root disk remained intact. Metadata was restored before further lifecycle work. |
| Corrupt PID file | The agent recovered the exact process identity, stopped the VMM and restarted from its retained disk. |
| Paused VMM with failed resume API | The real VMM was paused; an isolated curl wrapper rejected the resume request. Stop used forced termination and completed in about 0.1 seconds. The disk remained usable on the next cold start. |
| Final controller artifact | The updated controller restarted against its existing state and completed another stop/start of a retained environment. |
| Missing NAT table during delete | Only the namespace's TTstack NAT table was removed. Both deletes completed, released all reservations and removed guest processes, disks, metadata, TAPs, per-VM cgroups and isolation rules. |

Initial test iterations exposed two implementation mistakes before the successful
run: a sysfs-based interface check observed the original namespace rather than the
entered namespace, and `ip tuntap add` could not reopen an existing persistent TAP
because it requests exclusive creation. Interface inspection now uses `ip` in the
current namespace; stopped-TAP ownership uses the Linux TUN ioctl without deleting
the device. The affected create and repeated stop/start cases were rerun successfully.

The first HTTP fixture omitted proper request draining/response length, producing
an application-level connection reset after HTTP 200. Only stopped test clone disks
were mounted to correct the fixture; subsequent published-port probes verified
complete response bodies. This was not treated as a TTstack forwarding defect.

The captured jailer/VMM log contained kernel boot and guest console output. The
original audit assertion that it could not contain a guest console was therefore
removed. Forced termination is recorded separately above; an API stop success by
itself is not proof of a graceful application shutdown.

## Cleanup and limits

API inventory was empty and CPU, memory, disk and VM-count reservations were zero.
Checks confirmed absence of task VMM PIDs, clone disks, jail roots, PID/log/sandbox
files, TAPs, per-VM cgroups and isolation rules. Task services were stopped, the
namespace/veth removed, and private binaries/images/state/key files deleted.
Pre-existing TTstack services remained inactive and Docker still had zero running
containers. Final host load was low; no task guests or services were left running.

This run covers Linux x86_64 Firecracker with file storage and the tested isolated
network. It does not verify QEMU, Podman, zvol/ZFS, OpenRC, musl, production deploy,
arbitrary Docker/firewalld coexistence, conntrack reuse behavior, or every possible
partial host failure. Checksums from a rolling image's own HTTPS origin do not
provide independent signature verification. The local restart-reservation
regression is not presented as an additional live-host experiment.
