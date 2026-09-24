# Guest images and access

Prepare images on the **agent host that will run the guest**. `tt image create`
is a local operation and does not contact the controller or distribute images.
Agent-only deployment installs only `tt-agent`; copy a compatible `tt` binary to
that host if using the built-in recipes there. Examples below use the default
install paths and a CLI already configured for the controller.

## Recipes and image discovery

```bash
/opt/ttstack/bin/tt image recipes
sudo mkdir -p /home/ttstack/images
sudo /opt/ttstack/bin/tt image create alpine-cloud --image-dir /home/ttstack/images
```

Use one recipe for the selected workflow. `image create all --engine ENGINE`
filters a bulk build; `--engine` has no effect when a single recipe is named.
Unfiltered `all` attempts recipes for multiple engines and may fail if their tools
are not installed. It is not needed for deployment.

| Recipe | Engine | Source / purpose |
|---|---|---|
| `alpine-cloud` | QEMU | Alpine 3.21.7 x86_64 BIOS NoCloud qcow2 |
| `debian-cloud` | QEMU | Debian 13 generic amd64 daily/latest cloud image |
| `ubuntu-cloud` | QEMU | Ubuntu 24.04 amd64 current cloud image |
| `fc-alpine` | Firecracker | Prebuilt kernel + Alpine 3.21.3 userspace; 128 MiB ext4 rootfs, idle boot/network check |
| `alpine`, `debian`, `ubuntu`, `rockylinux` | Docker | Base OS images: `alpine:3.21`, `debian:trixie-slim`, `ubuntu:24.04`, `rockylinux:9-minimal` |
| `nginx`, `redis`, `postgres` | Docker | `nginx:alpine`, `redis:7-alpine`, `postgres:17-alpine` |

Upstream rolling URLs and container tags can change. These recipes are not a
content-pinned image lockfile. Existing QEMU files are left in place; Firecracker
creation skips an existing kernel/rootfs. Repeating those recipes does not upgrade
an existing image. Docker recipes pull their tag and add a local short-name alias.

`tt image list` reports file/zvol names cached from **online** agents. It does not
list Docker/Podman images, validate image bootability or guarantee all hosts have
the same image contents. Use distinct names for image revisions and prepare each
revision on the intended hosts.

## QEMU: full VMs with SSH

File storage expects a **bootable qcow2 disk image**, named directly under
`image_dir` (for example `/home/ttstack/images/alpine-cloud`). Built-in cloud images
use cloud-init. A custom image needs a BIOS bootloader, kernel, virtio disk/network
drivers and a compatible userspace; merely formatting an empty qcow2 disk is not
a bootable image.

```bash
/opt/ttstack/bin/tt env create demo --image alpine-cloud --engine qemu \
  --cpu 1 --mem 256 --disk 2048 --ssh-key ~/.ssh/id_ed25519.pub
/opt/ttstack/bin/tt env show demo
```

Port 22 is included automatically. Connect to the **agent host** at the mapped
host port shown by `env show`; do not assume the controller address or a fixed
port. `running` does not imply SSH has finished starting.

At boot TTstack attaches a NoCloud seed ISO containing root SSH public keys,
password-login-disabled SSH configuration, and static network settings. Use
`--ssh-key` repeatedly for multiple public keys. The seed is regenerated on boot,
but cloud-init modules generally apply initial configuration once per instance;
this is not a key rotation interface. TTstack does not generate a login password.

Custom images without cloud-init must configure their own credentials **and**
use TTstack's allocated IP/gateway. A seed ISO alone cannot configure such a guest.
The QEMU engine still needs `genisoimage` or `mkisofs` to build the seed.

`--disk` is the cloned disk's virtual size in MiB (default 40960). It may grow but
cannot shrink below the base image's virtual size. The guest must also grow its
partition/filesystem; increasing the virtual disk alone does not do that. Disk
reservations track virtual capacity, not physical bytes used by sparse files.

