//! the `sa` binary. every subcommand is a disposable client of the daemon, except `daemon`.
//!
//! this is the user-facing program: it is the one place allowed to write to the terminal.
#![allow(clippy::print_stderr, clippy::print_stdout)]

mod cli;
mod commands;

use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command};
use sa_client::Client;
use sa_core::llm::{MissingProvider, OpenAiProvider, Provider};
use sa_daemon::{run, Config};

#[tokio::main]
async fn main() -> ExitCode {
    let parsed = Cli::parse();

    // the daemon is long-lived and wants its lifecycle visible; a one-shot client does not.
    let default_level = if matches!(parsed.command, Some(Command::Daemon)) {
        "info"
    } else {
        "warn"
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .init();

    let result = match parsed.command {
        // no subcommand means attach: `sa` on its own should get you working.
        None => commands::attach(None, false, false).await,
        Some(Command::Daemon) => {
            let provider: std::sync::Arc<dyn Provider> = match OpenAiProvider::from_env() {
                Ok(provider) => std::sync::Arc::new(provider),
                Err(err) => {
                    eprintln!("sa: no model credentials ({err}); turns will fail");
                    std::sync::Arc::new(MissingProvider)
                }
            };
            run(Config::from_env(), provider, shutdown_signal())
                .await
                .map_err(|e| e.to_string())
        }
        Some(Command::Ping) => ping().await,
        Some(Command::Rpc { stdio }) => {
            if stdio {
                proxy_stdio().await
            } else {
                Err("only --stdio is supported".to_owned())
            }
        }
        Some(Command::Run { prompt }) => commands::run_once(&prompt).await,
        Some(Command::Attach { session, new, line }) => commands::attach(session, new, line).await,
        Some(Command::Sessions) => commands::sessions().await,
        Some(Command::Context { dir }) => commands::context(dir),
        Some(Command::Export { session, format }) => commands::export(session, &format).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sa: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn ping() -> Result<(), String> {
    let path = cli::socket_path();
    let mut client = Client::connect(&path)
        .await
        .map_err(|e| format!("cannot reach daemon at {}: {e}", path.display()))?;
    client
        .handshake("sa")
        .await
        .map_err(|e| format!("handshake failed: {e}"))?;
    let ping = client
        .ping()
        .await
        .map_err(|e| format!("ping failed: {e}"))?;
    let json = serde_json::to_string_pretty(&ping).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

/// dumb byte relay between the daemon socket and stdio. no protocol awareness: the daemon
/// sees the proxy's own uid, which is the point (spec 4.2).
async fn proxy_stdio() -> Result<(), String> {
    let path = cli::socket_path();
    let stream = sa_client::connect_raw(&path)
        .await
        .map_err(|e| format!("cannot reach daemon at {}: {e}", path.display()))?;

    let (mut sock_read, mut sock_write) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    // on stdin eof, close the socket write half so the daemon sees the end of the stream and
    // closes in turn. without this the relay waits forever on a half-open connection.
    let upstream = async {
        let copied = tokio::io::copy(&mut stdin, &mut sock_write).await;
        drop(sock_write);
        copied
    };
    let downstream = async {
        let copied = tokio::io::copy(&mut sock_read, &mut stdout).await;
        let _ = tokio::io::AsyncWriteExt::flush(&mut stdout).await;
        copied
    };

    tokio::try_join!(upstream, downstream).map_err(|e| e.to_string())?;
    Ok(())
}

async fn shutdown_signal() {
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
