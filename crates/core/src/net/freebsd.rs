//! FreeBSD networking. Only TTstack's child anchors are replaced.
use super::*;
use std::net::Ipv4Addr;

fn run(args: &[&str]) -> Result<String> {
    let output = Command::new(args[0])
        .args(&args[1..])
        .bounded_output()
        .c(d!(args.join(" ")))?;
    if !output.status.success() {
        return Err(eg!(
            "{}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).c(d!("invalid command output"))
}

pub fn setup_bridge() -> Result<()> {
    if !Command::new("ifconfig")
        .arg(BRIDGE_NAME)
        .bounded_output()
        .c(d!())?
        .status
        .success()
    {
        run(&["ifconfig", "bridge", "create", "name", BRIDGE_NAME])?;
    }
    // Repeat configuration after an interrupted first setup.
    run(&["ifconfig", BRIDGE_NAME, "inet", BRIDGE_CIDR])?;
    run(&["ifconfig", BRIDGE_NAME, "up"])?;
    run(&["sysctl", "net.inet.ip.forwarding=1"])?;
    Ok(())
}

fn tap_record(vm_id: &str) -> std::path::PathBuf {
    std::path::Path::new(crate::model::RUN_DIR).join(format!("bhyve-{vm_id}.tap"))
}

pub fn tap_device(vm_id: &str) -> Result<String> {
    let name = std::fs::read_to_string(tap_record(vm_id)).c(d!("read bhyve TAP device"))?;
    if name
        .strip_prefix("tap")
        .is_none_or(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(eg!("invalid bhyve TAP device record"));
    }
    Ok(name)
}

pub fn create_tap(vm_id: &str) -> Result<()> {
    let tap = tap_name(vm_id);
    let interfaces = run(&["ifconfig", "-l"])?;
    if !interfaces.split_whitespace().any(|name| name == tap) {
        let device = if tap_record(vm_id).exists() {
            let saved = tap_device(vm_id)?;
            if interfaces.split_whitespace().any(|name| name == saved) {
                let info = run(&["ifconfig", &saved])?;
                if !info.contains(&format!("description: ttstack:{vm_id}")) {
                    return Err(eg!("TAP device ownership changed; retain resources"));
                }
                saved
            } else {
                String::new()
            }
        } else {
            String::new()
        };
        let device = if device.is_empty() {
            let created = run(&["ifconfig", "tap", "create"])?;
            let created = created.trim().to_string();
            run(&[
                "ifconfig",
                &created,
                "description",
                &format!("ttstack:{vm_id}"),
            ])?;
            std::fs::create_dir_all(crate::model::RUN_DIR).c(d!())?;
            if let Err(e) = std::fs::write(tap_record(vm_id), &created) {
                run(&["ifconfig", &created, "destroy"])?;
                return Err(eg!(e));
            }
            created
        } else {
            device
        };
        run(&["ifconfig", &device, "name", &tap])?;
    }
    // Renaming an interface does not rename its /dev/tapN character device.
    tap_device(vm_id)?;
    let bridge = run(&["ifconfig", BRIDGE_NAME])?;
    if !bridge
        .lines()
        .any(|line| line.trim().starts_with(&format!("member: {tap} ")))
    {
        run(&["ifconfig", BRIDGE_NAME, "addm", &tap])?;
    }
    run(&["ifconfig", &tap, "up"])?;
    Ok(())
}

pub fn destroy_tap(vm_id: &str) -> Result<()> {
    let tap = tap_name(vm_id);
    let interfaces = run(&["ifconfig", "-l"])?;
    if interfaces.split_whitespace().any(|name| name == tap) {
        run(&["ifconfig", &tap, "destroy"])?;
    } else if tap_record(vm_id).exists() {
        let saved = tap_device(vm_id)?;
        if interfaces.split_whitespace().any(|name| name == saved) {
            let info = run(&["ifconfig", &saved])?;
            if !info.contains(&format!("description: ttstack:{vm_id}")) {
                return Err(eg!("TAP device ownership changed; retain resources"));
            }
            run(&["ifconfig", &saved, "destroy"])?;
        }
    }
    match std::fs::remove_file(tap_record(vm_id)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(eg!(e)),
    }
    Ok(())
}

pub fn setup_nat() -> Result<()> {
    let info = run(&["pfctl", "-s", "info"])?;
    let nat = run(&["pfctl", "-sn"])?;
    let filter = run(&["pfctl", "-sr"])?;
    if !pf_hooks_ready(&info, &nat, &filter) {
        return Err(eg!(
            "enable PF and configure rdr-anchor \"ttstack/*\" and anchor \"ttstack/*\" without quick before starting guests; provision outbound NAT separately"
        ));
    }
    Ok(())
}

fn pf_hooks_ready(info: &str, nat: &str, filter: &str) -> bool {
    let mut hooks = filter
        .lines()
        .filter(|line| line.starts_with("anchor \"ttstack/*\""))
        .peekable();
    info.contains("Status: Enabled")
        && nat
            .lines()
            .any(|line| line.starts_with("rdr-anchor \"ttstack/*\""))
        && hooks.peek().is_some()
        && hooks.all(|line| !line.split_whitespace().any(|word| word == "quick"))
}

fn address_key(ip: &str) -> Result<String> {
    let ip: Ipv4Addr = ip.parse().c(d!("invalid guest IPv4 address"))?;
    if ip.octets()[..2] != [10, 10] {
        return Err(eg!("guest address is outside the TTstack subnet"));
    }
    Ok(ip.to_string().replace('.', "-"))
}

fn load_anchor(anchor: &str, rule: &str) -> Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().c(d!("PF rule file"))?;
    file.write_all(rule.as_bytes()).c(d!("write PF rule"))?;
    let output = Command::new("pfctl")
        .args(["-a", anchor, "-f"])
        .arg(file.path())
        .bounded_output()
        .c(d!("load PF anchor"))?;
    if !output.status.success() {
        return Err(eg!(
            "load PF anchor: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

pub fn add_port_forward(host_port: u16, ip: &str, guest_port: u16) -> Result<()> {
    let key = address_key(ip)?;
    load_anchor(
        &format!("ttstack/forward-{key}-{host_port}"),
        &format!(
            "rdr inet proto tcp from any to self port {host_port} -> {ip} port {guest_port}\n\
             pass in quick inet proto tcp from any to {ip} port {guest_port} flags S/SA keep state (if-bound)\n\
             pass out quick inet proto tcp from any to {ip} port {guest_port} flags S/SA keep state (if-bound)\n"
        ),
    )
}

pub fn remove_port_forwards(ip: &str) -> Result<()> {
    let prefix = format!("forward-{}-", address_key(ip)?);
    let anchors = run(&["pfctl", "-a", "ttstack", "-s", "Anchors"])?;
    for name in anchors
        .lines()
        .filter_map(|name| owned_forward_anchor(name, &prefix))
    {
        for kind in ["nat", "rules"] {
            run(&["pfctl", "-a", &format!("ttstack/{name}"), "-F", kind])?;
        }
    }
    Ok(())
}

fn owned_forward_anchor<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let name = name.trim();
    let name = name.strip_prefix("ttstack/").unwrap_or(name);
    name.strip_prefix(prefix)?.parse::<u16>().ok()?;
    Some(name)
}

pub fn deny_outgoing(ip: &str) -> Result<()> {
    let key = address_key(ip)?;
    // Filter guest initiation before source NAT. Interface-bound forwarding states
    // admit replies on the guest bridge and then on the external interface.
    load_anchor(
        &format!("ttstack/deny-{key}"),
        &format!(
            "pass in quick inet from {ip} to self keep state (if-bound)\n\
         block return in quick inet from {ip} to any\n"
        ),
    )
}

pub fn allow_outgoing(ip: &str) -> Result<()> {
    let key = address_key(ip)?;
    for kind in ["nat", "rules"] {
        run(&["pfctl", "-a", &format!("ttstack/deny-{key}"), "-F", kind])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pf_preflight_rejects_the_quick_wildcard_that_skips_sibling_anchors() {
        let info = "Status: Enabled for 0 days";
        let nat = "rdr-anchor \"ttstack/*\" all";
        let good = "anchor \"ttstack/*\" all\npass all flags S/SA keep state";
        let old = "anchor \"ttstack/*\" quick all";
        assert!(pf_hooks_ready(info, nat, good));
        assert!(!pf_hooks_ready(info, nat, old));
        assert!(!pf_hooks_ready(info, nat, &format!("{old}\n{good}")));
        assert!(!pf_hooks_ready("Status: Disabled", nat, good));
        assert!(!pf_hooks_ready(info, "", good));
    }

    #[test]
    fn cleanup_accepts_native_qualified_names_without_matching_other_guests() {
        let prefix = "forward-10-10-0-2-";
        assert_eq!(
            owned_forward_anchor("  ttstack/forward-10-10-0-2-20000", prefix),
            Some("forward-10-10-0-2-20000")
        );
        assert_eq!(
            owned_forward_anchor("forward-10-10-0-2-20001", prefix),
            Some("forward-10-10-0-2-20001")
        );
        for other in [
            "ttstack/forward-10-10-0-20-20000",
            "ttstack/deny-10-10-0-2",
            "elsewhere/forward-10-10-0-2-20000",
            "forward-10-10-0-2-20000/foreign",
        ] {
            assert!(owned_forward_anchor(other, prefix).is_none());
        }
    }
}
