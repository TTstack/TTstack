# Native SSH validation — 2026-09-28

The initial SSH implementation accompanying this report was tested on isolated
Linux QEMU and Firecracker guests with file storage. Public-IP login as `user`,
full guest sudo, SCP and cold stop/start passed on both engines. Firecracker also
preserved manual authorized-key replacement across restart without restoring the
removed initial key. Host identity was pinned during reconnection.

The [caller acceptance record](https://github.com/openmathmodel/omm-workspace/blob/main/docs/test-reports/2026-09-28-expert-ssh.md)
contains exact tested binary/image hashes, component baselines, test limits and
cleanup. Final local gates included fmt, Clippy, workspace tests, Rust 1.88 and
release build. Capability advertisement additionally requires ssh-keyscan.

No ZFS SSH, FreeBSD, arbitrary-image, automatic recovery or production rollout
claim follows from these tests. Existing guest disks and the parallel FreeBSD
worktree were not modified. The test VMs, isolated services, network namespace,
SSH ingress rules and remote staging directories were removed.

See the maintained [SSH contract](../ssh.md) and [documentation index](../README.md).

## Subsequent Firecracker/ZFS deployment

The [formal fleet report](https://github.com/openmathmodel/omm-deploy/blob/main/docs/test-reports/2026-09-28-expert-ssh-deployment.md)
records TTstack `e8980bf` deployed with matching Workspace and Code. Native SSH,
full guest sudo, SCP, private-management denial and retained key/file/host identity
through cold resume passed on the actual Firecracker/ZFS service. Its disposable
VM and ingress rule were deleted; existing guests were retained. This extends
Linux evidence only and does not validate the independent FreeBSD branch.
