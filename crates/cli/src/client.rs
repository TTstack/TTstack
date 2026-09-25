//! HTTP client for communicating with the tt-ctl controller.

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use ruc::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use ttcore::api::ApiResp;

/// Controller API client.
pub struct Client {
    base_url: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(addr: &str, api_key: Option<&str>) -> Result<Self> {
        let base_url = if addr.starts_with("http") {
            addr.to_string()
        } else {
            format!("http://{addr}")
        };

        let mut headers = HeaderMap::new();
        if let Some(key) = api_key {
            ttcore::auth::parse_api_key(key).map_err(|e| eg!(e))?;
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {key}")).c(d!("invalid API key"))?,
            );
        }

        Ok(Self {
            base_url,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .c(d!("build HTTP client"))?,
        })
    }

    /// GET request, returning deserialized data.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .c(d!("read request failed"))?;
        let status = resp.status();
        let body: ApiResp<T> = resp
            .json()
            .await
            .c(d!(format!("invalid response (HTTP {status})")))?;

        if status.is_success() && body.ok {
            body.data.ok_or_else(|| eg!("empty response"))
        } else {
            Err(eg!(body.error.unwrap_or_else(|| format!("HTTP {status}"))))
        }
    }

    /// POST request with JSON body, returning deserialized data.
    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .http
            .post(&url)
            .timeout(std::time::Duration::from_secs(600))
            .json(body)
            .send()
            .await
            .c(d!("request failed; a submitted operation may still be running — inspect the environment before retrying"))?;
        let status = resp.status();
        let body: ApiResp<T> = resp
            .json()
            .await
            .c(d!(format!("invalid response (HTTP {status})")))?;

        if status.is_success() && body.ok {
            body.data.ok_or_else(|| eg!("empty response"))
        } else {
            Err(eg!(body.error.unwrap_or_else(|| format!("HTTP {status}"))))
        }
    }

    /// POST request with no request body, no response body.
    pub async fn post_action(&self, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base_url);
        let resp = self.http.post(&url).timeout(std::time::Duration::from_secs(600)).send().await.c(d!("request failed; a submitted operation may still be running — inspect the environment before retrying"))?;
        let status = resp.status();
        let body: ApiResp<()> = resp
            .json()
            .await
            .c(d!(format!("invalid response (HTTP {status})")))?;

        if status.is_success() && body.ok {
            Ok(())
        } else {
            Err(eg!(body.error.unwrap_or_else(|| format!("HTTP {status}"))))
        }
    }

    /// DELETE request.
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base_url);
        let resp = self
            .http
            .delete(&url)
            .timeout(std::time::Duration::from_secs(600))
            .send()
            .await
            .c(d!("request failed; a submitted operation may still be running — inspect the environment before retrying"))?;
        let status = resp.status();
        let body: ApiResp<()> = resp
            .json()
            .await
            .c(d!(format!("invalid response (HTTP {status})")))?;

        if status.is_success() && body.ok {
            Ok(())
        } else {
            Err(eg!(body.error.unwrap_or_else(|| format!("HTTP {status}"))))
        }
    }
}

// ── Configuration File ──────────────────────────────────────────────

const CONFIG_FILE: &str = ".ttconfig";

/// CLI configuration: controller address and optional API key.
pub struct CliConfig {
    pub addr: String,
    pub api_key: Option<String>,
}

/// Read the CLI config from ~/.ttconfig.
///
/// File format (one value per line):
/// ```text
/// <addr>
/// <api_key>       # optional second line
/// ```
pub fn load_config() -> Option<CliConfig> {
    let path = dirs_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let mut lines = content.lines();
    let addr = lines.next()?.trim().to_string();
    if addr.is_empty() {
        return None;
    }
    let api_key = lines
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Some(CliConfig { addr, api_key })
}

/// Save the controller address and optional API key to ~/.ttconfig.
pub fn save_config(addr: &str, api_key: Option<&str>) -> Result<()> {
    let path = dirs_path()?;
    if addr.contains(['\n', '\r']) || addr.trim().is_empty() {
        return Err(eg!("invalid controller address"));
    }
    let saved = load_config().filter(|c| c.addr == addr);
    let api_key = api_key.or_else(|| saved.as_ref().and_then(|c| c.api_key.as_deref()));
    if let Some(key) = api_key {
        ttcore::auth::parse_api_key(key).map_err(|e| eg!(e))?;
    }
    let content = match api_key {
        Some(key) => format!("{addr}\n{key}\n"),
        None => format!("{addr}\n"),
    };
    save_config_file(std::path::Path::new(&path), &content)
}

fn save_config_file(path: &std::path::Path, content: &str) -> Result<()> {
    use std::io::Write;
    let mut staging =
        tempfile::NamedTempFile::new_in(path.parent().ok_or_else(|| eg!("config has no parent"))?)
            .c(d!("config staging"))?;
    staging
        .write_all(content.as_bytes())
        .c(d!("write config"))?;
    staging.as_file().sync_all().c(d!("sync config"))?;
    staging.persist(path).map_err(|e| eg!(e.to_string()))?;
    Ok(())
}

fn dirs_path() -> Result<String> {
    let home = std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| eg!("HOME must be set to store CLI credentials"))?;
    Ok(format!("{home}/{CONFIG_FILE}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_replacement_is_private_and_atomic() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_config_file(&path, "new\nsecret\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\nsecret\n");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    #[test]
    fn invalid_client_keys_are_rejected() {
        assert!(Client::new("127.0.0.1:9200", Some("bad\nkey")).is_err());
        assert!(Client::new("127.0.0.1:9200", Some("")).is_err());
    }
}
