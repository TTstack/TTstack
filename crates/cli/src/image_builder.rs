//! Local guest image recipes.
//!
//! Downloads/prepares base images on the target host. Recipes do not provision
//! applications; container defaults and custom guest startup must suit the workload.

use ruc::*;
use std::path::Path;
use tokio::process::Command;

// ── Image catalog ───────────────────────────────────────────────────

/// A built-in image recipe that can be auto-generated.
pub struct ImageRecipe {
    pub name: &'static str,
    pub engine: &'static str,
    pub description: &'static str,
}

/// List of all auto-generatable images.
pub const RECIPES: &[ImageRecipe] = &[
    // Docker / Podman — lightweight containers
    ImageRecipe {
        name: "alpine",
        engine: "docker",
        description: "Alpine Linux 3.21 container base (needs a long-running workload)",
    },
    ImageRecipe {
        name: "debian",
        engine: "docker",
        description: "Debian 13 Trixie slim container base (needs a long-running workload)",
    },
    ImageRecipe {
        name: "ubuntu",
        engine: "docker",
        description: "Ubuntu 24.04 container base (needs a long-running workload)",
    },
    ImageRecipe {
        name: "rockylinux",
        engine: "docker",
        description: "Rocky Linux 9 minimal container base (needs a long-running workload)",
    },
    ImageRecipe {
        name: "nginx",
        engine: "docker",
        description: "Nginx web server (Alpine-based)",
    },
    ImageRecipe {
        name: "redis",
        engine: "docker",
        description: "Redis 7 (Alpine-based)",
    },
    ImageRecipe {
        name: "postgres",
        engine: "docker",
        description: "PostgreSQL 17 (requires workload-specific initialization settings)",
    },
    // Firecracker — microVMs
    ImageRecipe {
        name: "fc-alpine",
        engine: "firecracker",
        description: "Alpine boot/network check (kernel + 128 MiB rootfs; no SSH)",
    },
    // QEMU/KVM — full VMs (cloud images)
    ImageRecipe {
        name: "alpine-cloud",
        engine: "qemu",
        description: "Alpine Linux 3.21.7 NoCloud image (qcow2)",
    },
    ImageRecipe {
        name: "debian-cloud",
        engine: "qemu",
        description: "Debian 13 generic cloud image (qcow2, daily/latest)",
    },
    ImageRecipe {
        name: "ubuntu-cloud",
        engine: "qemu",
        description: "Ubuntu 24.04 cloud image (qcow2, current)",
    },
];

/// Print available image recipes.
pub fn list_recipes() {
    println!("{:<16} {:<14} DESCRIPTION", "NAME", "ENGINE");
    for r in RECIPES {
        println!("{:<16} {:<14} {}", r.name, r.engine, r.description);
    }
}

// ── Docker / Podman images ──────────────────────────────────────────

/// Map recipe name to Docker image tag.
fn docker_tag(name: &str) -> &str {
    match name {
        "alpine" => "alpine:3.21",
        "debian" => "debian:trixie-slim",
        "ubuntu" => "ubuntu:24.04",
        "rockylinux" => "rockylinux:9-minimal",
        "nginx" => "nginx:alpine",
        "redis" => "redis:7-alpine",
        "postgres" => "postgres:17-alpine",
        _ => name,
    }
}

async fn detect_runtime() -> Result<&'static str> {
    if Command::new("docker")
        .arg("version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Ok("docker");
    }
    if Command::new("podman")
        .arg("version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Ok("podman");
    }
    Err(eg!("neither docker nor podman found"))
}

async fn create_docker(name: &str) -> Result<()> {
    let rt = detect_runtime().await?;
    let tag = docker_tag(name);

    println!("[image] pulling {tag} via {rt}...");
    let output = Command::new(rt)
        .args(["pull", tag])
        .output()
        .await
        .c(d!("pull failed"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eg!("{rt} pull {tag} failed: {}", stderr));
    }

    // Tag as the short name so `tt env create --image alpine` works
    if tag != name {
        run_cmd(rt, &["tag", tag, name]).await?;
    }

    println!("[image] {name} ready ({rt})");
    Ok(())
}

// ── Firecracker images ──────────────────────────────────────────────

const FC_KERNEL_URL: &str =
    "https://s3.amazonaws.com/spec.ccfc.min/img/quickstart_guide/x86_64/kernels/vmlinux.bin";

