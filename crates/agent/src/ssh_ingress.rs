//! Optional outer-namespace ingress for SSH only. Existing management/app ports stay private.
use ruc::*;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::Command;
use ttcore::command::CommandExt;
use ttcore::model::Vm;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SshIngress {
    pub public_address: Ipv4Addr,
    pub namespace: Option<PathBuf>,
    pub target: Option<Ipv4Addr>,
}

impl SshIngress {
    pub fn validate(&self) -> Result<()> {
        if !cfg!(target_os = "linux") && self.namespace.is_some() {
            return Err(eg!("outer SSH network namespaces require Linux"));
        }
        if self.namespace.is_some() != self.target.is_some() {
            return Err(eg!(
                "SSH ingress requires both namespace and target address"
            ));
        }
        if self
            .namespace
            .as_ref()
            .is_some_and(|p| !p.is_absolute() || !p.exists())
        {
            return Err(eg!(
                "SSH ingress namespace must be an existing absolute namespace path"
            ));
        }
        Ok(())
    }

    fn command(&self) -> Command {
        if let Some(namespace) = &self.namespace {
            let mut cmd = Command::new("nsenter");
            cmd.arg(format!("--net={}", namespace.display()))
                .args(["--", "nft"]);
            cmd
        } else {
            Command::new("nft")
        }
    }

    fn rules(&self) -> Result<Option<serde_json::Value>> {
        let output = self
            .command()
            .args(["-j", "list", "tables"])
            .bounded_output()
            .c(d!("inspect SSH ingress tables"))?;
        if !output.status.success() {
            return Err(eg!("cannot inspect SSH ingress tables"));
        }
        let tables: serde_json::Value =
            serde_json::from_slice(&output.stdout).c(d!("decode SSH ingress tables"))?;
        let exists = tables["nftables"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["table"]["name"] == "tt-ssh" && item["table"]["family"] == "ip")
        });
        if !exists {
            return Ok(None);
        }
        let output = self
            .command()
            .args(["-j", "list", "table", "ip", "tt-ssh"])
            .bounded_output()
            .c(d!("inspect SSH ingress rules"))?;
        if !output.status.success() {
            return Err(eg!("cannot inspect SSH ingress rules"));
        }
        Ok(Some(
            serde_json::from_slice(&output.stdout).c(d!("decode SSH ingress rules"))?,
        ))
    }

    fn write(&self, script: String) -> Result<()> {
        if script.is_empty() {
            return Ok(());
        }
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().c(d!("SSH ingress transaction"))?;
        file.write_all(script.as_bytes())
            .c(d!("write SSH ingress transaction"))?;
        let result = self
            .command()
            .arg("-f")
            .arg(file.path())
            .bounded_output()
            .c(d!("apply SSH ingress"))?;
        if !result.status.success() {
            return Err(eg!("cannot apply SSH ingress transaction"));
        }
        Ok(())
    }

    pub fn apply(&self, vm: &Vm) -> Result<()> {
        if self.namespace.is_none() || vm.ssh.is_none() {
            return Ok(());
        }
        let port = *vm
            .port_map
            .get(&22)
            .ok_or_else(|| eg!("SSH guest port is not mapped"))?;
        let rules = self.rules()?;
        let mut script = String::new();
        if rules.is_none() {
            script.push_str("add table ip tt-ssh\nadd chain ip tt-ssh prerouting { type nat hook prerouting priority -105; policy accept; }\n");
        }
        script.push_str(&remove_rules(rules.as_ref(), &vm.id));
        script.push_str(&format!(
            "add rule ip tt-ssh prerouting ip daddr {} tcp dport {} dnat to {}:{} comment {}\n",
            self.public_address,
            port,
            self.target.unwrap(),
            port,
            serde_json::to_string(&format!("ttstack:ssh:{}", vm.id)).unwrap()
        ));
        self.write(script)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        if self.namespace.is_none() {
            return Ok(());
        }
        self.write(remove_rules(self.rules()?.as_ref(), id))
    }
}

fn remove_rules(rules: Option<&serde_json::Value>, id: &str) -> String {
    let mut script = String::new();
    if let Some(items) = rules.and_then(|r| r["nftables"].as_array()) {
        for item in items {
            let rule = &item["rule"];
            if rule["family"] == "ip"
                && rule["table"] == "tt-ssh"
                && rule["chain"] == "prerouting"
                && rule["comment"] == format!("ttstack:ssh:{id}")
                && let Some(handle) = rule["handle"].as_u64()
            {
                script.push_str(&format!(
                    "delete rule ip tt-ssh prerouting handle {handle}\n"
                ));
            }
        }
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletes_only_the_exact_owned_mapping() {
        let rules = serde_json::json!({"nftables": [
            {"rule":{"family":"ip","table":"tt-ssh","chain":"prerouting","handle":1,"comment":"ttstack:ssh:mine"}},
            {"rule":{"family":"ip","table":"tt-ssh","chain":"prerouting","handle":2,"comment":"ttstack:ssh:other"}},
            {"rule":{"family":"ip","table":"other","chain":"prerouting","handle":3,"comment":"ttstack:ssh:mine"}}
        ]});
        assert_eq!(
            remove_rules(Some(&rules), "mine"),
            "delete rule ip tt-ssh prerouting handle 1\n"
        );
        assert_eq!(remove_rules(None, "mine"), "");
    }
}

#[cfg(all(test, not(target_os = "linux")))]
mod platform_tests {
    use super::*;

    #[test]
    fn rejects_linux_namespace_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let mut ingress = SshIngress {
            public_address: "192.0.2.1".parse().unwrap(),
            namespace: Some(directory.path().to_path_buf()),
            target: Some("192.0.2.2".parse().unwrap()),
        };
        assert!(
            ingress
                .validate()
                .unwrap_err()
                .to_string()
                .contains("require Linux")
        );
        ingress.namespace = None;
        ingress.target = None;
        assert!(ingress.validate().is_ok());
    }
}
