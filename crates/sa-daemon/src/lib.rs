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
    pub host_fingerprint: String,
    pub turn_timeout_secs: u64,
    pub aux_budget_secs: u64,
}

impl Config {
    /// the daemon-wide store. sessions record their own `cwd`. nothing is written into a
    /// workspace: per-workspace config lives under `$XDG_CONFIG_HOME/sa/workspaces/`.
    #[must_use]
    pub fn from_env() -> Self {
        let runtime_dir = socket::default_runtime_dir();
        // never a relative path: an unset XDG_STATE_HOME must not drop `.sa/` into the
        // daemon's cwd, which is usually a workspace (spec 6.1).
        let state_dir = std::env::var_os("XDG_STATE_HOME")
            .map_or_else(
                || {
                    std::env::var_os("HOME")
                        .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
                        .join(".local/state")
                },
                PathBuf::from,
            )
            .join("sa");
        let socket_path = std::env::var_os("SA_SOCKET")
            .map_or_else(|| runtime_dir.join(socket::SOCKET_FILE), PathBuf::from);
        let state_dir = std::env::var_os("SA_STATE_DIR").map_or(state_dir, PathBuf::from);
        Self {
            db_path: state_dir.join("sa.db"),
            model: std::env::var("SA_MODEL").unwrap_or_default(),
            host_fingerprint: host_fingerprint(&state_dir),
            turn_timeout_secs: std::env::var("SA_TURN_TIMEOUT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1800),
            aux_budget_secs: std::env::var("SA_AUX_BUDGET")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20),
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
            model: String::new(),
            host_fingerprint: "test-host".to_owned(),
            turn_timeout_secs: 1800,
            aux_budget_secs: 20,
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

    // ask the endpoint what it serves rather than inventing a name. if it cannot answer
    // yet, keep asking in the background: the bar simply omits the area until it can.
    let mut discovered = Vec::new();
    if cfg.model.trim().is_empty() {
        for attempt in 1..=3 {
            match provider.list_models().await {
                Ok(models) if !models.is_empty() => {
                    tracing::info!(model = %models[0], "resolved model from the endpoint");
                    discovered = models;
                    break;
                }
                Ok(_) => tracing::debug!(attempt, "endpoint reported no models"),
                Err(err) => tracing::debug!(attempt, %err, "model discovery failed"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    } else {
        discovered = vec![cfg.model.clone()];
    }

    let models: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(discovered));
    if models.lock().map_or(true, |m| m.is_empty()) {
        let slot = Arc::clone(&models);
        let provider = Arc::clone(&provider);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                if let Ok(found) = provider.list_models().await {
                    if let Some(first) = found.first() {
                        tracing::info!(model = %first, "resolved model from the endpoint");
                        if let Ok(mut guard) = slot.lock() {
                            *guard = found;
                        }
                        return;
                    }
                }
            }
        });
    }

    let store = Store::open(&cfg.db_path)?;
    let manager = Arc::new(Manager::new(
        store,
        Arc::new(ManagerConfig {
            host_fingerprint: cfg.host_fingerprint.clone(),
            provider,
            max_tokens: std::env::var("SA_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok()),
            reasoning_effort: std::env::var("SA_REASONING_EFFORT").ok(),
            default_effort: std::env::var("SA_REASONING_EFFORT")
                .unwrap_or_else(|_| "medium".to_owned()),
            models,
            lock_dir: cfg.runtime_dir.join("locks"),
            turn_timeout_secs: cfg.turn_timeout_secs,
            aux_budget_secs: cfg.aux_budget_secs,
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
