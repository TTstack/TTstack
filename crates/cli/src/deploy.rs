//! Deployment embedded in the `tt` CLI.
//!
//! Local deployment requires Linux/systemd; distributed deployment detects
//! systemd or OpenRC and uses sudo on targets, including for root SSH users.
//! Copies binaries and configures services; engine/image setup is separate.

use ruc::*;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::process::Command;

// ── Deploy config (deploy.toml) ─────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DeployConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub controller: Option<ControllerConfig>,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
}

#[derive(Debug, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_prefix")]
    pub prefix: String,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default = "default_release_dir")]
    pub release_dir: String,
    /// Shared controller/agent key. If omitted, reuse/import a key before generating one.
    pub api_key: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            prefix: default_prefix(),
            user: default_user(),
            release_dir: default_release_dir(),
            api_key: None,
        }
    }
}

fn generate_api_key() -> String {
    format!(
        "tt-{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn parse_api_key(content: &str) -> Result<String> {
    let key = content
        .lines()
        .find_map(|line| line.strip_prefix("TT_API_KEY="))
        .unwrap_or(content.trim());
    if key.is_empty()
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err(eg!(
            "deployment API key must contain only letters, digits, '-', '_' or '.'"
        ));
    }
    Ok(key.to_string())
}

fn save_secret(path: &Path, key: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).c(d!("create key directory"))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).c(d!("create API key file"))?;
    writeln!(file, "{key}").c(d!("save API key"))?;
    file.sync_all().c(d!("sync API key"))?;
    Ok(())
}

/// Migrate existing service keys once, then share one stable key across local roles.
fn local_api_key(etc: &Path) -> Result<String> {
    let shared = etc.join("api-key");
    if shared.exists() {
        return parse_api_key(&std::fs::read_to_string(shared).c(d!("read API key"))?);
    }
    let mut keys = Vec::new();
    for name in ["tt-agent.env", "tt-ctl.env"] {
        let path = etc.join(name);
        match std::fs::read_to_string(path) {
            Ok(content) => keys.push(parse_api_key(&content)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).c(d!("read existing service key")),
        }
    }
    keys.sort();
    keys.dedup();
    if keys.len() > 1 {
        return Err(eg!(
            "existing agent/controller keys differ; put the intended shared key in {}/api-key and deploy all",
            etc.display()
        ));
    }
    let key = keys.pop().unwrap_or_else(generate_api_key);
    save_secret(&shared, &key)?;
    Ok(key)
}

#[derive(Debug, Deserialize)]
pub struct ControllerConfig {
    pub host: String,
    #[serde(default = "default_ssh_user")]
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default = "default_ctl_listen")]
    pub listen: String,
    pub data_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AgentConfig {
    pub host: String,
    #[serde(default = "default_ssh_user")]
    pub ssh_user: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default = "default_agent_listen")]
    pub listen: String,
    #[serde(default = "default_storage")]
    pub storage: String,
    pub image_dir: Option<String>,
    pub runtime_dir: Option<String>,
    #[serde(default)]
    pub cpu_total: u32,
    #[serde(default)]
    pub mem_total: u32,
    #[serde(default = "default_disk_total")]
    pub disk_total: String,
    pub host_id: Option<String>,
    /// Override release_dir for this agent (e.g. for musl/cross-compiled binaries).
    pub release_dir: Option<String>,
}

fn default_prefix() -> String {
    "/opt/ttstack".into()
}
fn default_user() -> String {
    "ttstack".into()
}
fn default_release_dir() -> String {
    "./target/release".into()
}
fn default_ssh_user() -> String {
    "root".into()
}
fn default_ssh_port() -> u16 {
    22
}
fn default_ctl_listen() -> String {
    "0.0.0.0:9200".into()
}
fn default_agent_listen() -> String {
    "0.0.0.0:9100".into()
}
fn default_storage() -> String {
    "file".into()
}
fn default_disk_total() -> String {
    "200G".into()
}

/// Parse disk_total: "200G" → 204800 (MiB), plain number passes through.
fn parse_disk(val: &str) -> Result<u32> {
    let val = val.trim().trim_matches('"');
    let (number, factor) = match val.strip_suffix(['g', 'G']) {
        Some(number) => (number, 1024),
        None => (val, 1),
    };
    let number: u32 = number.parse().c(d!("invalid disk_total"))?;
    number
        .checked_mul(factor)
        .filter(|n| *n > 0)
        .ok_or_else(|| eg!("disk_total must be positive and fit in MiB"))
}

