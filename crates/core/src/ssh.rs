//! One-time SSH bootstrap. Login private keys belong to the caller, never TTstack.
use ruc::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub fn seed_path(id: &str) -> std::path::PathBuf {
    Path::new(crate::model::RUN_DIR).join(format!("ssh-{id}.sh"))
}

pub const SEED_FILE: &str = "ttstack-ssh.sh";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshOptions {
    pub user: String,
    #[serde(default)]
    pub sudo: bool,
}

impl Default for SshOptions {
    fn default() -> Self {
        Self {
            user: "root".into(),
            sudo: false,
        }
    }
}

impl SshOptions {
    pub fn validate(&self) -> std::result::Result<(), String> {
        let bytes = self.user.as_bytes();
        if bytes.is_empty()
            || bytes.len() > 32
            || !bytes[0].is_ascii_lowercase()
            || !bytes
                .iter()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_' || *b == b'-')
        {
            return Err("ssh.user must be a lowercase Unix account name".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshInfo {
    pub user: String,
    pub sudo: bool,
    pub host: String,
    pub port: u16,
    pub host_key: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default)]
    pub initialized: bool,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Generate a host identity in private staging; only its public part goes in VM records.
pub fn bootstrap(options: &SshOptions, keys: &[String]) -> Result<(String, String)> {
    use crate::command::CommandExt;
    use std::process::Command;
    options.validate().map_err(|e| eg!(e))?;
    let staging = tempfile::tempdir().c(d!("SSH host key staging"))?;
    let key = staging.path().join("host");
    let result = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "ttstack-vm", "-f"])
        .arg(&key)
        .bounded_output()
        .c(d!("generate VM host identity"))?;
    if !result.status.success() {
        return Err(eg!("cannot generate VM host identity"));
    }
    let private = std::fs::read_to_string(&key).c(d!("read host identity"))?;
    let public =
        std::fs::read_to_string(key.with_extension("pub")).c(d!("read public host identity"))?;
    Ok((
        script(options, keys, &private, &public),
        public.trim().into(),
    ))
}

fn script(options: &SshOptions, keys: &[String], private: &str, public: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\naccount={}\nsudo_access={}\ninitial_keys={}\nhost_private={}\nhost_public={}\n{}",
        quote(&options.user),
        if options.sudo { "yes" } else { "no" },
        quote(&(keys.join("\n") + "\n")),
        quote(private),
        quote(public),
        include_str!("ssh_bootstrap.sh")
    )
}

/// Only a service banner is observed. User-owned key/config changes are not reconciled.
pub fn ready(ip: &str) -> bool {
    use std::io::Read;
    use std::net::{IpAddr, SocketAddr, TcpStream};
    use std::time::Duration;
    let Ok(ip) = ip.parse::<IpAddr>() else {
        return false;
    };
    let Ok(mut stream) =
        TcpStream::connect_timeout(&SocketAddr::new(ip, 22), Duration::from_millis(200))
    else {
        return false;
    };
    if stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .is_err()
    {
        return false;
    }
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).is_ok() && &prefix == b"SSH-"
}

/// Confirm initial bootstrap using the host identity that only its seed installed.
/// Once confirmed, caller-owned host/key edits are not continuously reconciled.
pub fn initial_identity_ready(ip: &str, public: &str) -> bool {
    use crate::command::CommandExt;
    use std::process::Command;
    if ip.parse::<std::net::IpAddr>().is_err() {
        return false;
    }
    let expected: Vec<_> = public.split_whitespace().take(2).collect();
    let Ok(output) = Command::new("ssh-keyscan")
        .args(["-T", "1", "-t", "ed25519", ip])
        .output_timeout(std::time::Duration::from_secs(2))
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        let fields: Vec<_> = line.split_whitespace().skip(1).take(2).collect();
        expected.len() == 2 && fields == expected
    })
}

/// Inspect a prepared ext4 image without mounting or executing its contents.
pub fn prepared_rootfs(path: &Path) -> Result<()> {
    use crate::command::CommandExt;
    use std::process::Command;
    let output = Command::new("debugfs")
        .args(["-R", "cat /etc/ttstack/ssh-bootstrap-version"])
        .arg(path)
        .bounded_output()
        .c(d!("inspect SSH image support"))?;
    if !output.status.success() || output.stdout.trim_ascii() != b"1" {
        return Err(eg!(
            "Firecracker image requires TTstack SSH bootstrap version 1"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_account_without_echoing_private_inputs() {
        for user in ["", "-root", "user;id", "user\nroot", "../root", "user name"] {
            assert!(
                SshOptions {
                    user: user.into(),
                    sudo: true
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            SshOptions {
                user: "user".into(),
                sudo: true
            }
            .validate()
            .is_ok()
        );
    }
    #[test]
    fn initial_script_quotes_keys_and_preserves_completed_bootstrap() {
        let script = script(
            &SshOptions {
                user: "user".into(),
                sudo: true,
            },
            &["ssh-ed25519 AAAA $(touch unsafe)' comment".into()],
            "private\n",
            "public\n",
        );
        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&script)
            .output()
            .unwrap();
        assert!(output.status.success());
        // Exercise the early-exit branch in a private directory without touching /etc.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("ssh-initialized"), b"1").unwrap();
        let script = script.replace("/var/lib/ttstack", dir.path().to_str().unwrap());
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(!dir.path().join("unsafe").exists());
    }
    #[test]
    fn generates_unique_host_identity_without_returning_it_in_metadata() {
        let (script, public) =
            bootstrap(&SshOptions::default(), &["ssh-ed25519 AAAA test".into()]).unwrap();
        assert!(public.starts_with("ssh-ed25519 "));
        assert!(!public.contains("PRIVATE"));
        assert!(script.contains("OPENSSH PRIVATE KEY"));
        assert_ne!(public, bootstrap(&SshOptions::default(), &[]).unwrap().1);
    }
}
