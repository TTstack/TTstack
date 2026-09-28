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