// ── SSH helpers ─────────────────────────────────────────────────────

struct SshTarget {
    user: String,
    host: String,
    port: u16,
}

impl SshTarget {
    async fn exec(&self, cmd: &str) -> Result<String> {
        let output = Command::new("ssh")
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=10",
                "-p",
                &self.port.to_string(),
            ])
            .arg(format!("{}@{}", self.user, self.host))
            .arg(cmd)
            .output()
            .await
            .c(d!("ssh exec failed"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("ssh command failed on {}: {}", self.host, stderr));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn copy(&self, local: &Path, remote: &str) -> Result<()> {
        let output = Command::new("scp")
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "ConnectTimeout=10",
                "-P",
                &self.port.to_string(),
            ])
            .arg(local)
            .arg(format!("{}@{}:{}", self.user, self.host, remote))
            .output()
            .await
            .c(d!("scp failed"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eg!("scp to {} failed: {}", self.host, stderr));
        }
        Ok(())
    }
}

// ── Service templates ───────────────────────────────────────────────

fn systemd_unit(name: &str, exec_start: &str, run_as_root: bool, env_file: Option<&str>) -> String {
    let user_lines = if run_as_root {
        "# VM processes must survive agent restarts\nKillMode=process\nTimeoutStopSec=360"
            .to_string()
    } else {
        "User=ttstack\nGroup=ttstack".to_string()
    };

    let env_file_line = match env_file {
        Some(path) => format!("EnvironmentFile={path}\n"),
        None => String::new(),
    };

    format!(
        r#"[Unit]
Description=TTstack {name}
After=network.target

[Service]
Type=simple
{user_lines}
{env_file_line}ExecStart={exec_start}
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"#
    )
}

fn openrc_initd(name: &str, exec_start: &str) -> String {
    // Split exec_start into command and args for OpenRC
    let mut parts = exec_start.splitn(2, ' ');
    let cmd = parts.next().unwrap_or(exec_start);
    let args = parts.next().unwrap_or("");

    format!(
        r#"#!/sbin/openrc-run

name="{name}"
description="TTstack {name}"
command="{cmd}"
command_args="{args}"
command_background=true
pidfile="/run/${{name}}.pid"
output_log="/var/log/${{name}}.log"
error_log="/var/log/${{name}}.log"

depend() {{
    need net
    after firewall
}}
"#
    )
}

