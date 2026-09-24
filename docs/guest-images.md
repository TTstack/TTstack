# Guest Image Guide

TTstack supports multiple VM engines, each requiring its own image format.
This guide covers how to create, manage, and deploy guest images for each engine.

## Quick Start

Use the built-in image recipes:

```bash
# List available recipes
tt image recipes

# Create all images for this platform
sudo tt image create all --image-dir /home/ttstack/images

# Create all Docker images
sudo tt image create all --engine docker

# Create a specific image
sudo tt image create fc-alpine --image-dir /home/ttstack/images
sudo tt image create alpine-cloud --image-dir /home/ttstack/images
```

## Accessing VMs

QEMU cloud images support **SSH**. Provide SSH public keys when creating the
environment; TTstack configures root access via cloud-init. Other engines have
the access methods listed below. Port 22 is auto-included for QEMU / experimental Bhyve and Jail.

| Engine | Access Method |
|--------|--------------|
| **QEMU** (cloud images) | SSH via port forwarding (key injected by cloud-init) |
| **QEMU** (custom images) | SSH via port forwarding (your own key setup) |
| **Docker** | SSH into container (if sshd installed) or `docker exec` from host |
| **Firecracker** | Boot log on agent; custom guest workload (no managed SSH/console) |
| **Bhyve** (experimental FreeBSD) | SSH via port forwarding |

### QEMU Cloud Images (SSH)

For built-in cloud images (`alpine-cloud`, `debian-cloud`, `ubuntu-cloud`),
TTstack auto-generates a **cloud-init seed ISO** on each VM boot that:

- Injects your SSH public key(s) into `~root/.ssh/authorized_keys`
- Enables SSH public-key authentication
- Configures the VM's network (static IP, gateway, DNS)

To SSH into a QEMU VM:

```bash
# Create environment with your SSH key
tt env create myenv --image alpine-cloud --engine qemu \
  --ssh-key ~/.ssh/id_ed25519.pub

# Show environment to see port mappings
tt env show myenv
# Example output:
#   ID             IMAGE         ENGINE   STATE    IP             PORTS
#   abc12345-678   alpine-cloud  qemu     running  10.10.0.3      20100->22

# SSH using the mapped port
ssh root@<host-ip> -p 20100
```

For custom QEMU images that do not use cloud-init, the seed ISO
is harmlessly ignored — you manage SSH credentials yourself.

### Docker Containers

Docker containers are managed via the Docker runtime. If the container
has an SSH daemon, connect via the mapped port. Otherwise, access
from the host machine:

```bash
# Find the container ID
docker ps | grep <vm-id>

# Exec into the container (host-only fallback)
docker exec -it <container-id> sh
```

### Firecracker MicroVMs

Firecracker's built-in image is a boot/network smoke-test workload. It has no
managed SSH or interactive console. View boot output on the agent at
`/home/ttstack/run/fc-<vm-id>.log`; use a custom rootfs for your workload.

## Image Formats by Engine

### Docker / Podman

Docker images are **container images** pulled from registries or built locally.
They are **not** stored in the TTstack image directory — they live in the
Docker/Podman image store.

**Creating a Docker image:**
```bash
# Pull from a registry
docker pull alpine:latest

# Or build locally
cat > Dockerfile <<'EOF'
FROM scratch
COPY myapp /app
CMD ["/app"]
EOF
docker build -t my-image .
```

**Using with TTstack:**
```bash
tt env create myenv --image alpine --engine docker --port 80
```

**Key points:**
- The `--image` name must match a locally available Docker/Podman image
- Docker images do NOT need to exist in `image_dir`
- Port mappings are handled by Docker's `-p` flag
- Docker manages its own networking (no TAP devices or bridge needed)

### Firecracker

Firecracker images are **directories** containing two files:
- `vmlinux` — uncompressed Linux kernel with virtio_mmio built-in
- `rootfs.ext4` — ext4 filesystem image used as the root device

**Image directory structure:**
```
/home/ttstack/images/
  └── fc-alpine/
      ├── vmlinux        # ~21MB kernel
      └── rootfs.ext4    # Root filesystem
```

**Creating manually:**

1. **Get a Firecracker-compatible kernel:**
   ```bash
   # Option A: Download pre-built (recommended)
   curl -sL https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/x86_64/kernels/vmlinux.bin \
     -o /home/ttstack/images/fc-alpine/vmlinux

   # Option B: Build from source with virtio_mmio=y
   # (The stock Debian kernel won't work — it has virtio_mmio as a module)
   ```

