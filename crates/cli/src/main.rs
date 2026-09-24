//! TTstack CLI — manage your private cloud from the command line.

mod client;
mod deploy;
mod image_builder;

use clap::{Parser, Subcommand};
use client::Client;
use ruc::*;
use ttcore::api::*;
use ttcore::model::*;

/// TTstack — lightweight private cloud for developers and small teams.
#[derive(Parser)]
#[command(name = "tt", version, about)]
struct Cli {
    /// Controller address (overrides ~/.ttconfig).
    #[arg(long, short, global = true)]
    server: Option<String>,

    /// API key for controller authentication.
    #[arg(long, short = 'k', global = true, env = "TT_API_KEY")]
    api_key: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Configure the controller address and optional API key.
    Config {
        /// Controller address, e.g. "10.0.0.1:9200".
        addr: String,
        /// API key for authentication (optional).
        #[arg(long, short = 'k')]
        api_key: Option<String>,
    },
    /// Show fleet-wide status.
    Status,
    /// Manage physical hosts.
    Host {
        #[command(subcommand)]
        action: HostCmd,
    },
    /// Manage environments.
    Env {
        #[command(subcommand)]
        action: EnvCmd,
    },
    /// Manage images.
    Image {
        #[command(subcommand)]
        action: ImageCmd,
    },
    /// Deploy TTstack to local or remote hosts.
    Deploy {
        #[command(subcommand)]
        action: DeployCmd,
    },
}

#[derive(Subcommand)]
enum HostCmd {
    /// Register a new host by its agent address.
    Add {
        /// Agent address, e.g. "10.0.0.2:9100".
        addr: String,
    },
    /// List all hosts.
    List,
    /// Show host details.
    Show { id: String },
    /// Remove a host from the fleet.
    Remove { id: String },
}

#[derive(Subcommand)]
enum EnvCmd {
    /// Create a new environment with VMs.
    Create {
        /// Environment name.
        name: String,
        /// Image name (repeatable).
        #[arg(long, short, required = true)]
        image: Vec<String>,
        /// Engine type: qemu, firecracker, docker (Linux); bhyve, jail (experimental FreeBSD).
        #[arg(long, default_value = "qemu")]
        engine: String,
        /// CPU cores per VM.
        #[arg(long)]
        cpu: Option<u32>,
        /// Memory per VM in MiB.
        #[arg(long)]
        mem: Option<u32>,
        /// QEMU virtual disk size in MiB (other engines use the existing image size).
        #[arg(long)]
        disk: Option<u32>,
        /// Duplicate each image N times.
        #[arg(long, default_value_t = 1)]
        dup: u32,
        /// Port to expose (repeatable).
        #[arg(long, short)]
        port: Vec<u16>,
        /// Environment lifetime in seconds (default 21600; 0 = no expiry).
        #[arg(long)]
        lifetime: Option<u64>,
        /// Block routed outgoing traffic (unsupported for Docker; not host/VM isolation).
        #[arg(long)]
        deny_outgoing: bool,
        /// Owner identifier (defaults to $USER).
        #[arg(long)]
        owner: Option<String>,
        /// SSH public key for VM access (repeatable). Can also be a path to a .pub file.
        #[arg(long)]
        ssh_key: Vec<String>,
    },
    /// List all environments.
    List,
    /// Show environment details.
    Show { name: String },
    /// Delete an environment.
    Delete { name: String },
    /// Stop all VMs in an environment.
    Stop { name: String },
    /// Start all VMs in an environment.
    Start { name: String },
}

#[derive(Subcommand)]
enum ImageCmd {
    /// List available images across all hosts.
    List,
    /// List built-in image recipes that can be auto-created.
    Recipes,
    /// Create an image from a built-in recipe.
    Create {
        /// Recipe name (see 'tt image recipes'), or "all".
        name: String,
        /// Image directory (for non-Docker engines).
        #[arg(long, default_value = "/home/ttstack/images")]
        image_dir: String,
        /// Only create images for this engine (docker, firecracker, qemu, jail).
        #[arg(long)]
        engine: Option<String>,
    },
}