/// Generate the platform-aware remote setup script.
///
/// Uses `sudo` for all privileged operations, detects init system,
/// and handles both glibc (useradd) and busybox (adduser) user creation.
#[allow(clippy::too_many_arguments)]
fn remote_setup_script(
    service_name: &str,
    exec_cmd: &str,
    user: &str,
    home: &str,
    prefix: &str,
    tmp: &str,
    binaries: &[&str],
    extra_dirs: &[&str],
    host_label: &str,
    env_file: Option<(&str, &str)>,
) -> String {
    let bin_copies: String = binaries
        .iter()
        .map(|b| format!("sudo cp {tmp}/{b} {prefix}/bin/{b}\nsudo chmod 755 {prefix}/bin/{b}",))
        .collect::<Vec<_>>()
        .join("\n");

    let dir_list: String = extra_dirs
        .iter()
        .map(|d| d.to_string())
        .chain([format!("{home}/data"), format!("{home}/run")])
        .collect::<Vec<_>>()
        .join(" ");

    let env_file_path = env_file.map(|(p, _)| p);
    let systemd_unit_content = systemd_unit(
        service_name,
        exec_cmd,
        service_name == "tt-agent",
        env_file_path,
    )
    .replace("User=ttstack", &format!("User={user}"))
    .replace("Group=ttstack", &format!("Group={user}"));
    let mut openrc_initd_content = openrc_initd(service_name, exec_cmd);
    if let Some(path) = env_file_path {
        openrc_initd_content.push_str(&format!(
            "\nstart_pre() {{\n    . {path}\n    export TT_API_KEY\n}}\n"
        ));
    }
    if service_name == "tt-ctl" {
        openrc_initd_content.push_str(&format!("\ncommand_user=\"{user}:{user}\"\n"));
    }

    // Shell snippet that writes the env file with 0600 permissions
    let env_file_setup = match env_file {
        Some((path, content)) => format!(
            r#"
# Write environment file (keeps secrets out of process args)
sudo mkdir -p $(dirname {path})
sudo install -m 600 /dev/null {path}
printf '%s' '{content}' | sudo tee {path} > /dev/null
sudo chmod 600 {path}
"#,
            path = path,
            content = content,
        ),
        None => String::new(),
    };

    // For OpenRC / manual fallback, source env file before exec
    let env_source = match env_file {
        Some((path, _)) => format!(". {path} && export TT_API_KEY && "),
        None => String::new(),
    };

    format!(
        r#"#!/bin/sh
set -e

if [ "$(uname -s)" != "Linux" ]; then
    echo "deployment requires a Linux host" >&2
    exit 1
fi

# Create service user (portable: works on glibc and busybox)
if id {user} >/dev/null 2>&1; then
    echo "[deploy] user '{user}' exists"
else
    echo "[deploy] creating user '{user}'"
    if command -v useradd >/dev/null 2>&1; then
        sudo useradd -r -m -d {home} -s /bin/sh {user}
    elif command -v adduser >/dev/null 2>&1; then
        sudo adduser -D -h {home} -s /bin/sh {user} 2>/dev/null || true
    fi
fi

# Create directories
sudo mkdir -p {prefix}/bin {prefix}/etc {dir_list}

# Install binaries
{bin_copies}

# Set ownership
sudo chown -R {user}:{user} {home} 2>/dev/null || true
{env_file_setup}
# Detect init system and install service
if command -v systemctl >/dev/null 2>&1 && [ -d /etc/systemd/system ]; then
    echo "[deploy] using systemd"
    sudo tee /etc/systemd/system/{service_name}.service > /dev/null <<'UNIT'
{systemd_unit}
UNIT
    sudo systemctl daemon-reload
    sudo systemctl enable {service_name}
    sudo systemctl restart {service_name}
    sleep 1
    sudo systemctl is-active {service_name} && echo "[deploy] {service_name} is active"
elif command -v rc-service >/dev/null 2>&1; then
    echo "[deploy] using OpenRC"
    sudo tee /etc/init.d/{service_name} > /dev/null <<'INITD'
{openrc_initd}
INITD
    sudo chmod 755 /etc/init.d/{service_name}
    sudo rc-update add {service_name} default 2>/dev/null || true
    sudo rc-service {service_name} restart 2>/dev/null || sudo rc-service {service_name} start
    sleep 1
    sudo rc-service {service_name} status && echo "[deploy] {service_name} is running"
else
    echo "[deploy] WARNING: no known init system; starting {service_name} manually"
    sudo pkill -f '{prefix}/bin/{service_name}' 2>/dev/null || true
    sleep 1
    sudo nohup sh -c '{env_source}{exec_cmd}' > /var/log/{service_name}.log 2>&1 &
    sleep 2
    pgrep -f '{prefix}/bin/{service_name}' && echo "[deploy] {service_name} started (manual)"
fi

# Cleanup
rm -rf {tmp}
echo "[deploy] {service_name} deployed on {host_label}"
"#,
        user = user,
        home = home,
        prefix = prefix,
        tmp = tmp,
        dir_list = dir_list,
        bin_copies = bin_copies,
        service_name = service_name,
        exec_cmd = exec_cmd,
        systemd_unit = systemd_unit_content,
        openrc_initd = openrc_initd_content,
        host_label = host_label,
        env_file_setup = env_file_setup,
        env_source = env_source,
    )
}

// ── Local deploy ────────────────────────────────────────────────────