2. **Create the rootfs:**
   ```bash
   dd if=/dev/zero of=rootfs.ext4 bs=1M count=128
   mkfs.ext4 rootfs.ext4
   mkdir /tmp/mnt && mount -o loop rootfs.ext4 /tmp/mnt

   # Populate with your desired userspace (Alpine, BusyBox, etc.)
   # At minimum: /sbin/init (or /init) must exist
   mkdir -p /tmp/mnt/{bin,sbin,etc,proc,sys,dev}
   # ... install packages or copy a static init binary ...

   umount /tmp/mnt
   ```

**Important notes:**
- The kernel MUST have `virtio_mmio` built-in (not as a module)
- The stock Debian/Ubuntu kernel will NOT work — use the Firecracker pre-built kernel
- Boot args: `console=ttyS0 reboot=k panic=1 pci=off`; the rootfs is set via `is_root_device: true`
- Firecracker VMs use TAP devices on the `tt0` bridge for networking
- Pause/resume is supported via the Firecracker API

### QEMU / KVM

QEMU images are **qcow2 disk image files** containing a bootable filesystem.

**Image location:**
```
/home/ttstack/images/
  └── qemu-test          # Single qcow2 file (for file storage)
```

When using zvol storage, the image is written to a ZFS zvol exposed as a
raw block device at `/dev/zvol/<pool>/<dataset>`:
```
/dev/zvol/tank/ttstack/images/qemu-test   # Raw block device
```

**Creating manually:**

```bash
# Create qcow2 image
qemu-img create -f qcow2 /home/ttstack/images/qemu-test 256M

# Mount via NBD and populate
modprobe nbd max_part=8
qemu-nbd --connect=/dev/nbd0 /home/ttstack/images/qemu-test
mkfs.ext4 /dev/nbd0
mount /dev/nbd0 /tmp/mnt

# Populate rootfs
mkdir -p /tmp/mnt/{bin,sbin,etc,proc,sys,dev}
# ... install packages or copy a static init binary ...

umount /tmp/mnt
qemu-nbd --disconnect /dev/nbd0
```

**Important notes:**
- QEMU uses KVM acceleration (`-enable-kvm`), so `/dev/kvm` must exist
- The disk is attached as virtio (`if=virtio`), so the guest kernel needs virtio drivers
- QEMU uses TAP devices on the `tt0` bridge for networking
- Stop/start uses the QEMU monitor (`stop`/`cont` commands) — the process stays alive

## Storage Backend Considerations

### File (default)

Images are plain qcow2 files, filesystem-agnostic. On Linux, cloning uses
`cp --reflink=auto` (CoW if the filesystem supports it, full copy
otherwise). On FreeBSD, plain `cp -a` is used.

```bash
# No special setup needed — just place images in image_dir
cp my-image.qcow2 /home/ttstack/images/qemu-test
```

### Zvol

Images are stored as **ZFS zvols** — raw block devices exposed at
`/dev/zvol/<pool>/<dataset>`. Cloning uses `zfs snapshot` + `zfs clone`
— instant and space-efficient.

```bash
# Setup
zpool create ttpool /dev/nvmeXnYpZ
zfs create ttpool/images
zfs create ttpool/runtime

# Create image as a zvol
zfs create -V 10G ttpool/images/qemu-test
# Write image data to /dev/zvol/ttpool/images/qemu-test

# Agent config
tt-agent --image-dir ttpool/images --runtime-dir ttpool/runtime --storage zvol
```

## Networking

All VM engines (except Docker) use a shared network topology:

```
Guest VM ←→ TAP device ←→ tt0 bridge (10.10.0.1/16) ←→ NAT (nftables) ←→ Host
```

- Each VM gets a unique IP in the 10.10.0.0/16 range
- Port forwarding: host port → guest port via nftables DNAT (Linux) or PF rdr (FreeBSD)
- The `tt0` bridge and NAT table are created automatically by the agent

Docker containers use Docker's native networking with `-p` port publishing.

Experimental FreeBSD support uses PF instead of nftables:

```
Guest VM ←→ TAP device ←→ tt0 bridge (10.10.0.1/16) ←→ NAT (PF) ←→ Host
```

## Experimental FreeBSD development notes

The following is historical development guidance, not a supported deployment
workflow. FreeBSD/Bhyve/Jail fixes and host validation are outside the primary scope.

Running FreeBSD inside QEMU on a Linux host is useful for testing the
FreeBSD agent, controller, and CLI. This section documents the
non-interactive approach, including common pitfalls.

### Recommended: mfsBSD Live ISO

