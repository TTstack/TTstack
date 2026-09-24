//! TTstack central controller entry point.
//!
//! The controller manages the fleet of hosts, schedules VM placement,
//! and exposes an HTTP API for the CLI client and web interface.

mod auth;
mod config;
mod db;
mod handler;
mod scheduler;
mod web;

use axum::Router;
use axum::routing::{get, post};
use clap::Parser;
use config::Config;
use db::Db;
use handler::CtlState;
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let cfg = Config::parse();

    std::fs::create_dir_all(&cfg.data_dir).unwrap_or_else(|e| {
        eprintln!("Failed to create data dir {}: {e}", cfg.data_dir);
        std::process::exit(1);
    });

    let db_path = format!("{}/ctl.db", cfg.data_dir);
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("Failed to open database: {e}");
        std::process::exit(1);
    });

    let state: CtlState = Arc::new(handler::CtlShared::new(db, cfg.api_key.clone()));

    // Background task: expire old environments
    let expiry_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            expire_envs(&expiry_state).await;
        }
    });

    // Background task: periodic host health check
    let heartbeat_state = state.clone();
    tokio::spawn(async move {
        let client = handler::agent_client(heartbeat_state.api_key.as_deref(), 5);
        loop {
            // Avoid racing a creation plan with an old host snapshot.
            if let Ok(_operation) = heartbeat_state.operations.try_lock() {
                handler::refresh_all_hosts(&heartbeat_state, &client).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        }
    });

    if cfg.api_key.is_some() {
        eprintln!("API key authentication enabled");
    } else {
        eprintln!("WARNING: no --api-key set, all API endpoints are unauthenticated!");
    }

    let api_routes = Router::new()
        .route(
            "/api/hosts",
            get(handler::list_hosts).post(handler::register_host),
        )
        .route(
            "/api/hosts/{id}",
            get(handler::get_host).delete(handler::remove_host),
        )
        .route(
            "/api/envs",
            get(handler::list_envs).post(handler::create_env),
        )
        .route(
            "/api/envs/{id}",
            get(handler::get_env).delete(handler::delete_env),
        )
        .route("/api/envs/{id}/stop", post(handler::stop_env))
        .route("/api/envs/{id}/start", post(handler::start_env))
        .route("/api/vms/{id}", get(handler::get_vm))
        .route("/api/images", get(handler::list_images))
        .route("/api/status", get(handler::fleet_status))
        .with_state(state);

    let api_routes = if let Some(ref key) = cfg.api_key {
        api_routes.layer(axum::middleware::from_fn(auth::make_auth_layer(
            key.clone(),
        )))
    } else {
        api_routes
    };

    let app = Router::new().route("/", get(web::index)).merge(api_routes);

    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind {}: {e}", cfg.listen);
            std::process::exit(1);
        });

    eprintln!("tt-ctl listening on {}", cfg.listen);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap_or_else(|e| eprintln!("Server error: {e}"));

    eprintln!("tt-ctl shutting down");
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

/// Retry incomplete deletions and expire environments without forgetting failed cleanup.
async fn expire_envs(state: &CtlState) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ids = match state.lock_db().list_envs() {
        Ok(envs) => envs
            .into_iter()
            .filter(|e| {
                e.state == ttcore::model::EnvState::Deleting
                    || (e.expires_at > 0 && e.expires_at <= now)
            })
            .map(|e| e.id)
            .collect::<Vec<_>>(),
        Err(e) => {
            eprintln!("[ctl] expiry DB error: {e}");
            return;
        }
    };
    for id in ids {
        if let Err((_, error)) = handler::expire_environment(state, &id).await {
            eprintln!("[ctl] cleanup {id}: {error}");
        }
    }
}