/// Alpine rootfs mirror.
const ALPINE_MINIROOTFS_URL: &str = "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/x86_64/alpine-minirootfs-3.21.3-x86_64.tar.gz";

async fn create_firecracker(name: &str, image_dir: &Path) -> Result<()> {
    let target = image_dir.join(name);
    tokio::fs::create_dir_all(&target).await.c(d!("mkdir"))?;

    let kernel = target.join("vmlinux");
    let destination = target.join("rootfs.ext4");

    // Download kernel
    if !kernel.exists() {
        println!("[image] downloading Firecracker kernel...");
        download_verified(
            FC_KERNEL_URL,
            &kernel,
            Some("ea5e7d5cf494a8c4ba043259812fc018b44880d70bcbbfc4d57d2760631b1cd6"),
        )
        .await?;
        println!("[image] kernel: {}", human_size(&kernel).await);
    } else {
        println!("[image] kernel already exists");
    }

    // Create rootfs with Alpine userspace
    if destination.exists() {
        println!("[image] rootfs already exists");
        return Ok(());
    }

    let staging = tempfile::NamedTempFile::new_in(&target).c(d!("rootfs staging file"))?;
    let rootfs = staging.path().to_path_buf();
    let rootfs_mb: u32 = 128;
    println!("[image] creating {rootfs_mb}MB rootfs with Alpine userspace...");

    // Create empty ext4 image
    run_cmd(
        "dd",
        &[
            "if=/dev/zero",
            &format!("of={}", rootfs.display()),
            "bs=1M",
            &format!("count={rootfs_mb}"),
        ],
    )
    .await?;
    run_cmd("mkfs.ext4", &["-q", &rootfs.display().to_string()]).await?;

    // Mount and populate
    let mount_dir = tempfile::Builder::new()
        .prefix("tt-image-")
        .tempdir()
        .c(d!("private mount directory"))?;
    let mnt = mount_dir.path().to_path_buf();
    run_cmd(
        "mount",
        &[
            "-o",
            "loop",
            &rootfs.display().to_string(),
            &mnt.display().to_string(),
        ],
    )
    .await?;

    let populate: Result<()> = async {
        // Download and extract Alpine minirootfs
        let tarball = format!("{}/alpine.tar.gz", mnt.display());
        download_verified(ALPINE_MINIROOTFS_URL, Path::new(&tarball), Some("1a694899e406ce55d32334c47ac0b2efb6c06d7e878102d1840892ad44cd5239")).await?;
        run_cmd("tar", &["xzf", &tarball, "-C", &mnt.display().to_string()]).await?;
        tokio::fs::remove_file(&tarball).await.ok();

        // Create init wrapper that mounts essential filesystems
        let init_script = format!(
            r#"#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev 2>/dev/null
mkdir -p /dev/pts
mount -t devpts devpts /dev/pts
hostname ttstack
echo "TTstack Firecracker guest [{name}] booted OK"

# Set up networking if virtio-net is available
ip link set eth0 up 2>/dev/null
# The agent supplies the allocated address through the kernel command line.
for arg in $(cat /proc/cmdline); do
    case "$arg" in
        ip=*) addr=${{arg#ip=}}; addr=${{addr%%:*}}; ip addr add "$addr/16" dev eth0 2>/dev/null ;;
    esac
done
ip route add default via 10.10.0.1 2>/dev/null

# Configuration is data, never executed by this generic image.
if [ -b /dev/vdb ]; then
    mkdir -p /run/ttstack-config
    chmod 700 /run/ttstack-config
    mount -t ext4 -o ro,nosuid,nodev,noexec /dev/vdb /run/ttstack-config || exit 1
fi
# BusyBox init stays alive and handles Ctrl-Alt-Del and shutdown.
"#
        );
        let init_path = format!("{}/etc/ttstack-boot", mnt.display());
        tokio::fs::write(&init_path, init_script)
            .await
            .c(d!("write init"))?;
        run_cmd("chmod", &["755", &init_path]).await?;

        // Ensure /sbin/init symlink
        let sbin = format!("{}/sbin", mnt.display());
        tokio::fs::create_dir_all(&sbin).await.ok();
        let sbin_init = format!("{sbin}/init");
        let _ = tokio::fs::remove_file(&sbin_init).await;
        tokio::fs::symlink("/bin/busybox", &sbin_init)
            .await
            .c(d!("install guest init"))?;
        tokio::fs::write(mnt.join("etc/inittab"), "::sysinit:/etc/ttstack-boot\n::ctrlaltdel:/sbin/reboot\n::shutdown:/bin/sync\n::shutdown:/bin/umount -a -r\n")
            .await.c(d!("write guest shutdown configuration"))?;

        // Set up DNS
        let etc = format!("{}/etc", mnt.display());
        tokio::fs::create_dir_all(&etc).await.ok();
        tokio::fs::write(format!("{etc}/resolv.conf"), "nameserver 8.8.8.8\n")
            .await
            .ok();

        Ok(())
    }
    .await;
    if let Err(e) = run_cmd("umount", &[&mnt.display().to_string()]).await {
        let mnt = mount_dir.keep();
        let (_, saved) = staging.keep().map_err(|e| eg!(e.to_string()))?;
        return Err(eg!(
            "{}; staging rootfs retained at {}; unmount {} before removing it",
            e,
            saved.display(),
            mnt.display()
        ));
    }
    tokio::fs::remove_dir(&mnt).await.ok();
    populate?;
    staging
        .persist(&destination)
        .map_err(|e| eg!(e.to_string()))?;

    println!(
        "[image] {name} ready: kernel={}, rootfs={}",
        human_size(&kernel).await,
        human_size(&destination).await
    );
    Ok(())
}

// ── QEMU cloud images ──────────────────────────────────────────────

fn qemu_cloud_url(name: &str) -> Option<&'static str> {
    match name {
        "alpine-cloud" => Some(
            "https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/cloud/nocloud_alpine-3.21.7-x86_64-bios-cloudinit-r0.qcow2",
        ),
        "debian-cloud" => Some(
            "https://cloud.debian.org/images/cloud/trixie/daily/latest/debian-13-generic-amd64-daily.qcow2",
        ),
        "ubuntu-cloud" => {
            Some("https://cloud-images.ubuntu.com/noble/current/noble-server-cloudimg-amd64.img")
        }
        _ => None,
    }
}