async fn local_ensure_user(user: &str) -> Result<()> {
    let check = Command::new("id")
        .arg(user)
        .output()
        .await
        .c(d!("check user"))?;

    if check.status.success() {
        println!("[deploy] user '{user}' exists");
    } else {
        println!("[deploy] creating user '{user}'");
        let out = Command::new("useradd")
            .args([
                "-r",
                "-m",
                "-d",
                &format!("/home/{user}"),
                "-s",
                "/bin/sh",
                user,
            ])
            .output()
            .await
            .c(d!("create user"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(eg!("useradd failed: {}", stderr));
        }
    }
    Ok(())
}

async fn local_ensure_dirs(home: &str, user: &str) -> Result<()> {
    for dir in ["images", "runtime", "data", "ctl", "run"] {
        let path = format!("{home}/{dir}");
        tokio::fs::create_dir_all(&path).await.c(d!("mkdir"))?;
    }
    Command::new("chown")
        .args(["-R", &format!("{user}:{user}"), home])
        .output()
        .await
        .c(d!("chown"))?;
    Ok(())
}

async fn local_install_bin(src: &Path, prefix: &str) -> Result<()> {
    let bin_dir = format!("{prefix}/bin");
    tokio::fs::create_dir_all(&bin_dir)
        .await
        .c(d!("mkdir bin"))?;

    let name = src.file_name().unwrap().to_str().unwrap();
    let dst = format!("{bin_dir}/{name}");
    tokio::fs::copy(src, &dst).await.c(d!("copy binary"))?;

    Command::new("chmod")
        .args(["755", &dst])
        .output()
        .await
        .c(d!("chmod"))?;

    println!("[deploy] installed {dst}");
    Ok(())
}

async fn local_install_systemd(
    name: &str,
    exec_start: &str,
    run_as_root: bool,
    env_file: Option<(&str, &str)>,
) -> Result<()> {
    // Write optional environment file with restricted permissions
    if let Some((path, content)) = env_file {
        if let Some(parent) = std::path::Path::new(path).parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .c(d!("create env dir"))?;
        }
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path).c(d!("open env file"))?;
            file.write_all(content.as_bytes()).c(d!("write env file"))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }

    let env_path = env_file.map(|(p, _)| p);
    let unit = systemd_unit(name, exec_start, run_as_root, env_path);
    let path = format!("/etc/systemd/system/{name}.service");
    tokio::fs::write(&path, &unit).await.c(d!("write unit"))?;

    let output = Command::new("systemctl")
        .arg("daemon-reload")
        .output()
        .await
        .c(d!("daemon-reload"))?;
    check_output(output, "systemctl daemon-reload")?;
    let output = Command::new("systemctl")
        .args(["enable", name])
        .output()
        .await
        .c(d!("enable service"))?;
    check_output(output, "systemctl enable")?;

    println!("[deploy] unit {name} installed");
    Ok(())
}

async fn local_restart_service(name: &str) -> Result<()> {
    let active = Command::new("systemctl")
        .args(["is-active", "--quiet", name])
        .output()
        .await;

    let action = if active.map(|o| o.status.success()).unwrap_or(false) {
        "restart"
    } else {
        "start"
    };

    let output = Command::new("systemctl")
        .args([action, name])
        .output()
        .await
        .c(d!("restart service"))?;
    check_output(output, "systemctl start/restart")?;

    let status = Command::new("systemctl")
        .args(["is-active", name])
        .output()
        .await
        .c(d!("check status"))?;
    if !status.status.success() {
        return Err(eg!(
            "service {} is not active: {}",
            name,
            String::from_utf8_lossy(&status.stderr)
        ));
    }
    let state = String::from_utf8_lossy(&status.stdout);
    println!("[deploy] {name} is {}", state.trim());
    Ok(())
}

