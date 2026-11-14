//! unix socket server, dispatcher host and supervision.

pub mod conn;
pub mod socket;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sa_core::dispatcher::Dispatcher;
use sa_core::llm::Provider;
use sa_core::manager::{Manager, ManagerConfig};
use sa_core::peer;
use sa_core::store::Store;
use sa_proto::error::{code, RpcError};

#[derive(Debug, Clone)]
pub struct Config {
    pub runtime_dir: PathBuf,
    pub socket_path: PathBuf,
    pub state_dir: PathBuf,
    pub db_path: PathBuf,
    pub model: String,
    pub allowlist_path: PathBuf,
    pub host_fingerprint: String,
}

impl Config {
    /// the daemon-wide store. sessions record their own `cwd`; `.sa/` stays per-workspace
    /// config only, because one daemon serves many workspaces.
    #[must_use]
    pub fn from_env() -> Self {
        let runtime_dir = socket::default_runtime_dir();
        let state_dir = std::env::var_os("XDG_STATE_HOME")
            .map_or_else(|| PathBuf::from(".sa"), PathBuf::from)
            .join("sa");
        let socket_path = std::env::var_os("SA_SOCKET")
            .map_or_else(|| runtime_dir.join(socket::SOCKET_FILE), PathBuf::from);
        let state_dir = std::env::var_os("SA_STATE_DIR").map_or(state_dir, PathBuf::from);
        Self {
            db_path: state_dir.join("sa.db"),
            allowlist_path: state_dir.join("policy.toml"),
            model: std::env::var("SA_MODEL").unwrap_or_else(|_| "default".to_owned()),
            host_fingerprint: host_fingerprint(&state_dir),
            runtime_dir,
            socket_path,
            state_dir,
        }
    }

    /// a config rooted entirely at `root`, for tests.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        let state_dir = root.join("state");
        Self {
            runtime_dir: root.join("run"),
            socket_path: root.join("run").join(socket::SOCKET_FILE),
            db_path: state_dir.join("sa.db"),
            model: "test-model".to_owned(),
            allowlist_path: state_dir.join("policy.toml"),
            host_fingerprint: "test-host".to_owned(),
            state_dir,
        }
    }
}

/// stable local identifier, never a hostname or address (spec 5.3).
#[must_use]
pub fn host_fingerprint(state_dir: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    if let Ok(machine) = std::fs::read("/etc/machine-id") {
        hasher.update(&machine);
    } else {
        let path = state_dir.join("machine-id");
        let value = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            let generated = uuid::Uuid::new_v4().to_string();
            let _ = std::fs::create_dir_all(state_dir);
            let _ = std::fs::write(&path, &generated);
            generated
        });
        hasher.update(value.trim().as_bytes());
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(#[from] sa_core::store::StoreError),
    #[error("manager: {0}")]
    Manager(#[from] sa_core::manager::ManagerError),
}

/// runs the daemon until `shutdown` resolves.
pub async fn run(
    cfg: Config,
    provider: Arc<dyn Provider>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), DaemonError> {
    socket::prepare_runtime_dir(&cfg.runtime_dir)?;
    std::fs::create_dir_all(&cfg.state_dir)?;

    let store = Store::open(&cfg.db_path)?;
    let manager = Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: cfg.host_fingerprint.clone(),
            default_model: cfg.model.clone(),
            allowlist_path: cfg.allowlist_path.clone(),
            provider,
            max_tokens: std::env::var("SA_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok()),
            reasoning_effort: std::env::var("SA_REASONING_EFFORT").ok(),
            lock_dir: cfg.runtime_dir.join("locks"),
            context_limit: std::env::var("SA_CONTEXT_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(128_000),
        }),
    ));

    let recovered = manager.recover()?;
    if recovered > 0 {
        tracing::warn!(recovered, "turns interrupted by the previous daemon");
    }

    let listener = socket::bind(&cfg.socket_path)?;
    let dispatcher = Arc::new(Dispatcher::new(Arc::clone(&manager)));
    let own_uid = nix::unistd::geteuid().as_raw();

    tracing::info!(socket = %cfg.socket_path.display(), "sa daemon listening");

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let std_stream = stream.into_std()?;
                let uid = conn::peer_uid(&std_stream)?;
                std_stream.set_nonblocking(true)?;
                let stream = tokio::net::UnixStream::from_std(std_stream)?;

                if let Err(err) = peer::check_uid(own_uid, uid) {
                    let error = RpcError::new(code::PEER_UID_MISMATCH, err.to_string());
                    tokio::spawn(async move {
                        let _ = conn::reject(stream, error).await;
                    });
                    continue;
                }

                let dispatcher = Arc::clone(&dispatcher);
                let manager = Arc::clone(&manager);
                tokio::spawn(async move {
                    if let Err(err) = conn::serve(stream, dispatcher, manager).await {
                        tracing::debug!(%err, "connection closed");
                    }
                });
            }
        }
    }

    let _ = std::fs::remove_file(&cfg.socket_path);
    tracing::info!("sa daemon stopped");
    Ok(())
}
