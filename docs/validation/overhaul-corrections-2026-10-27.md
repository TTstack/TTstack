# Overhaul correction validation — 2026-10-27

Functionally verifies a bounded selection of the full-repository overhaul
repairs on two authorized hosts. This describes the tested revisions and their
limits; it is not evidence for any other revision, host or engine combination.

## Revision and hosts

- Sources under test: `4c8c1d4` for the initial Linux cases and the FreeBSD
  native tests; `d026cf5` for the controller rebuild behind the mixed-failure
  scheduler re-check. The two revisions differ only in
  `crates/ctl/src/scheduler.rs` and the audit registry.
- Linux host: the Linux test machine itself, table cases nested inside its
  existing test network namespace for guest traffic and on loopback-only
  service ports outside it; guest budget 1 vCPU / 256 MiB / 1 GiB disk.
  Host load and memory stayed far below the half-host budget throughout.
- FreeBSD host: the dedicated test jail on the FreeBSD 15.1-RELEASE-p3
  machine, using the operator-provided passwordless sudo in that jail.

## Cases and observations

- **Isolated stack**: one root `tt-agent` (file storage, 8 CPU/8192 MiB/40 GiB
  budgets, ports 21000–21999) and one `tt-ctl` on loopback-only test ports
  with a throwaway API key. Startup capability probing advertised QEMU fully
  and reported Firecracker unavailable, matching the documented probe
  behavior. Registration and an immediate repeat registration were
  idempotent (201 then 200).
- **Guest-config admission**: a Firecracker environment request with 32
  caller files plus an SSH key was rejected up front with the new admission
  error (`guest_config allows at most 31 files when SSH bootstrap adds its
  seed`) and left no environment record (`/api/envs` empty). Without the
  seed, 32 files remain valid per unit coverage.
- **Role-less deployment**: a deploy configuration with neither controller
  nor agents fails before any remote work with
  `deploy config needs at least one controller or agent role`.
- **QEMU lifecycle**: the pinned `alpine-cloud` recipe downloaded and
  SHA-512-verified (164.1 MB). Create succeeded; guest SSH became ready and
  accepted the injected key (real sshd handshake, not just a live VMM).
  A marker file written in the guest survived a normal stop (orderly
  shutdown, roughly two seconds — no forced termination was observed) and a
  cold start. Delete removed the VM record, runtime clone, TAP device and
  the port-22 DNAT entry; the runtime directory was empty afterwards.
- **Mixed image/capacity diagnosis (before/after)**: with a fit host lacking
  the image and an image-bearing host limited to a 200 MiB disk budget, the
  pre-fix controller answered `no host has image 'alpine-cloud'` although
  the fleet had the image — registered as a scheduler finding. The `d026cf5`
  controller answers
  `no host has enough resources for engine=qemu, cpu=1, mem=256MB, disk=1024MB`
  and creates no records or reservations.
- **FreeBSD native tests**: `cargo test --locked -p ttcore net` under Rust
  1.88.0 in the jail passed all 13 tests, including the platform-module
  ownership and bridge-type regressions added by this overhaul
  (`tap_ownership_requires_the_exact_interface_description`,
  `foreign_interfaces_named_tt0_are_refused_before_configuration`) and the
  PF preflight/anchor tests.
- **FreeBSD output parity**: a throwaway bridge interface created and
  destroyed in the jail printed a bare `\tgroups: bridge` line, matching the
  classifier added by the bridge-type check.

## Cleanup

All task-owned material was removed after the runs: guests, agent/controller
processes, state and runtime directories, downloaded images, the throwaway
key material, the agent run directory under `/home/ttstack`, and the FreeBSD
build tree, toolchain and build-only packages installed for the native run.
No pre-existing service, guest, bridge, firewall rule or package was
modified; checks afterwards found no leftover `tt0`, TAP, nft/PF rules or
processes on either host.

## Limits and untested scope

- Linux ran QEMU and the HTTP surfaces only. Firecracker and Docker were
  absent/unused on the Linux host; no Firecracker, bhyve, Jail, backup,
  resize, SSH-ingress or expiry behavior was exercised live. Those repairs
  are covered by the new unit regressions instead.
- The FreeBSD run covered compilation and pure networking predicates plus
  one manual bridge probe; no bhyve/Jail guest or PF lifecycle was run.
- Error-retention, detach and resize-status changes rely on fault injection
  that is only available in unit tests; they were not reproduced live.
- The local-write half of the atomic key-file repair was unit-tested; the
  remote deployment snippet was reviewed and shell-verified in shape but a
  full redeploy against a live service was not performed.