fn check_output(output: std::process::Output, action: &str) -> Result<()> {
    if !output.status.success() {
        return Err(eg!(
            "{} failed: {}",
            action,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

// ── Deploy entry points ─────────────────────────────────────────────

/// Deploy locally on this host (requires root).
pub async fn deploy_local(role: &str, release_dir: &str) -> Result<()> {
    if !cfg!(target_os = "linux") || !Path::new("/run/systemd/system").is_dir() {
        return Err(eg!(
            "local deploy requires Linux with systemd; use distributed deploy for Linux with OpenRC. Experimental FreeBSD hosts require manual setup"
        ));
    }
    let uid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u32>().ok())
        })
        .unwrap_or(1000);
    if uid != 0 {
        return Err(eg!("local deploy requires root (run with sudo)"));
    }

    let prefix = "/opt/ttstack";
    let user = "ttstack";
    let home = "/home/ttstack";

    local_ensure_user(user).await?;
    local_ensure_dirs(home, user).await?;

    let release = PathBuf::from(release_dir);
    let api_key = local_api_key(Path::new(prefix).join("etc").as_path())?;
    let env_content = format!("TT_API_KEY={api_key}\n");

    match role {
        "agent" | "all" => {
            let bin = release.join("tt-agent");
            if !bin.exists() {
                return Err(eg!(
                    "{} not found (run 'cargo build --release' first)",
                    bin.display()
                ));
            }
            local_install_bin(&bin, prefix).await?;

            let cmd = format!(
                "{prefix}/bin/tt-agent --listen 0.0.0.0:9100 \
                 --image-dir {home}/images --runtime-dir {home}/runtime \
                 --data-dir {home}/data --storage file"
            );
            let env_path = format!("{prefix}/etc/tt-agent.env");
            local_install_systemd("tt-agent", &cmd, true, Some((&env_path, &env_content))).await?;
            local_restart_service("tt-agent").await?;
        }
        _ => {}
    }

    match role {
        "ctl" | "all" => {
            for name in ["tt-ctl", "tt"] {
                let bin = release.join(name);
                if !bin.exists() {
                    return Err(eg!("{} not found", bin.display()));
                }
                local_install_bin(&bin, prefix).await?;
            }

            let cmd = format!("{prefix}/bin/tt-ctl --listen 0.0.0.0:9200 --data-dir {home}/ctl");
            let env_path = format!("{prefix}/etc/tt-ctl.env");
            local_install_systemd("tt-ctl", &cmd, false, Some((&env_path, &env_content))).await?;
            local_restart_service("tt-ctl").await?;

            println!("[deploy] API key: {api_key}");
            println!("[deploy] Run: tt config 127.0.0.1:9200 --api-key {api_key}");
        }
        _ => {}
    }

    println!("[deploy] local deploy complete");
    Ok(())
}

/// Distributed deploy from a config file.
pub async fn deploy_distributed(config_path: &str) -> Result<()> {
    let content = std::fs::read_to_string(config_path).c(d!("read config"))?;
    let cfg: DeployConfig =
        toml::from_str(&content).map_err(|e| eg!(format!("parse deploy.toml: {e}")))?;

    // Validate the entire fleet configuration before changing any remote host.
    for agent in &cfg.agents {
        parse_disk(&agent.disk_total)?;
        agent
            .storage
            .parse::<ttcore::model::Storage>()
            .map_err(|e| eg!(e))?;
    }
    let release_dir = PathBuf::from(&cfg.general.release_dir);
    let prefix = &cfg.general.prefix;
    let user = &cfg.general.user;
    let home = format!("/home/{user}");

    // Verify local binaries exist
    for bin in ["tt", "tt-ctl", "tt-agent"] {
        let p = release_dir.join(bin);
        if !p.exists() {
            return Err(eg!(
                "{} not found (run 'cargo build --release')",
                p.display()
            ));
        }
    }

    let sidecar = PathBuf::from(format!("{config_path}.api-key"));
    let api_key = if let Some(ref key) = cfg.general.api_key {
        use std::io::Write;
        let key = parse_api_key(key)?;
        let parent = sidecar
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut staging = tempfile::NamedTempFile::new_in(parent).c(d!("API key staging file"))?;
        writeln!(staging, "{key}").c(d!("save configured API key"))?;
        staging
            .as_file()
            .sync_all()
            .c(d!("sync configured API key"))?;
        staging.persist(&sidecar).map_err(|e| eg!(e.to_string()))?;
        key
    } else if sidecar.exists() {
        parse_api_key(&std::fs::read_to_string(&sidecar).c(d!("read deployment key"))?)?
    } else {
        let target = if let Some(ctl) = &cfg.controller {
            SshTarget {
                user: ctl.ssh_user.clone(),
                host: ctl.host.clone(),
                port: ctl.ssh_port,
            }
        } else if let Some(agent) = cfg.agents.first() {
            SshTarget {
                user: agent.ssh_user.clone(),
                host: agent.host.clone(),
                port: agent.ssh_port,
            }
        } else {
            return Err(eg!("deploy config has no controller or agents"));
        };
        let previous = target.exec(&format!("sudo sh -c 'if [ -f {prefix}/etc/tt-ctl.env ]; then cat {prefix}/etc/tt-ctl.env; elif [ -f {prefix}/etc/tt-agent.env ]; then cat {prefix}/etc/tt-agent.env; fi'" )).await?;
        let key = if previous.trim().is_empty() {
            generate_api_key()
        } else {
            parse_api_key(&previous)?
        };
        save_secret(&sidecar, &key)?;
        key
    };
    let env_content = format!("TT_API_KEY={api_key}\n");

    // Deploy controller
    if let Some(ctl) = &cfg.controller {
        println!("\n[deploy] === {} (controller) ===", ctl.host);
        let target = SshTarget {
            user: ctl.ssh_user.clone(),
            host: ctl.host.clone(),
            port: ctl.ssh_port,
        };

        let tmp = format!("/tmp/ttstack-deploy-{}", std::process::id());
        target.exec(&format!("mkdir -p {tmp}")).await?;

        for bin in ["tt-ctl", "tt"] {
            target
                .copy(&release_dir.join(bin), &format!("{tmp}/{bin}"))
                .await?;
            println!("[deploy] uploaded {bin}");
        }

        let data_dir = ctl
            .data_dir
            .clone()
            .unwrap_or_else(|| format!("{home}/ctl"));
        let exec_cmd = format!(
            "{prefix}/bin/tt-ctl --listen {listen} --data-dir {data_dir}",
            listen = ctl.listen,
        );
        let env_path = format!("{prefix}/etc/tt-ctl.env");

        let script = remote_setup_script(
            "tt-ctl",
            &exec_cmd,
            user,
            &home,
            prefix,
            &tmp,
            &["tt-ctl", "tt"],
            &[format!("{home}/ctl").as_str()],
            &ctl.host,
            Some((&env_path, &env_content)),
        );
        let out = target.exec(&script).await?;
        print!("{out}");
    }

    // Deploy agents
    for (i, agent) in cfg.agents.iter().enumerate() {
        println!("\n[deploy] === {} (agent {}) ===", agent.host, i);
        let target = SshTarget {
            user: agent.ssh_user.clone(),
            host: agent.host.clone(),
            port: agent.ssh_port,
        };

        let tmp = format!("/tmp/ttstack-deploy-{}", std::process::id());
        target.exec(&format!("mkdir -p {tmp}")).await?;

        let agent_release = agent
            .release_dir
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| release_dir.clone());
        target
            .copy(&agent_release.join("tt-agent"), &format!("{tmp}/tt-agent"))
            .await?;
        println!("[deploy] uploaded tt-agent");

        let image_dir = agent
            .image_dir
            .clone()
            .unwrap_or_else(|| format!("{home}/images"));
        let runtime_dir = agent
            .runtime_dir
            .clone()
            .unwrap_or_else(|| format!("{home}/runtime"));
        let disk = parse_disk(&agent.disk_total)?;

        let mut exec_cmd = format!(
            "{prefix}/bin/tt-agent --listen {listen} \
             --image-dir {image_dir} --runtime-dir {runtime_dir} \
             --data-dir {home}/data --storage {storage} \
             --cpu-total {cpu} --mem-total {mem} --disk-total {disk}",
            listen = agent.listen,
            storage = agent.storage,
            cpu = agent.cpu_total,
            mem = agent.mem_total,
        );
        if let Some(hid) = &agent.host_id {
            exec_cmd.push_str(&format!(" --host-id {hid}"));
        }
        let env_path = format!("{prefix}/etc/tt-agent.env");

        let script = remote_setup_script(
            "tt-agent",
            &exec_cmd,
            user,
            &home,
            prefix,
            &tmp,
            &["tt-agent"],
            &[image_dir.as_str(), runtime_dir.as_str()],
            &agent.host,
            Some((&env_path, &env_content)),
        );
        let out = target.exec(&script).await?;
        print!("{out}");
    }

    println!("\n[deploy] distributed deployment complete");
    println!("[deploy] API key: {api_key}");
    if let Some(ctl) = &cfg.controller {
        println!(
            "[deploy] Run: tt config {}:{} --api-key {api_key}",
            ctl.host,
            ctl.listen.rsplit(':').next().unwrap_or("9200")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_disk_gib() {
        assert_eq!(parse_disk("200G").unwrap(), 204800);
        assert_eq!(parse_disk("100g").unwrap(), 102400);
        assert_eq!(parse_disk("\"500G\"").unwrap(), 512000);
    }

    #[test]
    fn parse_disk_mib() {
        assert_eq!(parse_disk("204800").unwrap(), 204800);
        assert_eq!(parse_disk("1024").unwrap(), 1024);
    }

    #[test]
    fn parse_config_minimal() {
        let toml_str = r#"
[[agents]]
host = "10.0.0.2"
"#;
        let cfg: DeployConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.controller.is_none());
        assert_eq!(cfg.agents.len(), 1);
        assert_eq!(cfg.agents[0].host, "10.0.0.2");
        assert_eq!(cfg.agents[0].ssh_port, 22);
        assert_eq!(cfg.agents[0].storage, "file");
    }

    #[test]
    fn parse_config_full() {
        let toml_str = r#"
[general]
prefix = "/opt/tt"
user = "myuser"
api_key = "my-secret-key"

[controller]
host = "10.0.0.1"
listen = "0.0.0.0:9200"

[[agents]]
host = "10.0.0.2"
storage = "zvol"
host_id = "node-a"
cpu_total = 32
mem_total = 65536
disk_total = "1000G"
image_dir = "tank/images"
runtime_dir = "tank/runtime"

[[agents]]
host = "10.0.0.3"
"#;
        let cfg: DeployConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.general.prefix, "/opt/tt");
        assert_eq!(cfg.general.api_key.as_deref(), Some("my-secret-key"));
        assert!(cfg.controller.is_some());
        assert_eq!(cfg.agents.len(), 2);
        assert_eq!(cfg.agents[0].storage, "zvol");
        assert_eq!(cfg.agents[0].host_id.as_deref(), Some("node-a"));
        assert_eq!(parse_disk(&cfg.agents[0].disk_total).unwrap(), 1024000);
    }

    #[test]
    fn remote_script_contains_sudo() {
        let script = remote_setup_script(
            "tt-agent",
            "/opt/tt/bin/tt-agent --listen 0.0.0.0:9100",
            "ttstack",
            "/home/ttstack",
            "/opt/tt",
            "/tmp/deploy-1",
            &["tt-agent"],
            &["/home/ttstack/images"],
            "testhost",
            Some(("/opt/tt/etc/tt-agent.env", "TT_API_KEY=test-key\n")),
        );
        assert!(script.contains("sudo"));
        assert!(script.contains("rc-service") || script.contains("systemctl"));
        assert!(script.contains("adduser") || script.contains("useradd"));
        assert!(script.contains("EnvironmentFile=/opt/tt/etc/tt-agent.env"));
        assert!(script.contains("TT_API_KEY=test-key"));
        assert!(!script.contains("--api-key"));

        // Refuse unsupported hosts before creating users, paths or services.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let tools = tempfile::tempdir().unwrap();
            let uname = tools.path().join("uname");
            std::fs::write(&uname, "#!/bin/sh\necho UnsupportedOS\n").unwrap();
            std::fs::set_permissions(&uname, std::fs::Permissions::from_mode(0o755)).unwrap();
            let result = std::process::Command::new("/bin/sh")
                .args(["-c", &script])
                .env("PATH", tools.path())
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
            assert_eq!(
                String::from_utf8_lossy(&result.stderr),
                "deployment requires a Linux host\n"
            );
        }
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    #[test]
    fn deployments_reuse_existing_role_key_and_persist_shared_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tt-agent.env"), "TT_API_KEY=existing-key\n").unwrap();
        assert_eq!(local_api_key(dir.path()).unwrap(), "existing-key");
        assert_eq!(local_api_key(dir.path()).unwrap(), "existing-key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join("api-key"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn mismatched_role_keys_are_not_silently_rotated() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tt-agent.env"), "TT_API_KEY=agent-key\n").unwrap();
        std::fs::write(dir.path().join("tt-ctl.env"), "TT_API_KEY=ctl-key\n").unwrap();
        assert!(local_api_key(dir.path()).is_err());
        assert!(!dir.path().join("api-key").exists());
    }
    #[test]
    fn invalid_disk_budgets_do_not_silently_fall_back_or_overflow() {
        for value in ["oops", "0", "4294967295G"] {
            assert!(parse_disk(value).is_err());
        }
    }
}
