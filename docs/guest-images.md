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
| `freebsd-base` | Jail | Experimental FreeBSD base archive; not a validated Linux workflow |

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

Firecracker requires **file storage** and an image directory with both files:

```text
/home/ttstack/images/fc-alpine/
├── vmlinux
└── rootfs.ext4
```

The kernel must support Firecracker's devices, including built-in virtio-mmio and
root filesystem support. The ext4 rootfs needs a working `/sbin/init` and its
userspace dependencies. TTstack passes `root=/dev/vda rw init=/sbin/init` plus a
static `ip=ADDRESS::10.10.0.1:255.255.0.0::eth0:off` argument. The guest must apply
that address, either through kernel IP configuration or its init process.

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
both the raw file and its ext4 filesystem with `resize2fs` before first boot.
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
| `file` (default) | QEMU qcow2 file; Firecracker kernel/rootfs directory | Per-VM copy, using reflinks on Linux when available, otherwise a full copy |
| `zvol` (QEMU) | Raw bootable disk in a ZFS volume | Clone of a base snapshot |
| Docker runtime | Container image | Managed by Docker/Podman, independently of the agent storage setting |

For zvol, configure dataset names such as `tank/ttstack/images` and
`tank/ttstack/runtime`, not `/dev/zvol/...` paths. Prepare the existing pool,
parent datasets and base volume manually; import **raw disk contents**, not a
qcow2 file's encoded bytes. The corresponding device is exposed at
`/dev/zvol/tank/ttstack/images/IMAGE_NAME`. `tt image create` does not import zvols.

The zvol backend reuses each base's `@ttsnap` snapshot for clones. Editing the base
volume does not refresh that snapshot. Use a new base volume name for a new image
revision. Zvol is implemented but is not covered by the current live validation.

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

All FreeBSD/Bhyve/Jail/PF paths are **experimental** and require manual setup.
QEMU and Firecracker instructions above describe Linux hosts. Bhyve currently
rejects in-place restart; do not assume the Linux lifecycle guarantees apply to
FreeBSD. See [compatibility and validation](compatibility.md).