async fn create_qemu(name: &str, image_dir: &Path) -> Result<()> {
    let target = image_dir.join(name);

    if target.exists() {
        let info = Command::new("qemu-img")
            .args(["info", "--output=json"])
            .arg(&target)
            .kill_on_drop(true)
            .output()
            .await
            .c(d!("inspect existing image"))?;
        let valid = info.status.success()
            && serde_json::from_slice::<serde_json::Value>(&info.stdout)
                .ok()
                .is_some_and(|v| {
                    v["format"] == "qcow2" && v["virtual-size"].as_u64().is_some_and(|s| s > 0)
                });
        if !valid {
            return Err(eg!(
                "existing image is not a valid qcow2 disk; inspect or remove it before retrying"
            ));
        }
        println!("[image] {name} already exists");
        return Ok(());
    }

    let url = qemu_cloud_url(name).ok_or_else(|| eg!(format!("unknown QEMU image: {name}")))?;

    println!("[image] downloading {name} cloud image...");
    let staging = tempfile::Builder::new()
        .prefix(".tt-image-")
        .tempdir_in(image_dir)
        .c(d!("image staging directory"))?;
    let downloaded = staging.path().join("download");
    let pin = (name == "alpine-cloud").then_some("5b22a46e9aa6bbacf585c055e87362c8be1993e53c121bdaf74203ac3490c70bdbbf714df4276eef36184ea8c11fd7cd3b28c9ccc74f6a9a82c430d441fa2f95");
    download_verified(url, &downloaded, pin).await?;
    // Cloud providers use .img for both qcow2 and raw: inspect content, never guess by suffix.
    let info = Command::new("qemu-img")
        .args(["info", "--output=json"])
        .arg(&downloaded)
        .kill_on_drop(true)
        .output()
        .await
        .c(d!("inspect downloaded image"))?;
    if !info.status.success() {
        return Err(eg!(
            "invalid cloud image: {}",
            String::from_utf8_lossy(&info.stderr)
        ));
    }
    let info: serde_json::Value =
        serde_json::from_slice(&info.stdout).c(d!("cloud image format"))?;
    match info["format"].as_str() {
        Some("qcow2") => tokio::fs::rename(&downloaded, &target)
            .await
            .c(d!("publish image"))?,
        Some("raw") => {
            let converted = staging.path().join("disk.qcow2");
            run_cmd(
                "qemu-img",
                &[
                    "convert",
                    "-f",
                    "raw",
                    "-O",
                    "qcow2",
                    &downloaded.display().to_string(),
                    &converted.display().to_string(),
                ],
            )
            .await?;
            tokio::fs::rename(converted, &target)
                .await
                .c(d!("publish converted image"))?;
        }
        _ => return Err(eg!("unsupported cloud image format; expected raw or qcow2")),
    }

    println!("[image] {name} ready: {}", human_size(&target).await);
    Ok(())
}

