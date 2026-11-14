//! command line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "sa", version, about = "daemon-first coding agent")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// run the daemon in the foreground
    Daemon,
    /// handshake and ping the daemon
    Ping,
    /// relay the daemon socket over stdin/stdout (spec 4.2)
    Rpc {
        /// newline-delimited json-rpc on stdin/stdout
        #[arg(long)]
        stdio: bool,
    },
    /// run one prompt to completion and print the answer
    Run {
        /// the prompt
        #[arg(short = 'p', long)]
        prompt: String,
    },
    /// attach to a session interactively
    Attach {
        /// session id; defaults to the most recent session
        #[arg(long)]
        session: Option<String>,
    },
    /// dump a session as jsonl or markdown
    Export {
        #[arg(long)]
        session: Option<String>,
        #[arg(long, default_value = "markdown")]
        format: String,
    },
}

/// the daemon socket. `SA_SOCKET` overrides the default so several daemons can coexist.
#[must_use]
pub fn socket_path() -> PathBuf {
    std::env::var_os("SA_SOCKET").map_or_else(sa_daemon::socket::default_socket_path, PathBuf::from)
}
