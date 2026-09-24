//! Agent configuration.

use clap::Parser;
use ttcore::model::Storage;

/// TTstack host agent — manages VMs and containers on this host.
#[derive(Parser, Debug)]
#[command(name = "tt-agent", version)]
pub struct Config {
    /// Listen address for the HTTP API.
    #[arg(long, default_value = "0.0.0.0:9100")]
    pub listen: String,

    /// Base image directory (file) or dataset (zvol); excludes Docker images.
    #[arg(long, default_value = "/home/ttstack/images")]
    pub image_dir: String,

    /// Runtime clone directory (file) or dataset (zvol).
    #[arg(long, default_value = "/home/ttstack/runtime")]
    pub runtime_dir: String,

    /// Directory for persistent state (SQLite database).
    #[arg(long, default_value = "/home/ttstack/data")]
    pub data_dir: String,

    /// Storage backend: file or zvol.
    #[arg(long, default_value = "file")]
    pub storage: Storage,

    /// Total CPU cores available for VMs (0 = auto-detect).
    #[arg(long, default_value_t = 0)]
    pub cpu_total: u32,

    /// Total memory for VMs in MiB (0 = auto-detect).
    #[arg(long, default_value_t = 0)]
    pub mem_total: u32,

    /// Disk scheduling budget in MiB (default 200 GiB); not auto-detected or a quota.
    #[arg(long, default_value_t = 200 * 1024)]
    pub disk_total: u32,

    /// Unique host identifier (generated once and persisted if not set).
    #[arg(long)]
    pub host_id: Option<String>,
    /// API key for authentication. If set, all API requests must include
    /// `Authorization: Bearer <key>`. Can also be provided via TT_API_KEY env var.
    #[arg(long, env = "TT_API_KEY")]
    pub api_key: Option<String>,
}

impl Config {
    pub fn storage_kind(&self) -> Storage {
        self.storage
    }

    /// Auto-detect CPU count if set to 0.
    pub fn effective_cpu(&self) -> u32 {
        if self.cpu_total == 0 {
            std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(4)
        } else {
            self.cpu_total
        }
    }

    /// Auto-detect memory if set to 0 (read from /proc/meminfo).
    pub fn effective_mem(&self) -> u32 {
        if self.mem_total == 0 {
            read_total_mem_mb().unwrap_or(8192)
        } else {
            self.mem_total
        }
    }
}

/// Read total system memory in MiB.
///
/// Uses `/proc/meminfo` on Linux, `sysctl hw.physmem` on FreeBSD,
/// `sysctl hw.memsize` on macOS.
fn read_total_mem_mb() -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse_mem_total(&content)
    }
    #[cfg(target_os = "freebsd")]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.physmem")
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        return Some((bytes / (1024 * 1024)) as u32);
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let output = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output()
            .ok()?;
        let bytes: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;
        Some((bytes / (1024 * 1024)) as u32)
    }
}

/// Parse MemTotal from /proc/meminfo content.
#[cfg(any(target_os = "linux", test))]
fn parse_mem_total(content: &str) -> Option<u32> {
    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
            return Some((kb / 1024) as u32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_meminfo() {
        let content = "\
MemTotal:       131696312 kB
MemFree:         1234567 kB
MemAvailable:    9876543 kB";
        assert_eq!(parse_mem_total(content), Some(131696312 / 1024));
    }

    #[test]
    fn parse_meminfo_missing() {
        assert_eq!(parse_mem_total("nothing here"), None);
    }

    #[test]
    fn parse_meminfo_malformed() {
        assert_eq!(parse_mem_total("MemTotal: notanumber kB"), None);
    }

    #[test]
    fn storage_kind_default() {
        // Default is "file"
        let cfg = Config::parse_from(["tt-agent"]);
        assert_eq!(cfg.storage_kind(), Storage::File);
    }

    #[test]
    fn storage_kind_zvol() {
        let cfg = Config::parse_from(["tt-agent", "--storage", "zvol"]);
        assert_eq!(cfg.storage_kind(), Storage::Zvol);
    }

    #[test]
    fn storage_kind_invalid_is_rejected() {
        assert!(Config::try_parse_from(["tt-agent", "--storage", "foo"]).is_err());
    }

    #[test]
    fn effective_cpu_explicit() {
        let cfg = Config::parse_from(["tt-agent", "--cpu-total", "16"]);
        assert_eq!(cfg.effective_cpu(), 16);
    }

    #[test]
    fn effective_cpu_auto() {
        let cfg = Config::parse_from(["tt-agent"]);
        // Auto-detect: should be > 0
        assert!(cfg.effective_cpu() > 0);
    }

    #[test]
    fn effective_mem_explicit() {
        let cfg = Config::parse_from(["tt-agent", "--mem-total", "4096"]);
        assert_eq!(cfg.effective_mem(), 4096);
    }

    #[test]
    fn effective_mem_auto() {
        let cfg = Config::parse_from(["tt-agent"]);
        // On any real machine, should detect > 0
        assert!(cfg.effective_mem() > 0);
    }
}