// ── Public entry point ──────────────────────────────────────────────

/// Create a specific image by recipe name.
pub async fn create_image(name: &str, image_dir: &Path) -> Result<()> {
    let recipe = RECIPES.iter().find(|r| r.name == name).ok_or_else(|| {
        eg!(format!(
            "unknown image recipe '{name}' (run 'tt image recipes' to list)"
        ))
    })?;

    match recipe.engine {
        "docker" => create_docker(name).await,
        "firecracker" => create_firecracker(name, image_dir).await,
        "qemu" => create_qemu(name, image_dir).await,
        _ => Err(eg!("unsupported engine: {}", recipe.engine)),
    }
}

/// Create all images for a given engine.
pub async fn create_all_for_engine(engine: &str, image_dir: &Path) -> Result<()> {
    let matching: Vec<_> = RECIPES.iter().filter(|r| r.engine == engine).collect();
    if matching.is_empty() {
        return Err(eg!(format!("no recipes for engine '{engine}'")));
    }

    let mut failures = Vec::new();
    for recipe in matching {
        println!("\n--- {}: {} ---", recipe.name, recipe.description);
        if let Err(e) = create_image(recipe.name, image_dir).await {
            failures.push(format!("{}: {e}", recipe.name));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(eg!("image creation failed: {}", failures.join("; ")))
    }
}

/// Create all available images.
pub async fn create_all(image_dir: &Path) -> Result<()> {
    let mut failures = Vec::new();
    for recipe in RECIPES {
        println!("\n--- {}: {} ---", recipe.name, recipe.description);
        if let Err(e) = create_image(recipe.name, image_dir).await {
            failures.push(format!("{}: {e}", recipe.name));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(eg!("image creation failed: {}", failures.join("; ")))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

pub(crate) fn file_digest(path: &Path, sha512: bool) -> Result<String> {
    use sha2::{Digest, Sha256, Sha512};
    use std::io::Read;
    let mut file = std::fs::File::open(path).c(d!("open digest input"))?;
    let mut small = Sha256::new();
    let mut large = Sha512::new();
    let mut buffer = [0; 65536];
    loop {
        let size = file.read(&mut buffer).c(d!("read digest input"))?;
        if size == 0 {
            break;
        }
        if sha512 {
            large.update(&buffer[..size]);
        } else {
            small.update(&buffer[..size]);
        }
    }
    Ok(if sha512 {
        large
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    } else {
        small
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    })
}

fn expected_digest(manifest: &str, name: &str) -> Result<String> {
    manifest
        .lines()
        .find_map(|line| {
            let mut parts = line.split_whitespace();
            let digest = parts.next()?;
            let file = parts.next()?.trim_start_matches('*');
            (file == name
                && matches!(digest.len(), 64 | 128)
                && digest.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| digest.to_ascii_lowercase())
        })
        .ok_or_else(|| eg!("image is missing from upstream checksum manifest"))
}

async fn download_verified(url: &str, dest: &Path, pin: Option<&str>) -> Result<()> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let staging = tempfile::tempdir_in(parent).c(d!("verified download staging"))?;
    let image = staging.path().join("image");
    download_file(url, &image).await?;
    let expected = if let Some(pin) = pin {
        pin.to_owned()
    } else {
        let (base, name) = url
            .rsplit_once('/')
            .ok_or_else(|| eg!("invalid recipe URL"))?;
        let sums = if url.contains("cloud.debian.org/") {
            "SHA512SUMS"
        } else {
            "SHA256SUMS"
        };
        let manifest = staging.path().join("checksums");
        download_file(&format!("{base}/{sums}"), &manifest).await?;
        expected_digest(
            &std::fs::read_to_string(&manifest).c(d!("read image checksums"))?,
            name,
        )?
    };
    let actual = file_digest(&image, expected.len() == 128)?;
    if actual != expected {
        return Err(eg!("download checksum mismatch; image was not published"));
    }
    println!(
        "[image] verified SHA-{}: {actual}",
        if expected.len() == 128 { 512 } else { 256 }
    );
    tokio::fs::rename(&image, dest)
        .await
        .c(d!("publish verified download"))?;
    Ok(())
}

async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let staging = tempfile::NamedTempFile::new_in(parent).c(d!("download staging file"))?;
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "15",
            "--max-time",
            "900",
            "--max-filesize",
            "8589934592",
            "-o",
        ])
        .arg(staging.path())
        .arg(url)
        .kill_on_drop(true)
        .output()
        .await
        .c(d!("curl failed"))?;
    if !output.status.success() {
        return Err(eg!(
            "download {} failed: {}",
            url,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    staging.persist(dest).map_err(|e| eg!(e.to_string()))?;
    Ok(())
}

async fn run_cmd(cmd: &str, args: &[&str]) -> Result<()> {
    let mut command = Command::new(cmd);
    command.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(300), command.output())
        .await
        .c(d!("host command timed out"))?
        .c(d!(format!("run {cmd}")))?;
    if !output.status.success() {
        return Err(eg!(
            "{} failed: {}",
            cmd,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

async fn human_size(path: &Path) -> String {
    tokio::fs::metadata(path)
        .await
        .map(|m| {
            let bytes = m.len();
            if bytes >= 1024 * 1024 {
                format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
            } else if bytes >= 1024 {
                format!("{:.1} KB", bytes as f64 / 1024.0)
            } else {
                format!("{bytes} B")
            }
        })
        .unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_have_unique_names() {
        let mut seen = std::collections::HashSet::new();
        for r in RECIPES {
            assert!(seen.insert(r.name), "duplicate recipe: {}", r.name);
        }
    }

    #[test]
    fn docker_tags_resolve() {
        assert_eq!(docker_tag("alpine"), "alpine:3.21");
        assert_eq!(docker_tag("debian"), "debian:trixie-slim");
        assert_eq!(docker_tag("unknown"), "unknown");
    }

    #[test]
    fn qemu_urls_resolve() {
        assert!(qemu_cloud_url("alpine-cloud").is_some());
        assert!(qemu_cloud_url("debian-cloud").is_some());
        assert!(qemu_cloud_url("ubuntu-cloud").is_some());
        assert!(qemu_cloud_url("nonexistent").is_none());
    }

    #[test]
    fn all_engines_covered() {
        let engines: std::collections::HashSet<&str> = RECIPES.iter().map(|r| r.engine).collect();
        assert!(engines.contains("docker"));
        assert!(engines.contains("firecracker"));
        assert!(engines.contains("qemu"));
        assert_eq!(engines.len(), 3);
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn failed_download_keeps_previous_image_and_success_publishes_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("image");
        std::fs::write(&dest, b"previous").unwrap();
        for (response, success) in [
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial",
                false,
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\ncomplete",
                true,
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/image", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            });
            assert_eq!(download_file(&url, &dest).await.is_ok(), success);
            server.await.unwrap();
            assert_eq!(
                std::fs::read(&dest).unwrap(),
                if success {
                    b"complete".as_slice()
                } else {
                    b"previous".as_slice()
                }
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    #[test]
    fn checksum_manifests_select_the_exact_filename() {
        let expected = "a".repeat(64);
        let manifest = format!("{}  other.img\n{expected} *wanted.img\n", "b".repeat(64));
        assert_eq!(expected_digest(&manifest, "wanted.img").unwrap(), expected);
        assert!(expected_digest(&manifest, "missing.img").is_err());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image");
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            file_digest(&path, false).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[tokio::test]
    async fn checksum_mismatch_never_replaces_an_existing_image() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("image");
        std::fs::write(&dest, b"previous").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc")
                .await
                .unwrap();
        });
        assert!(
            download_verified(&url, &dest, Some(&"0".repeat(64)))
                .await
                .is_err()
        );
        server.await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"previous");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
