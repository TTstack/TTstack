//! Opt-in IPv4 guest isolation, independent of guest firewall cooperation.
use super::*;

fn table(vm_id: &str) -> String {
    tap_name(vm_id).replace('-', "_")
}

fn rules(vm_id: &str, address: &str) -> Result<String> {
    let ip: std::net::Ipv4Addr = address.parse().c(d!("invalid guest IPv4 address"))?;
    let [a, b, c, d] = ip.octets();
    if [a, b] != [10, 10] || ip.to_string() == BRIDGE_ADDR {
        return Err(eg!("invalid isolated guest address"));
    }
    let mac = format!("02:54:{a:02x}:{b:02x}:{c:02x}:{d:02x}");
    let tap = tap_name(vm_id);
    let t = table(vm_id);
    // Both tables are replaced in one nft transaction; no unprotected interval.
    // Bridge rules stop L2 peer traffic and IP/ARP spoofing before routing.
    // Inet rules also protect every host-local address, not only the bridge IP.
    Ok(format!(
        r#"
add table bridge {t}
flush table bridge {t}
add chain bridge {t} source {{ type filter hook prerouting priority -300; policy accept; }}
add rule bridge {t} source iifname "{tap}" ether saddr != {mac} drop
add rule bridge {t} source iifname "{tap}" ether type arp arp saddr ip != {ip} drop
add rule bridge {t} source iifname "{tap}" ether type arp arp saddr ether != {mac} drop
add rule bridge {t} source iifname "{tap}" ether type arp arp daddr ip != {BRIDGE_ADDR} drop
add rule bridge {t} source iifname "{tap}" ether type ip ip saddr != {ip} drop
add rule bridge {t} source iifname "{tap}" ether type != {{ ip, arp }} drop
add chain bridge {t} peers {{ type filter hook forward priority -300; policy accept; }}
add rule bridge {t} peers iifname "{tap}" drop
add rule bridge {t} peers oifname "{tap}" drop
add table inet {t}
flush table inet {t}
add chain inet {t} host {{ type filter hook input priority -10; policy accept; }}
add rule inet {t} host iifname "{BRIDGE_NAME}" ip saddr {ip} ct direction reply accept
add rule inet {t} host iifname "{BRIDGE_NAME}" ip saddr {ip} drop
add chain inet {t} routed {{ type filter hook forward priority -10; policy accept; }}
add rule inet {t} routed iifname "{BRIDGE_NAME}" ip saddr {ip} ct direction reply accept
add rule inet {t} routed iifname "{BRIDGE_NAME}" ip saddr {ip} ip daddr {{ 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16, 224.0.0.0/4, 240.0.0.0/4 }} drop
add rule inet {t} routed iifname "{BRIDGE_NAME}" oifname "{BRIDGE_NAME}" ip daddr {ip} drop
"#
    ))
}

pub fn isolate(vm_id: &str, ip: &str) -> Result<()> {
    super::platform::nft(&rules(vm_id, ip)?)
}

pub fn remove_isolation(vm_id: &str) -> Result<()> {
    let table = table(vm_id);
    // List first to make cleanup idempotent without hiding permission/tool failures.
    let output = Command::new("nft")
        .args(["-j", "list", "tables"])
        .bounded_output()
        .c(d!("list isolation tables"))?;
    if !output.status.success() {
        return Err(eg!("cannot list isolation tables"));
    }
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).c(d!("isolation tables"))?;
    for family in ["bridge", "inet"] {
        if json["nftables"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|e| e["table"]["family"] == family && e["table"]["name"] == table)
        }) {
            super::platform::nft(&format!("delete table {family} {table}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolation_rejects_unallocated_addresses() {
        for ip in ["::1", "1.2.3.4", "10.10.0.1", "10.10.0.2; flush ruleset"] {
            assert!(rules("test", ip).is_err());
        }
        assert!(rules("test", "10.10.0.2").is_ok());
    }
}
