//! end-to-end tests for the v1.0 cli client (spec 14).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SA: &str = env!("CARGO_BIN_EXE_sa");

struct Daemon {
    child: Child,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Daemon {
    // the child is waited on in `Drop`, which clippy's zombie_processes lint cannot see.
    #[allow(clippy::zombie_processes)]
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("daemon.sock");

        let child = Command::new(SA)
            .arg("daemon")
            .env("SA_SOCKET", &socket)
            .env("XDG_RUNTIME_DIR", dir.path())
            .env("SA_STATE_DIR", dir.path().join("state"))
            .stderr(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn daemon");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if socket.exists() {
                return Self {
                    child,
                    socket,
                    _dir: dir,
                };
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("daemon socket never appeared at {}", socket.display());
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn hello_line() -> String {
    r#"{"jsonrpc":"2.0","id":1,"method":"hello","params":{"protocol":1,"client":"test","client_version":"0"}}"#
        .to_owned()
}

#[test]
fn ping_cli_reports_version() {
    let daemon = Daemon::start();

    let out = Command::new(SA)
        .arg("ping")
        .env("SA_SOCKET", &daemon.socket)
        .output()
        .expect("run sa ping");

    assert!(
        out.status.success(),
        "sa ping failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("json on stdout");
    assert!(value["version"].is_string(), "{stdout}");
    assert_eq!(value["sessions"], 0);
}

#[test]
fn ping_cli_fails_when_the_daemon_is_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.sock");

    let out = Command::new(SA)
        .arg("ping")
        .env("SA_SOCKET", &missing)
        .output()
        .expect("run sa ping");

    assert!(!out.status.success(), "must fail without a daemon");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot reach daemon"), "{stderr}");
}

#[test]
fn stdio_proxy_serves_ping() {
    let daemon = Daemon::start();

    let mut child = Command::new(SA)
        .args(["rpc", "--stdio"])
        .env("SA_SOCKET", &daemon.socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn proxy");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    writeln!(stdin, "{}", hello_line()).expect("write hello");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read hello response");
    let handshake: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert!(handshake["error"].is_null(), "{line}");
    assert_eq!(handshake["result"]["protocol"], 1);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"ping","params":{{}}}}"#
    )
    .expect("write ping");
    line.clear();
    stdout.read_line(&mut line).expect("read ping response");
    let ping: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert!(ping["error"].is_null(), "{line}");
    assert_eq!(ping["result"]["sessions"], 0);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn stdio_proxy_relays_errors_verbatim() {
    let daemon = Daemon::start();

    let mut child = Command::new(SA)
        .args(["rpc", "--stdio"])
        .env("SA_SOCKET", &daemon.socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn proxy");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));

    // ping before hello must be refused, through the proxy, unchanged.
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{}}}}"#
    )
    .expect("write ping");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    let value: serde_json::Value = serde_json::from_str(&line).expect("json");
    assert_eq!(value["error"]["code"], -32003, "{line}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn stdio_proxy_exits_nonzero_when_daemon_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.sock");

    let status = Command::new(SA)
        .args(["rpc", "--stdio"])
        .env("SA_SOCKET", &missing)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run proxy");

    assert!(!status.success(), "proxy must fail without a daemon");
}

#[test]
fn no_subcommand_attaches_to_the_most_recent_session() {
    let daemon = Daemon::start();

    // stdin is not a terminal here, so attach falls back to line mode; with no input it
    // attaches, prints the banner, then exits on EOF.
    let out = Command::new(SA)
        .env("SA_SOCKET", &daemon.socket)
        .stdin(Stdio::null())
        .output()
        .expect("run sa with no arguments");

    assert!(
        out.status.success(),
        "sa with no arguments must attach: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("attached to"),
        "expected the attach banner, got: {stdout}"
    );
}
