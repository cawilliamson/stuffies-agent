//! foreground daemon binary. systemd owns this in production (spec 12).

use std::sync::Arc;

use sa_core::llm::{MissingProvider, OpenAiProvider, Provider};
use sa_daemon::{run, Config};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env();
    let provider: Arc<dyn Provider> = match OpenAiProvider::from_env() {
        Ok(provider) => Arc::new(provider),
        Err(err) => {
            tracing::warn!(%err, "no model credentials; turns will fail");
            Arc::new(MissingProvider)
        }
    };
    match run(cfg, provider, shutdown_signal()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "daemon failed");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn shutdown_signal() {
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(sig) => sig,
        Err(err) => {
            tracing::error!(%err, "cannot install sigterm handler");
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("sigint"),
        _ = term.recv() => tracing::info!("sigterm"),
    }
}
