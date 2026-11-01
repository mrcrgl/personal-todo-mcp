use personal_todo_mcp::app;
use personal_todo_mcp::config::Config;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = Config::from_env().map_err(|e| anyhow::anyhow!(e))?;
    let private_key = std::fs::read_to_string(&cfg.github_private_key_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read GitHub App key {}: {e}",
            cfg.github_private_key_path
        )
    })?;
    let services = app::build_services(&cfg, &private_key).map_err(|e| anyhow::anyhow!(e))?;

    tracing::info!(
        port = cfg.port,
        public_base_url = %cfg.public_base_url,
        app = %cfg.github_app_slug,
        "starting personal-todo-mcp"
    );

    let ct = CancellationToken::new();
    let router = app::router(&cfg, services, ct.child_token());
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", cfg.port)).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(ct))
        .await?;
    Ok(())
}

/// SIGTERM (k8s) and SIGINT (local) both drain gracefully; the exec-form CMD
/// in the Dockerfile is what makes SIGTERM actually reach PID 1.
async fn shutdown_signal(ct: CancellationToken) {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }
    tracing::info!("shutdown signal received, draining");
    ct.cancel();
}
