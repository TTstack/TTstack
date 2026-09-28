# Version 0.5.1 upgrade validation — 2026-09-28

Scope: the existing Linux Firecracker/ZFS installation, upgrading agent and
controller from `2e4619f525d06ac97a220a7c58cfb3ccde45ce0d` (0.5.0) to
`05c5a432e5647245058a5c74d9fe5cb7064a59d0` (0.5.1). The user authorized analysis
and deployment on gateway `103.66.181.255` and resource host `103.244.113.119`.

## Compatibility analysis

The complete delta consists of documentation/site updates, two Rust doc-comment
edits, and workspace version changes in Cargo.toml/Cargo.lock. External dependency
versions, executable Rust logic, API models, capability gates and database code
are unchanged. Agent schema remains **5**, controller schema remains **4**. The
native-state reset used in an earlier transition is not applicable to this patch.

Both running databases were inspected read-only. Their schema markers and SQL
schema digests are unchanged after deployment, and `PRAGMA quick_check` returned
`ok` before and after. The same host, environment and VM records remain.

## Build and deployment

The clean checkout was built on the gateway with Rust 1.95.0, limiting Cargo to
eight jobs on its 32 logical CPUs. `cargo fmt --all -- --check` and
`cargo test --workspace --locked` passed: 179 tests, zero failures or ignored
cases. Configuration-drive tools were present. Locked release builds of the two
installed packages passed and both `--version` outputs are 0.5.1. No dependency or
MSRV changes were made; the MSRV gate was not repeated for this version-only delta.

OMM Deploy owns this installation. Its plan changed only the two executable
files; API-key files, units, namespace/network settings, images and paths stayed
unchanged. Agent `KillMode=process` and the separate Firecracker cgroup were
inspected. The deployment restarted agent and controller normally through
systemd; shutdown logs show their SIGTERM/graceful-shutdown paths.

| Binary | Installed SHA-256 |
| --- | --- |
| tt-agent | `7bb5a9d8e5cb1a21f3b1a7878911421d95dca072da08c2253ef0ad9a94ead053` |
| tt-ctl | `f0bf2c1ba653e1acc6032b7adfb267add4bdfef178dc4bf3a70622ae87d64c9e` |

The actual running `/proc/<MainPID>/exe` version and digest were checked, not only
files on disk. Both services were active/healthy with no pending recovery or
managed-file drift. Authenticated API reads passed; unauthenticated API requests
still returned 401. A final fleet check found all eight inventory services healthy.

## Retained workload and observed interruption

There was one pre-existing running Firecracker VM. Its VM/environment IDs,
4-vCPU and 8-GiB guest memory allocation, disk reservation, IP, port mappings and
ZFS dataset GUIDs/volume sizes remain unchanged. The original disk was not
recreated. Agent capabilities and resource reservations matched the baseline.
The public SSH endpoint was reachable, and its actual Ed25519 host key matched
the pre-upgrade database's public host identity. No guest login/private key was
used; this is SSH endpoint/identity evidence, not application or user-session
acceptance.

**This run does not demonstrate uninterrupted VM uptime.** The strict
same-Firecracker-PID check failed, and that failure was retained rather than
reported as a pass. The caller's Workspace log showed this sequence (UTC):

| Time | Observation |
| --- | --- |
| 18:09:16 | Agent management process restarted |
| 18:09:17 | Controller restarted; Workspace ready → unavailable |
| 18:09:18 | Workspace unavailable → ready |
| 18:09:21 | Workspace ready → stopping |
| 18:09:26 | Workspace stopping → stopped |
| 18:11:21 | Workspace stopped → starting |
| 18:11:22 | A new Firecracker process started |
| 18:11:24 | Workspace starting → ready |

Firecracker PID/start ticks changed from `704692 / 35068438` to
`708721 / 35176341`. Management-service restarts completed before the caller's
stop/start sequence. Source inspection identifies Workspace authority, client
revocation and idle policy as possible stop paths; the installed idle threshold
is 900 seconds. Existing logs do not identify which trigger occurred, and cannot
establish that the later stop was independent of the management interruption.
No VM stop/start/delete request was issued by this validation task. The caller
policy was not changed. Guest shutdown type and preservation of guest RAM were
not verified; only retained disk/state and recovered availability are established.

## Backup, cleanup and limits

Before deployment, private online SQLite backups and pre/post projections were
saved on their respective hosts under
`/var/lib/omm-tt/upgrade-backups/0.5.1-20260928` (directory 0700, files 0600).
The resource host also retains the scoped recursive snapshot
`ttvm/zruntime@ttstack-0.5.1-20260928`. This is online/crash-consistent protection,
not a coordinated application-consistent backup of a quiesced guest. No backup
was restored. Binary rollback remains available through OMM Deploy; it must not
be confused with restoring old metadata over live disks.

Temporary inspection scripts were removed. No test environment, guest, namespace,
image or firewall rule was created. Build receipts, ordinary build artifacts and
recovery backups are intentionally retained. No keys, guest configuration,
private inventory or database copies are included in this report.

New VM creation/deletion, cold-start application checks, QEMU, containers and host
reboots were not exercised. Deployment-specific evidence is linked from
[OMM Deploy](https://github.com/openmathmodel/omm-deploy/blob/main/docs/test-reports/2026-09-28-ttstack-0.5.1-upgrade.md).
