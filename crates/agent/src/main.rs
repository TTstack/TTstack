//! TTstack host agent entry point.
//!
//! The agent runs on each physical host, managing local VMs/containers
//! and exposing an HTTP API for the central controller.
//!
//! Supported platforms:
//! - **Linux**: Qemu, Firecracker, Docker/Podman
//! - **FreeBSD (experimental)**: Bhyve, Jail
//! - **Other Unix**: Docker code exists; no validated agent deployment path

mod auth;
mod config;
mod handler;
mod runtime;

use axum::Router;
use axum::routing::{get, post};
use clap::Parser;
use config::Config;
use handler::AppState;
use runtime::Runtime;
use std::sync::Arc;
use ttcore::model::Resource;

#[tokio::main]
async fn main() {
    let cfg = Config::parse();
    #[cfg(target_os = "freebsd")]
    eprintln!("WARNING: FreeBSD support is experimental and outside the primary validation scope");

    let db_path = format!("{}/agent.db", cfg.data_dir);
    std::fs::create_dir_all(&cfg.data_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create data dir {}: {e}", cfg.data_dir);
        std::process::exit(1);
    });

    let host_id = runtime::resolve_host_id(&db_path, cfg.host_id.clone()).unwrap_or_else(|e| {
        eprintln!("Failed to resolve host_id: {e}");
        std::process::exit(1);
    });

    let resource = Resource {
        cpu_total: cfg.effective_cpu(),
        mem_total: cfg.effective_mem(),
        disk_total: cfg.disk_total,
        ..Default::default()
    };

    let rt = Runtime::new(
        host_id.clone(),
        cfg.storage_kind(),
        cfg.image_dir.clone(),
        cfg.runtime_dir.clone(),
        &db_path,
        resource,
    )
    .unwrap_or_else(|e| {
        eprintln!("Failed to initialize runtime: {e}");
        std::process::exit(1);
    });

    let info = rt.agent_info().unwrap_or_else(|e| {
        eprintln!("Failed to read agent info: {e}");
        std::process::exit(1);
    });
    let state: AppState = Arc::new(handler::AgentShared {
        runtime: Arc::new(tokio::sync::Mutex::new(rt)),
        db_path,
        info,
        image_dir: cfg.image_dir.clone(),
    });
    let recovery = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            if let Ok(mut rt) = recovery.runtime.clone().try_lock_owned() {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = rt.reconcile() {
                        eprintln!("[agent] reconciliation failed: {e}");
                    }
                })
                .await;
            }
        }
    });

    let app = Router::new()
        .route("/api/info", get(handler::get_info))
        .route("/api/images", get(handler::list_images))
        .route("/api/vms", get(handler::list_vms).post(handler::create_vm))
        .route(
            "/api/vms/{id}",
            get(handler::get_vm).delete(handler::destroy_vm),
        )
        .route("/api/vms/{id}/stop", post(handler::stop_vm))
        .route("/api/vms/{id}/start", post(handler::start_vm))
        .with_state(state);

    let app = if let Some(key) = cfg.api_key {
        eprintln!("API key authentication enabled");
        app.layer(axum::middleware::from_fn(auth::make_auth_layer(key)))
    } else {
        eprintln!("WARNING: no --api-key set, all agent endpoints are unauthenticated!");
        app
    };

    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind {}: {e}", cfg.listen);
            std::process::exit(1);
        });

    eprintln!("tt-agent [{host_id}] listening on {}", cfg.listen);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| eprintln!("Server error: {e}"));

    eprintln!("tt-agent shutting down");
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("received shutdown signal");
}
