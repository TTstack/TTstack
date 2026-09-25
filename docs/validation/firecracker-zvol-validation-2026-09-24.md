# Firecracker zvol validation — 2026-09-24

Implementation tested: `9a907726fc3c4faf890d146ee38188f072b35f0a`.
This extends the earlier [ZFS file-dataset restart check](firecracker-restart-validation-2026-09-24.md)
with actual block-device roots. The maintained layouts and allocation policy are
in the [guest storage guide](../guest-images.md#storage).

## Environment and isolation

- Authorized dedicated Ubuntu 24.04 x86_64 VM host, Linux 6.8.0-142-generic,
  32 logical CPUs, approximately 186 GiB RAM, KVM and cgroup v2.
- Firecracker/jailer 1.17.0, guest kernel 6.1.186 and ZFS 2.2.2-0ubuntu9.5. An existing dedicated ZFS
  pool on a separate 3.84 TB disk was reused; no disk was formatted for this test.
- A task-owned dataset subtree held a 128 MiB BusyBox/ext4 fixture, kernels,
  clone volumes and file-backend comparison images. Temporary agents/controller
  ran in a separate network namespace with loopback-only administrative APIs,
  private credentials and separate SQLite state. No host firewall was reset.
- One guest at a time: fixture guests used 1 vCPU and 128 MiB RAM; the later
  application check used 2 vCPU, 4 GiB RAM and an 8 GiB disk. The host remained
  well below the authorized approximate 50% CPU/memory budget. No throughput,
  saturation, pool exhaustion or maximum-size benchmark was run.

The [upstream jailer contract](https://github.com/firecracker-microvm/firecracker/blob/v1.17.0/docs/jailer.md)
requires resources to be accessible within the jail after privilege dropping.
This implementation creates only the assigned root block-device node there,
without changing ownership of the host zvol device or exposing host `/dev`.
Feasibility was established by booting that configuration, not inferred from
ordinary file-backed Firecracker support.

## Results

| Check | Observation |
| --- | --- |
| Direct jailed zvol feasibility | Two real boots as non-root UID 100999; boot counter and clean-shutdown marker persisted |
| Controller storage preference | With both file and ZFS agents offering the fixture, allocation chose ZFS despite the file host having less free memory |
| Root disk identity | The jail root disk was a block node with the clone's actual device number, mode 0600 and the VM UID; the host device remained root-owned |
| Clone isolation and sizing | A 128 MiB base stayed unchanged; the clone and its ext4 filesystem grew to 192 MiB |
| Configuration drive | Exact marker was readable; guest writes to the attached configuration filesystem failed |
| Accounting | 192 MiB root plus 4 MiB config reserved 196 MiB; stop released CPU/RAM and retained disk capacity |
| Stop/start | Guest reached boot counters 1, 2 and 3 with retained data and orderly-shutdown markers |
| Agent restart with running guest | Guest remained at boot count 2 and continued serving requests |
| Agent restart with stopped guest | The same VM started again with its retained disk |
| File allocation fallback | Removing the ZFS candidate selected the file agent; its create, stop/start and deletion also passed |
| Shrink rejection | A 64 MiB request against the 128 MiB base created neither a VM record nor a clone |
| Invalid kernel | Launch failed after provisioning; failed state stayed visible and explicit/repeated deletion removed the clone and jail |
| Partial provisioning failure | A conflicting root-disk reference failed during preparation; deletion removed both dataset and volume, releasing reservations |
| Final isolated accounting | Both test agents returned to zero VMs and zero CPU, memory and disk reservations |

## Dedicated deployment and application check

The resource agent was switched from file storage to explicitly provisioned ZFS
image/runtime roots only after confirming it owned no user VMs. The original
file image was retained. Importing its 8 GiB ext4 contents and kernel into the
new base passed byte-for-byte SHA-256 verification against the source manifest.
The controller was upgraded with ZFS preference and the agent advertises
`firecracker_zvol`. Dataset mount guards remain required at service startup.

A task-owned guest using the real preinstalled OMM Code `b89040ba` image booted
from a zvol. Its authenticated HTTP service created a durable session and executed
a shell command writing a marker in its project directory. No model credentials
were needed or supplied for this storage check.

The guest was stopped, an offline recursive snapshot was taken, and the dedicated
host was cleanly rebooted. After ZFS mounting, the management link and agent
recovered, the same VM cold-started, and OMM's original session and project-file
marker were readable. Deletion removed the runtime dataset, volume and test
snapshot. This validates cold boot and persistence, not VM memory restoration.

## Cleanup and limits

Task-owned VM records, clones, jails, snapshots, transient services, test namespace,
fixture datasets and private test credentials were removed. The dedicated host
retains only the intended production image/runtime roots, the original file image
and persistent operator state. Its agent and the gateway's controller, RMC, proxy
and MCP services were checked after validation.

136 workspace tests passed, including meaningful regression cases for file-root
inode preservation, rejected unsafe image members, ZFS preference, file fallback,
legacy-agent capability checks and Docker's unchanged placement policy. Formatting,
all-target Clippy with warnings denied, Rust 1.88 locked checks and a locked release
build passed. The dependency feature change requires no lockfile version changes.

This run did not boot QEMU on a zvol, test sudden power loss, pool failure,
application-consistent online snapshots, backup restoration, migration of an
existing file-backed VM, or high concurrency. QEMU's existing raw-volume layout
remains supported but its guest execution is not newly validated here. The OMM
application check used explicit native-key development mode without a provider
key: it validates server boot, shell, session and disk persistence, not SAIR
sign-in, model inference, or a native client's Local/Remote selector.