Stop requests guest shutdown, then terminates QEMU if necessary. Start boots the
preserved disk in a new process. Memory is not preserved; save guest work first.
See [lifecycle and recovery](rest-api.md#lifecycle-and-recovery).

## Docker / Podman: application containers

Container images live in the runtime's own image store. The agent uses Docker if
its binary is installed, otherwise Podman; use that same runtime/store to prepare
images. Rootless and root-owned image stores are distinct.

A minimal web-container workflow on a host with a working Docker runtime is:

```bash
sudo /opt/ttstack/bin/tt image create nginx
/opt/ttstack/bin/tt env create web --image nginx --engine docker \
  --cpu 1 --mem 128 --port 80
/opt/ttstack/bin/tt env show web
```

Open `http://AGENT_HOST:MAPPED_PORT/` using the displayed mapping. Registry image
references such as `nginx:alpine` can also be passed directly to `--image` without
a recipe. The runtime may pull a missing image during creation, so registry/network
failures can still make creation fail; Docker images are not checked by the
controller's file/zvol catalog.

Images must have a long-running default command. Plain OS images may exit
immediately. Service recipes are downloads, not service provisioning: for example,
PostgreSQL needs initialization settings that TTstack does not expose. TTstack
has no container environment-variable, command-override or volume-mount interface;
use a prepared workload image when those defaults are needed.

TTstack rejects disk sizing, `--deny-outgoing` and SSH key injection for Docker.
Port 22 is not automatic. To use SSH, the image must provide sshd and credentials,
and port 22 must be explicitly requested. Otherwise use the host runtime, for
example `sudo docker exec -it tt-VM_ID sh`, replacing `VM_ID` with the full ID from
`env show`. Stop/start retains the container filesystem; deletion removes it.

## Firecracker: prepared microVM workloads

With file storage, Firecracker uses an image directory with both files:

```text
/home/ttstack/images/fc-alpine/
├── vmlinux
└── rootfs.ext4
```

The ZFS backend stores the root filesystem in a zvol and the kernel in its parent
dataset; see [storage](#storage) for the layout and import steps.

The kernel must support Firecracker's devices, including built-in virtio-mmio and
root filesystem support. The ext4 rootfs needs a working `/sbin/init` and its
userspace dependencies. TTstack passes `root=/dev/vda rw init=/sbin/init` plus a
static `ip=ADDRESS::10.10.0.1:255.255.0.0::eth0:off` argument. The guest must apply
that address, either through kernel IP configuration or its init process.

TTstack attaches a rate-limited virtio entropy device for reliable cold starts.
Use a maintained guest kernel with built-in `CONFIG_HW_RANDOM=y` and
`CONFIG_HW_RANDOM_VIRTIO=y`; without its driver, applications that need secure
randomness can wait minutes for the kernel's random pool to initialize. Do not
reuse a fixed random seed or disable secure randomness to shorten boot time.
The Firecracker binary must support the
[entropy device configuration](https://github.com/firecracker-microvm/firecracker/blob/main/docs/entropy.md)
(verified with Firecracker 1.17.0). The built-in `fc-alpine` recipe is only a boot
smoke test; its legacy kernel is not a maintained application image.

```bash
sudo /opt/ttstack/bin/tt image create fc-alpine
/opt/ttstack/bin/tt env create micro --image fc-alpine --engine firecracker \
  --cpu 1 --mem 128
```

The built-in rootfs configures networking and runs BusyBox init. It does not start
an application or sshd. Read boot output on the agent at
`/home/ttstack/run/fc-VM_ID.log`. There is no managed SSH key injection or
interactive console. Build a suitable rootfs for application workloads; old images
with a hard-coded IP need replacement, not just a TTstack binary upgrade.

Omit `--disk` to retain the base rootfs size, or specify a size in MiB at creation:

```bash
tt env create fc-work --image fc-alpine --engine firecracker \
  --cpu 2 --mem 1024 --disk 2048
```

The agent clones the image, checks the offline filesystem with `e2fsck`, and grows
the raw file or zvol and its ext4 filesystem with `resize2fs` before first boot.
Only unpartitioned ext4 rootfs images are supported. Shrinking below the base
image size is rejected before allocation; the base image is never modified.
Disk space is reserved at the requested logical size, even for sparse files;
configuration drives reserve another 4 MiB. Filesystem metadata and reserved
blocks reduce the capacity reported by guest tools such as `df`. Upgrade agents to one advertising
`firecracker_disk_resize` before specifying a size. CPU/RAM still need to fit the
host's configured capacity, including VMM overhead. TTstack has no application
or user-tier sizing policy; callers can impose their own ceilings.

This is creation-time sizing, not a resize API for existing VMs. Stop/start
preserves the selected size and data. It does not expand or shrink old disks.

Stop requests orderly shutdown on x86_64, waits up to 30 seconds, then forcibly
terminates if necessary. The kernel needs `CONFIG_SERIO_I8042` and
`CONFIG_KEYBOARD_ATKBD`; init must handle Ctrl-Alt-Del by stopping applications,
syncing/unmounting filesystems and rebooting (Firecracker exits on reset).
New `fc-alpine` builds provide BusyBox init shutdown handling. Existing images are
not rewritten; rebuild under a new image name/directory to adopt the new init.
Start cold-boots the preserved disk. Memory is not retained. Normal stop/start
does not implement snapshot or suspend-to-disk semantics.

### Firecracker guest configuration

`--guest-config FILE` reads a JSON object mapping simple file names to UTF-8
contents. REST clients use `vms[].guest_config`. For example:

```json
{"application.json":"{\"listen\":\"0.0.0.0:8080\"}"}
```

Use at most 32 files and 64 KiB total name/content bytes. Names are up to 128 ASCII
letters, digits, `.`, `_`, `-`, with no leading dot or path separators. Binary
files and directory trees are intentionally unsupported. The CLI input JSON has
a 512 KiB encoded-size limit. Configuration is per VM; CLI duplicates receive the
same contents, so submit separate requests when each VM needs a different secret.

The agent builds a 4 MiB ext4 drive labelled `TTCONFIG`, exposed read-only as
`/dev/vdb`. A custom guest init can mount it as root:

```sh
mkdir -p /run/ttstack-config
chmod 700 /run/ttstack-config
mount -t ext4 -o ro,nosuid,nodev,noexec /dev/vdb /run/ttstack-config
```

New `fc-alpine` builds perform that mount when the drive exists. Files are readable
by guest root; application init can copy selected values into its own protected
configuration. TTstack does not execute these files, interpret keys, install an
application or inject account credentials by itself. Do not include shared
administrator secrets. Query responses contain only a digest. The disk is private
to the VMM UID/root on the host and is deleted with the VM, not on stop.
Configuration remains unchanged across boots; renewal inside a running guest is
the application's responsibility.

## Storage

| Backend | Base image format | Runtime storage |
|---|---|---|
| `file` | QEMU qcow2 file; Firecracker kernel/rootfs directory | Per-VM copy, using reflinks on Linux when available, otherwise a full copy |
| `zvol` | QEMU raw disk volume; Firecracker kernel dataset with an ext4 root volume | Per-VM snapshot clones |
| Docker runtime | Container image | Managed by Docker/Podman, independently of agent storage |

For QEMU and Firecracker, the controller prefers eligible **ZFS hosts** over file
hosts, then packs by free memory within that group. The host must be online, have
the image and required capabilities, and have sufficient reservations. If no ZFS
host qualifies, a file host can be selected. Docker placement is unchanged.
Firecracker on ZFS requires the `firecracker_zvol` agent capability; update both
controller and agents before using it.

Storage is an operator-provisioned host setting: use `--storage zvol` with dataset
names such as `tank/ttstack/images` and `tank/ttstack/runtime`, not `/dev/zvol/...`
paths. Hosts without provisioned ZFS use `--storage file` (the agent/deployment
default). TTstack does not choose an arbitrary pool, format a disk, or switch a
running agent's storage automatically. An unavailable pool or failed clone is an
error, not permission to create an empty file disk. Existing VMs keep their host
and disks across stop/start. Do not change an agent's backend or storage roots
while it owns VMs; disk migration is not implemented.

### ZFS image preparation

Create the pool and parent datasets first. A QEMU base is a volume directly below
`images/IMAGE_NAME`; import **raw bootable disk contents**, not qcow2 encoded bytes.
Use `qemu-img convert -f qcow2 -O raw SOURCE.qcow2 RAW_DISK` when converting a
qcow2 source. `tt image create` prepares files; it does not import zvols.

A Firecracker base uses this layout:

```text
tank/ttstack/images/fc-app          filesystem dataset: vmlinux file
tank/ttstack/images/fc-app/rootfs   volume: raw, unpartitioned ext4 contents
```

For example, with a prepared **128 MiB** `rootfs.ext4` and matching kernel:

```bash
sudo zfs create -p -o mountpoint=/srv/tt-images tank/ttstack/images
sudo zfs create -p -o mountpoint=/srv/tt-run tank/ttstack/runtime
sudo zfs create tank/ttstack/images/fc-app
sudo cp /path/to/vmlinux /srv/tt-images/fc-app/vmlinux
sudo zfs create -s -V 128M -o volmode=dev tank/ttstack/images/fc-app/rootfs
sudo udevadm settle --timeout=10
sudo dd if=/path/to/rootfs.ext4 of=/dev/zvol/tank/ttstack/images/fc-app/rootfs \
  bs=4M conv=fsync status=progress
```

Choose the volume size to match the source image (a ZFS block-size multiple),
never smaller. If making a larger base volume, check and grow its offline ext4
filesystem with `e2fsck -f -p` and `resize2fs` before using it. Keep the kernel in
the dataset and leave `rootfs.ext4` absent there: TTstack creates a per-clone
symlink to the clone's own volume. Parent image/runtime datasets must have active
absolute mountpoints. Keep the runtime mountpoint short enough for Unix sockets.
Do not run the import against a volume used by an existing guest.

Each base revision uses a fixed `@ttsnap` snapshot. Firecracker snapshots the
kernel dataset and root volume together, then clones both. Editing a base does
not refresh that snapshot; import new revisions under new names. The runtime
clone keeps the kernel, immutable configuration drive and root-volume reference.
The jail exposes only that VM's block device, owned by its non-root VMM UID;
it does not change ownership of the host `/dev/zvol` node. On cold start the
current device is resolved again, so `/dev/zdN` numbers are not persisted.

Stop retains the datasets and volume. Delete removes the jail and the runtime
clones, leaving the base snapshots available for other VMs. Partially created
clones remain attached to a failed VM record for explicit, retryable deletion.
Do not independently destroy live clones or their base snapshots.

### Host paths and recovery

The `file` backend also works on a mounted ZFS filesystem. That still uses files,
not zvols or per-VM snapshots. Keep a file rootfs and its jail on the same filesystem
for persistent hard links. Zvol kernel/config files may cross dataset boundaries;
TTstack copies those read-only files into the jail when necessary.

Firecracker stores PID, console and sandbox metadata in `/home/ttstack/run`.
Place that directory and the agent database on persistent storage too. Dataset
quotas and agent disk reservations are separate limits; leave pool headroom and
configure both deliberately. Sparse volumes reserve logical capacity in TTstack,
not all their physical pool space. Snapshot stopped guests for an offline recovery
point; a running-disk snapshot is not an application consistency guarantee.

For systemd services, add `RequiresMountsFor=` and explicit
`ExecStartPre=/usr/bin/mountpoint -q PATH` checks for required dataset mountpoints.
A missing mount must fail startup rather than create replacement VM state on
the system disk. Use stable disk identifiers when provisioning a pool; verify
unused devices separately from TTstack deployment.

See [Firecracker zvol validation](firecracker-zvol-validation-2026-09-24.md) for
the tested versions, lifecycle evidence and limits.

## Networking and platform scope

Linux QEMU and Firecracker use TAP devices on bridge `tt0` (`10.10.0.1/16`), with
host-local IPv4 addresses and nftables NAT. Guest IPs are local to a host and may
repeat across hosts. QEMU cloud-init configures DNS as `8.8.8.8` and `1.1.1.1`;
the guest/network must be able to reach them for name resolution.

Port forwarding is **TCP only**, from allocated host ports to requested guest
ports. `--deny-outgoing` blocks routed outbound initiation while permitting replies
to inbound traffic. It can also prevent external DNS access; it is not isolation
from the host or other guests on the bridge. Environments do not create a private
cross-host network. Docker uses its own networking and port publishing.

For mutually untrusted Linux QEMU/Firecracker guests, set `isolated_network: true`
or `--isolated-network`. Host-enforced bridge rules block direct peer traffic,
MAC/IP/ARP spoofing and IPv6. Routed rules block guest-initiated access to host
services, RFC1918, link-local, carrier-grade NAT and other listed non-public ranges.
Public IPv4 egress and responses to host/external connections remain available;
combine with `deny_outgoing` to block routed initiation entirely. Isolation is
per VM, including peers in the same environment. It has no private-destination
allowlist; applications needing private services should use a separately controlled
gateway. It is not a public-Internet destination allowlist.

Published TCP ports still accept external connections. Restrict these to your
gateway using the host/provider firewall and authenticate guest applications.
Keep controller/agent endpoints on a protected management network; remote management
addresses on public networks are not covered by a private-address egress block.

See [compatibility and validation](compatibility.md) for tested Linux workflows.
