//! unix socket binding, permissions and liveness (spec 4.1).

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// the socket file name inside the runtime directory.
pub const SOCKET_FILE: &str = "daemon.sock";

/// the runtime directory, from `XDG_RUNTIME_DIR`. falls back to the per-user temp directory.
#[must_use]
pub fn default_runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || std::env::temp_dir().join(format!("sa-{}", nix::unistd::geteuid().as_raw())),
        |dir| PathBuf::from(dir).join("sa"),
    )
}

#[must_use]
pub fn default_socket_path() -> PathBuf {
    default_runtime_dir().join(SOCKET_FILE)
}

/// creates the runtime directory at mode 0700 (spec 4.1).
pub fn prepare_runtime_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// binds the daemon socket at mode 0600.
///
/// a socket left behind by a dead daemon is replaced; a socket held by a live one is not.
pub fn bind(path: &Path) -> io::Result<tokio::net::UnixListener> {
    // the socket's own directory is secured even when SA_SOCKET points somewhere custom.
    if let Some(parent) = path.parent() {
        prepare_runtime_dir(parent)?;
    }
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("another daemon is listening on {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }

    let listener = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    tokio::net::UnixListener::from_std(listener)
}