[mfsBSD](https://mfsbsd.vx.sk/) is a FreeBSD live CD that runs entirely
in RAM with SSH enabled out of the box. This is the fastest way to get
a working FreeBSD environment for build/test purposes.

```bash
# Download mfsBSD SE (Special Edition includes base packages)
curl -sL https://mfsbsd.vx.sk/files/iso/14/amd64/mfsbsd-se-14.2-RELEASE-amd64.iso \
  -o /tmp/mfsbsd.iso

# Create a work disk for persistent storage
qemu-img create -f qcow2 /tmp/fbsd-work.qcow2 20G

# Boot the VM (8GB RAM recommended for compiling Rust)
qemu-system-x86_64 -enable-kvm -m 8192 -smp 4 \
  -drive file=/tmp/fbsd-work.qcow2,format=qcow2,if=virtio \
  -cdrom /tmp/mfsbsd.iso -boot d \
  -netdev user,id=net0,hostfwd=tcp::2222-:22 \
  -device virtio-net-pci,netdev=net0 \
  -vnc none -daemonize

# SSH in (default root password: mfsroot)
sshpass -p 'mfsroot' ssh -p 2222 root@127.0.0.1
```

### Installing FreeBSD to Disk from mfsBSD

mfsBSD runs in RAM so installed packages are lost on reboot. For
persistent use, install FreeBSD to the work disk:

```bash
# Inside the mfsBSD VM:
gpart create -s gpt vtbd0
gpart add -t freebsd-boot -s 512k vtbd0
gpart add -t freebsd-ufs -l rootfs vtbd0
gpart bootcode -b /boot/pmbr -p /boot/gptboot -i 1 vtbd0
newfs -U /dev/vtbd0p2

# Mount and extract base + kernel
mkdir -p /rw/disk && mount /dev/vtbd0p2 /rw/disk
cd /rw/disk
fetch -o - https://download.freebsd.org/releases/amd64/14.3-RELEASE/base.txz | tar xf -
fetch -o - https://download.freebsd.org/releases/amd64/14.3-RELEASE/kernel.txz | tar xf -

# Configure the installed system
echo '/dev/vtbd0p2  /  ufs  rw  1  1' > etc/fstab
cat > etc/rc.conf <<'RCEOF'
hostname="fbsd-test"
ifconfig_vtnet0="DHCP"
sshd_enable="YES"
sendmail_enable="NONE"
RCEOF
echo 'mfsroot' | chroot /rw/disk pw usermod root -h 0
sed -i '' 's/^#PermitRootLogin .*/PermitRootLogin yes/' etc/ssh/sshd_config
echo 'nameserver 10.0.2.3' > etc/resolv.conf

umount /rw/disk
```

Then reboot the VM without the `-cdrom` and `-boot d` flags to boot
from disk.

### Common Pitfalls

| Problem | Cause | Solution |
|---------|-------|----------|
| FreeBSD cloud images won't accept SSH | Root login disabled, no password auth, cloud-init issues | Use mfsBSD instead |
| `virt-customize` fails on FreeBSD images | libguestfs cannot write to UFS filesystems | Use mfsBSD or manual install |
| mfsBSD `pkg install` fails with "No error" | pkg 2.1.0 on mfsBSD 14.2 cannot handle zstd-packed repos | Install FreeBSD to disk for a newer pkg |
| `cp --reflink=auto` fails on FreeBSD | GNU option not available | Fixed in TTstack — FreeBSD uses `cp -a` |
| `ifconfig bridge create` names bridge `bridge0` | FreeBSD auto-assigns names | Fixed in TTstack — uses `ifconfig bridge create name tt0` |
| `ifconfig <tap> create` fails for custom names | Must specify type first | Fixed in TTstack — uses `ifconfig tap create name <tap>` |
| Bhyve TAP name mismatch | Old code used `tap-{id}` instead of hashed name | Fixed in TTstack — uses `net::tap_name()` |
| OOM during Rust compilation on mfsBSD | mfsBSD runs in RAM; 4GB is insufficient | Use 8GB+ RAM or install to disk |

## Current operational limits

- Run `tt image create` locally on the target agent; image distribution is manual.
- QEMU file storage requires qcow2. `--disk` grows the cloned virtual disk; the
  guest must grow its partition/filesystem (cloud-init images commonly do this).
  Requests smaller than the source virtual disk fail explicitly. Zvol uses raw.
- Firecracker requires file storage and a directory containing `vmlinux` and
  `rootfs.ext4`. Its root filesystem size is retained. The guest must configure its
  IP from the kernel `ip=` argument; the built-in recipe does this. Regenerate older
  `fc-alpine` images to replace their hard-coded address. Boot output is in
  `/home/ttstack/run/fc-<id>.log`; managed SSH and interactive consoles are not provided.
- Docker images must have a long-running default command. Base OS images may exit
  immediately; database images may require configuration that TTstack does not
  expose. Supply a prepared image with a suitable default command. Disk quotas,
  outgoing network restrictions and SSH injection are not supported for Docker.