#[derive(Subcommand)]
enum DeployCmd {
    /// Deploy agent on this host (requires root).
    Agent {
        /// Path to release binaries directory.
        #[arg(long, default_value = "./target/release")]
        release_dir: String,
    },
    /// Deploy controller on this host (requires root).
    Ctl {
        /// Path to release binaries directory.
        #[arg(long, default_value = "./target/release")]
        release_dir: String,
    },
    /// Deploy both agent and controller on this host (requires root).
    All {
        /// Path to release binaries directory.
        #[arg(long, default_value = "./target/release")]
        release_dir: String,
    },
    /// Distributed deploy to all hosts defined in a config file.
    Dist {
        /// Path to deploy config (TOML format).
        #[arg(default_value = "deploy.toml")]
        config: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Cmd::Config { addr, api_key } = &cli.cmd {
        if let Err(e) = client::save_config(addr, api_key.as_deref()) {
            eprintln!("Failed to save config: {e}");
            std::process::exit(1);
        }
        println!("Controller set to: {addr}");
        if api_key.is_some() {
            println!("API key saved.");
        }
        return;
    }

    // Deploy and image-create commands don't need a controller
    if let Cmd::Deploy { action } = &cli.cmd {
        let result = match action {
            DeployCmd::Agent { release_dir } => deploy::deploy_local("agent", release_dir).await,
            DeployCmd::Ctl { release_dir } => deploy::deploy_local("ctl", release_dir).await,
            DeployCmd::All { release_dir } => deploy::deploy_local("all", release_dir).await,
            DeployCmd::Dist { config } => deploy::deploy_distributed(config).await,
        };
        if let Err(e) = result {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    if let Cmd::Image {
        action: ImageCmd::Recipes,
    } = &cli.cmd
    {
        image_builder::list_recipes();
        return;
    }

    if let Cmd::Image {
        action:
            ImageCmd::Create {
                name,
                image_dir,
                engine,
            },
    } = &cli.cmd
    {
        let dir = std::path::Path::new(image_dir);
        let result = if name == "all" {
            if let Some(eng) = engine {
                image_builder::create_all_for_engine(eng, dir).await
            } else {
                image_builder::create_all(dir).await
            }
        } else {
            image_builder::create_image(name, dir).await
        };
        if let Err(e) = result {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
        return;
    }

    let (addr, api_key) = if let Some(server) = cli.server {
        (server, cli.api_key)
    } else if let Some(cfg) = client::load_config() {
        (cfg.addr, cli.api_key.or(cfg.api_key))
    } else {
        eprintln!("No controller address. Run: tt config <addr>");
        std::process::exit(1);
    };

    let c = Client::new(&addr, api_key.as_deref());

    let result = match cli.cmd {
        Cmd::Config { .. } | Cmd::Deploy { .. } => unreachable!(),
        Cmd::Status => cmd_status(&c).await,
        Cmd::Host { action } => cmd_host(&c, action).await,
        Cmd::Env { action } => cmd_env(&c, action).await,
        Cmd::Image { action } => cmd_image(&c, action).await,
    };

    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

// ── Command Implementations ─────────────────────────────────────────

async fn cmd_status(c: &Client) -> Result<()> {
    let s: FleetStatus = c.get("/api/status").await?;
    println!("Fleet Status");
    println!("  Hosts:   {}/{} online", s.hosts_online, s.hosts);
    println!("  VMs:     {}", s.total_vms);
    println!("  Envs:    {}", s.total_envs);
    println!("  CPU:     {}/{} cores", s.cpu_used, s.cpu_total);
    println!("  Memory:  {}/{} MB", s.mem_used, s.mem_total);
    println!("  Disk:    {}/{} MB", s.disk_used, s.disk_total);
    Ok(())
}

async fn cmd_host(c: &Client, action: HostCmd) -> Result<()> {
    match action {
        HostCmd::Add { addr } => {
            let host: Host = c.post("/api/hosts", &RegisterHostReq { addr }).await?;
            println!("Host registered: {} ({})", host.id, host.addr);
            println!("  Engines: {:?}", host.engines);
            println!("  Storage: {}", host.storage);
            println!(
                "  Resources: {} CPU, {} MB RAM, {} MB disk",
                host.resource.cpu_total, host.resource.mem_total, host.resource.disk_total
            );
        }
        HostCmd::List => {
            let hosts: Vec<Host> = c.get("/api/hosts").await?;
            if hosts.is_empty() {
                println!("No hosts registered.");
                return Ok(());
            }
            println!(
                "{:<12} {:<22} {:<8} {:>6} {:>8} {:>8}",
                "ID", "ADDR", "STATE", "CPU", "MEM(MB)", "VMs"
            );
            for h in hosts {
                println!(
                    "{:<12} {:<22} {:<8} {:>3}/{:<3} {:>4}/{:<4} {:>4}",
                    h.id,
                    h.addr,
                    format!("{:?}", h.state).to_lowercase(),
                    h.resource.cpu_used,
                    h.resource.cpu_total,
                    h.resource.mem_used,
                    h.resource.mem_total,
                    h.resource.vm_count,
                );
            }
        }
        HostCmd::Show { id } => {
            let h: Host = c.get(&format!("/api/hosts/{id}")).await?;
            println!("Host: {}", h.id);
            println!("  Address:  {}", h.addr);
            println!("  State:    {:?}", h.state);
            println!("  Engines:  {:?}", h.engines);
            println!("  Storage:  {}", h.storage);
            println!(
                "  CPU:      {}/{}",
                h.resource.cpu_used, h.resource.cpu_total
            );
            println!(
                "  Memory:   {}/{} MB",
                h.resource.mem_used, h.resource.mem_total
            );
            println!(
                "  Disk:     {}/{} MB",
                h.resource.disk_used, h.resource.disk_total
            );
            println!("  VMs:      {}", h.resource.vm_count);
        }
        HostCmd::Remove { id } => {
            c.delete(&format!("/api/hosts/{id}")).await?;
            println!("Host removed: {id}");
        }
    }
    Ok(())
}

async fn cmd_env(c: &Client, action: EnvCmd) -> Result<()> {
    match action {
        EnvCmd::Create {
            name,
            image,
            engine,
            cpu,
            mem,
            disk,
            dup,
            port,
            lifetime,
            deny_outgoing,
            owner,
            ssh_key,
        } => {
            let engine: Engine = engine.parse().map_err(|e: String| eg!(e))?;

            let owner = owner
                .or_else(|| std::env::var("USER").ok())
                .unwrap_or_else(|| "default".to_string());

            // Resolve SSH keys: if a value looks like a file path, read it
            let ssh_keys: Vec<String> = ssh_key
                .into_iter()
                .map(|k| {
                    if (k.ends_with(".pub") || k.starts_with('/') || k.starts_with("~/"))
                        && !k.starts_with("ssh-")
                    {
                        let path = if k.starts_with("~/") {
                            k.replacen("~", &std::env::var("HOME").unwrap_or_default(), 1)
                        } else {
                            k.clone()
                        };
                        std::fs::read_to_string(&path)
                            .map(|s| s.trim().to_string())
                            .map_err(|e| eg!("cannot read SSH public key {}: {}", path, e))
                    } else {
                        Ok(k)
                    }
                })
                .collect::<Result<Vec<_>>>()?;

            if dup == 0 || (image.len() as u64) * u64::from(dup) > MAX_VMS as u64 {
                return Err(eg!(
                    "replicas must be positive and total VM count must not exceed {}",
                    MAX_VMS
                ));
            }
            let mut vms = Vec::new();
            for img in &image {
                for _ in 0..dup {
                    vms.push(VmSpec {
                        image: img.clone(),
                        engine,
                        cpu,
                        mem,
                        disk,
                        ports: port.clone(),
                        deny_outgoing,
                        ssh_keys: vec![],
                    });
                }
            }

            let req = CreateEnvReq {
                id: name.clone(),
                owner,
                vms,
                lifetime,
                ssh_keys,
            };

            let mut detail: EnvDetail = c.post("/api/envs", &req).await?;
            eprintln!("Creating environment {name}; you can inspect it with 'tt env show {name}'.");
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(600);
            while detail.env.state == EnvState::Creating {
                if tokio::time::Instant::now() >= deadline {
                    return Err(eg!(
                        "creation is still in progress; inspect it with 'tt env show {}'",
                        name
                    ));
                }
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                detail = c.get(&format!("/api/envs/{name}")).await?;
            }
            if detail.env.state != EnvState::Active {
                return Err(eg!(
                    "environment {} is {:?}; resources retained. {}. Inspect with 'tt env show {}' or clean up with 'tt env delete {}'",
                    name,
                    detail.env.state,
                    detail.warnings.join("; "),
                    name,
                    name
                ));
            }
            println!("Environment created: {name}");
            println!("  VMs: {}", detail.vms.len());
            for vm in &detail.vms {
                println!(
                    "    {} [{}] {} — {}  ports: {:?}",
                    vm.id, vm.engine, vm.image, vm.ip, vm.port_map
                );
            }
            print_access(c, &detail.vms).await;
            for w in &detail.warnings {
                eprintln!("  warning: {w}");
            }
        }
        EnvCmd::List => {
            let envs: Vec<Env> = c.get("/api/envs").await?;
            if envs.is_empty() {
                println!("No environments.");
                return Ok(());
            }
            println!("{:<16} {:<12} {:<8} {:>4}", "NAME", "OWNER", "STATE", "VMs");
            for e in envs {
                println!(
                    "{:<16} {:<12} {:<8} {:>4}",
                    e.id,
                    e.owner,
                    format!("{:?}", e.state).to_lowercase(),
                    e.vm_ids.len(),
                );
            }
        }
        EnvCmd::Show { name } => {
            let detail: EnvDetail = c.get(&format!("/api/envs/{name}")).await?;
            println!("Environment: {}", detail.env.id);
            println!("  Owner:   {}", detail.env.owner);
            println!("  State:   {:?}", detail.env.state);
            println!("  VMs:     {}", detail.vms.len());
            for warning in &detail.warnings {
                eprintln!("  warning: {warning}");
            }
            println!();
            if !detail.vms.is_empty() {
                let id_width = detail
                    .vms
                    .iter()
                    .map(|v| v.id.len())
                    .max()
                    .unwrap_or(2)
                    .max(2);
                let image_width = detail
                    .vms
                    .iter()
                    .map(|v| v.image.len())
                    .max()
                    .unwrap_or(5)
                    .max(5);
                println!(
                    "  {:<id_width$} {:<image_width$} {:<10} {:<8} {:<16} PORTS",
                    "ID", "IMAGE", "ENGINE", "STATE", "IP"
                );
                for vm in &detail.vms {
                    let ports: String = vm
                        .port_map
                        .iter()
                        .map(|(g, h)| format!("{h}->{g}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "  {:<id_width$} {:<image_width$} {:<10} {:<8} {:<16} {}",
                        vm.id,
                        vm.image,
                        vm.engine.to_string(),
                        vm.state.to_string(),
                        vm.ip,
                        ports
                    );
                }
            }
            print_access(c, &detail.vms).await;
        }
        EnvCmd::Delete { name } => {
            c.delete(&format!("/api/envs/{name}")).await?;
            println!("Environment deleted: {name}");
        }
        EnvCmd::Stop { name } => {
            c.post_action(&format!("/api/envs/{name}/stop")).await?;
            println!("Environment stopped: {name}");
        }
        EnvCmd::Start { name } => {
            c.post_action(&format!("/api/envs/{name}/start")).await?;
            println!("Environment started: {name}");
        }
    }
    Ok(())
}

async fn print_access(client: &Client, vms: &[Vm]) {
    let hosts: Vec<Host> = match client.get("/api/hosts").await {
        Ok(hosts) => hosts,
        Err(e) => {
            eprintln!("  Cannot resolve host access addresses: {e}");
            return;
        }
    };
    for vm in vms {
        if let Some(host) = hosts.iter().find(|h| h.id == vm.host_id)
            && let Ok(url) = reqwest::Url::parse(&format!("http://{}", host.addr))
            && let Some(addr) = url.host_str()
        {
            for (&guest, &port) in &vm.port_map {
                println!("  Access {}: {addr}:{port} -> guest TCP {guest}", vm.id);
            }
        }
    }
}

async fn cmd_image(c: &Client, action: ImageCmd) -> Result<()> {
    match action {
        ImageCmd::List => {
            let images: Vec<ImageInfo> = c.get("/api/images").await?;
            if images.is_empty() {
                println!("No images available.");
                return Ok(());
            }
            println!("{:<30} {:<12}", "IMAGE", "HOST");
            for img in images {
                println!("{:<30} {:<12}", img.name, img.host_id);
            }
        }
        ImageCmd::Recipes | ImageCmd::Create { .. } => {
            unreachable!("handled before controller connection")
        }
    }
    Ok(())
}
