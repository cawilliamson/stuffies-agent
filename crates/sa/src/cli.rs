//! command line surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "sa",
    // the commit is part of the version: a client and a daemon from different code are
    // different programs, and this is how that is seen at a glance.
    version = sa_proto::BUILD_NAME,
    about = "daemon-first coding agent"
)]
pub struct Cli {
    /// omit to attach to the most recent session.
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// attach to a session interactively
    Attach {
        /// session id; defaults to the most recent session
        #[arg(long)]
        session: Option<String>,
        /// start a fresh session instead of resuming one
        #[arg(long)]
        new: bool,
        /// plain line mode even when stdout is a terminal
        #[arg(long)]
        line: bool,
    },
    /// show the context and skills that would be injected for a directory
    Context {
        /// directory to inspect; defaults to the current one
        #[arg(long)]
        dir: Option<String>,
    },
    /// run the daemon in the foreground
    Daemon,
    /// dump a session as jsonl or markdown
    Export {
        #[arg(long)]
        session: Option<String>,
        #[arg(long, default_value = "markdown")]
        format: String,
    },
    /// list the language servers, whether each is installed, and whether it is running
    Lsp,
    /// list the configured mcp servers, their tools and their state
    Mcp,
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
    /// list sessions
    Sessions,
}

/// the daemon socket. `SA_SOCKET` overrides the default so several daemons can coexist.
#[must_use]
pub fn socket_path() -> PathBuf {
    std::env::var_os("SA_SOCKET").map_or_else(sa_daemon::socket::default_socket_path, PathBuf::from)
}
