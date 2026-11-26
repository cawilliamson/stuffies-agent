//! the `sa` binary. every subcommand is a disposable client of the daemon, except `daemon`.
//!
//! this is the user-facing program: it is the one place allowed to write to the terminal.
#![allow(clippy::print_stderr, clippy::print_stdout)]

mod cli;
mod commands;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command};
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
    // the tui owns the terminal, so a log line written to stderr is drawn over the
    // interface. while it is up, the log goes to a file instead.
    let screen = wants_the_screen(parsed.command.as_ref())
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal();
    init_tracing(default_level, screen);

    let result = match parsed.command {
        // no subcommand means attach: `sa` on its own should get you working.
        None => commands::attach(parsed.standalone, None, false, false).await,
        // the daemon already is the backend, so asking for it in-process says nothing.
        Some(Command::Daemon) if parsed.standalone => Err(
            "--standalone runs the backend in this process; `daemon` already is the backend"
                .to_owned(),
        ),
        Some(Command::Daemon) => run(Config::from_env(), commands::provider(), shutdown_signal())
            .await
            .map_err(|e| e.to_string()),
        Some(Command::Ping) => ping(parsed.standalone).await,
        Some(Command::Rpc { stdio }) => {
            if stdio {
                proxy_stdio(parsed.standalone).await
            } else {
                Err("only --stdio is supported".to_owned())
            }
        }
        Some(Command::Run { prompt }) => commands::run_once(parsed.standalone, &prompt).await,
        Some(Command::Attach { session, new, line }) => {
            commands::attach(parsed.standalone, session, new, line).await
        }
        Some(Command::Sessions) => commands::sessions(parsed.standalone).await,
        Some(Command::Lsp) => commands::lsp(parsed.standalone).await,
        Some(Command::Mcp) => commands::mcp(parsed.standalone).await,
        Some(Command::Openspec { args }) => commands::openspec(&args).await,
        Some(Command::Context { dir }) => commands::context(dir),
        Some(Command::Export { session, format }) => {
            commands::export(parsed.standalone, session, &format).await
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("sa: {message}");
            ExitCode::FAILURE
        }
    }
}

/// whether this invocation can hand the terminal to the tui.
const fn wants_the_screen(command: Option<&Command>) -> bool {
    match command {
        // no subcommand means attach, which is the tui when it has a terminal.
        None => true,
        Some(Command::Attach { line, .. }) => !*line,
        Some(_) => false,
    }
}

/// the log filter. `screen` says the tui is about to own the terminal.
fn filter_for(level: &str, screen: bool) -> tracing_subscriber::EnvFilter {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    quiet_markdown(filter, screen)
}

/// keeps the markdown renderer's unnamed-fence warning out of the log.
///
/// it warns for every fenced block it cannot name, which is any fence written without a
/// language: model output is full of them, they render fine, and there is nothing an
/// operator could do about them. only while the tui owns the screen, because everywhere
/// else the crate keeps its voice.
fn quiet_markdown(
    filter: tracing_subscriber::EnvFilter,
    screen: bool,
) -> tracing_subscriber::EnvFilter {
    match (screen, "tui_markdown=error".parse()) {
        (true, Ok(directive)) => filter.add_directive(directive),
        _ => filter,
    }
}

/// starts the log.
///
/// when the tui owns the terminal, a line written to stderr is drawn over the interface,
/// so the log goes to a file under the state directory instead. a dependency's warning
/// must never corrupt the display.
fn init_tracing(level: &str, screen: bool) {
    let Some(file) = screen
        .then(|| log_file_in(&sa_daemon::state_dir()))
        .flatten()
    else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter_for(level, screen))
            .init();
        return;
    };
    tracing_subscriber::fmt()
        .with_writer(file)
        .with_env_filter(filter_for(level, screen))
        .init();
}

/// the file the tui's log goes to, or none when the directory cannot be written.
fn log_file_in(dir: &std::path::Path) -> Option<std::fs::File> {
    let path = dir.join("sa-tui.log");
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

async fn ping(standalone: bool) -> Result<(), String> {
    // `connect` has already performed the mandatory handshake.
    let mut client = commands::connect(standalone).await?;
    let ping = client
        .ping()
        .await
        .map_err(|e| format!("ping failed: {e}"))?;
    let json = serde_json::to_string_pretty(&ping).map_err(|e| e.to_string())?;
    println!("{json}");
    // stdout stays pure json so it can be piped; the build comparison is for a person.
    eprintln!("client build: {}", sa_proto::BUILD_REV);
    eprintln!("daemon build: {}", ping.build_rev);
    if ping.build_rev == sa_proto::BUILD_REV {
        eprintln!("builds match");
    } else {
        eprintln!("builds differ: restart the daemon to pick up this client's code");
    }
    Ok(())
}

/// dumb byte relay between the daemon socket and stdio. no protocol awareness: the daemon
/// sees the proxy's own uid, which is the point (spec 4.2).
async fn proxy_stdio(standalone: bool) -> Result<(), String> {
    let stream = commands::stream(standalone).await?;

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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn only_an_interactive_attach_hands_over_the_screen() {
        assert!(wants_the_screen(None), "no subcommand is the tui");
        assert!(
            wants_the_screen(Some(&Command::Attach {
                session: None,
                new: false,
                line: false,
            })),
            "attach on a terminal is the tui"
        );
        assert!(
            !wants_the_screen(Some(&Command::Attach {
                session: None,
                new: false,
                line: true,
            })),
            "line mode never takes the screen"
        );
        for command in [
            Command::Daemon,
            Command::Lsp,
            Command::Mcp,
            Command::Openspec { args: Vec::new() },
            Command::Ping,
            Command::Sessions,
        ] {
            assert!(
                !wants_the_screen(Some(&command)),
                "only attach owns a terminal"
            );
        }
    }

    #[test]
    fn the_tui_log_is_a_file_in_the_state_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = log_file_in(dir.path()).expect("a writable directory takes the log");
        drop(file);
        assert!(
            dir.path().join("sa-tui.log").is_file(),
            "the log is a file, not the terminal"
        );
    }

    #[test]
    fn the_tui_filter_silences_the_markdown_renderer() {
        // the renderer warns for every fenced block it cannot name, which is any fence
        // written without a language. it renders fine, and the warning is not actionable.
        // built from a fixed level, so the ambient RUST_LOG of a build sandbox cannot
        // change what this test is asserting.
        let quiet = tracing_subscriber::fmt()
            .with_env_filter(quiet_markdown(
                tracing_subscriber::EnvFilter::new("warn"),
                true,
            ))
            .finish();
        tracing::subscriber::with_default(quiet, || {
            assert!(
                !tracing::enabled!(target: "tui_markdown::render::code", tracing::Level::WARN),
                "an unnamed code fence must not log"
            );
        });

        // everywhere else the crate keeps its voice.
        let loud = tracing_subscriber::fmt()
            .with_env_filter(quiet_markdown(
                tracing_subscriber::EnvFilter::new("warn"),
                false,
            ))
            .finish();
        tracing::subscriber::with_default(loud, || {
            assert!(
                tracing::enabled!(target: "tui_markdown::render::code", tracing::Level::WARN),
                "a client that does not own the screen still hears it"
            );
        });
    }
}
