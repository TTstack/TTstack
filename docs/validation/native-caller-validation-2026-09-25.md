# Native caller lifecycle validation — 2026-09-25

## Revision and scope

Controller and agent revision: `0a9761a`, based on upstream `b28e7c7`.
The change replaces fixed `chunks_exact(8)` iteration with `as_chunks::<8>()`
without changing stable TAP-name hashing. Rust 1.98.1 Clippy, formatting,
154 workspace tests, focused network tests, the Rust 1.88 check and release
build passed before deployment.

The authorized installation separates the gateway/controller from a dedicated
Linux resource host. The resource host has 32 CPUs and about 186 GiB RAM. It uses
Firecracker 1.17.0, guest kernel 6.1.186, jailer isolation and ZFS pool `ttvm` on
the dedicated disk. The agent's advertised capabilities include Firecracker zvol,
disk resize, guest configuration and isolated networking.

## Cases and observations

A real application caller requested two independent environments, each with
2 vCPU, 4 GiB RAM and an 8 GiB persistent root volume. The immutable base image
was already installed. ZFS showed separate `clone-<vm-id>/rootfs` volumes for
both guests, not file-backed fallback disks.

- Caller application readiness was checked after VM process startup.
- Real native clients executed shell and model work inside the assigned guests.
  Another account did not see the first guest's marker files.
- Idle stop retained the guest UUID and cloned zvol. Cold start restored access
  to the same application files and session history; VM memory was not retained.
- A caller service restart and service rename retained both environment mappings.
- Explicit application sign-out caused VM stop while retaining the root volume.
- One client releasing its connection did not stop another client's bounded task
  in the same VM. Client authorization and lease policy remained caller concerns.

The host reported load below 1 and roughly 3.6 GiB used host memory during the
checks. This is functional evidence within the authorized 50% budget, not a
capacity or throughput benchmark.

## Cleanup

Both task-owned environments were deleted through the controller after stop.
Their cloned zvols disappeared; only the immutable base root volume remained.
The controller and agent stayed active. No other guest, shared network namespace,
firewall rule or image was removed.

## Limits

This run does not revalidate QEMU, containers, file-backed Firecracker, image
migration, host reboot or storage backup/restore. The application caller's
identity and UI acceptance report lives in its own repository; TTstack does not
acquire application identity, installation or business-policy responsibilities.

Return to the [documentation index](../README.md).
