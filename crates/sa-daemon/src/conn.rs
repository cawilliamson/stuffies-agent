//! per-connection loop: peer credentials, framing, dispatch, notification push.

use std::io;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use sa_core::dispatcher::{ConnState, Dispatcher};
use sa_core::manager::Manager;
use sa_proto::error::{code, RpcError};
use sa_proto::rpc::{Request, Response};
use sa_proto::MAX_LINE_LEN;
use tokio_util::codec::{Framed, LinesCodec};

/// deadline for the mandatory first frame.
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ConnError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("client did not send hello within {}s", HANDSHAKE_TIMEOUT.as_secs())]
    HandshakeTimeout,
    #[error("framing: {0}")]
    Framing(String),
}

/// reads the peer uid from the accepted socket, before any frame is read.
pub fn peer_uid(stream: &StdUnixStream) -> io::Result<u32> {
    let cred = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
        .map_err(|err| io::Error::from_raw_os_error(err as i32))?;
    Ok(cred.uid())
}

fn codec() -> LinesCodec {
    LinesCodec::new_with_max_length(MAX_LINE_LEN)
}

fn encode(response: &Response) -> io::Result<String> {
    serde_json::to_string(response)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// writes one error frame and closes. used when a connection is refused before dispatch.
pub async fn reject(stream: tokio::net::UnixStream, error: RpcError) -> Result<(), ConnError> {
    let mut framed = Framed::new(stream, codec());
    let response = encode(&Response::err(None, error))?;
    framed
        .send(response)
        .await
        .map_err(|err| ConnError::Framing(err.to_string()))
}

/// serves one client until it disconnects or a protocol error closes the connection.
pub async fn serve(
    stream: tokio::net::UnixStream,
    dispatcher: Arc<Dispatcher>,
    manager: Arc<Manager>,
) -> Result<(), ConnError> {
    let (conn_id, mut notifications) = manager
        .register_connection()
        .map_err(|e| ConnError::Framing(e.to_string()))?;

    let result = serve_inner(stream, dispatcher, conn_id, &mut notifications).await;
    manager.drop_connection(conn_id);
    result
}

async fn serve_inner(
    stream: tokio::net::UnixStream,
    dispatcher: Arc<Dispatcher>,
    conn_id: u64,
    notifications: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
) -> Result<(), ConnError> {
    let mut framed = Framed::new(stream, codec());
    let mut state = ConnState::new(conn_id);
    let mut first = true;

    loop {
        let next = if first {
            match tokio::time::timeout(HANDSHAKE_TIMEOUT, framed.next()).await {
                Ok(next) => next,
                Err(_) => return Err(ConnError::HandshakeTimeout),
            }
        } else {
            tokio::select! {
                frame = framed.next() => frame,
                Some(notification) = notifications.recv() => {
                    framed
                        .send(notification.to_string())
                        .await
                        .map_err(|err| ConnError::Framing(err.to_string()))?;
                    continue;
                }
            }
        };
        first = false;

        let Some(frame) = next else { break };

        let line = match frame {
            Ok(line) => line,
            Err(err) => {
                let error = RpcError::new(code::LINE_TOO_LONG, format!("line too long: {err}"));
                let _ = framed.send(encode(&Response::err(None, error))?).await;
                return Err(ConnError::Framing(err.to_string()));
            }
        };

        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                let error = RpcError::new(code::PARSE_ERROR, format!("parse error: {err}"));
                let _ = framed.send(encode(&Response::err(None, error))?).await;
                return Err(ConnError::Framing(err.to_string()));
            }
        };

        if let Some(response) = dispatcher.handle(&mut state, request).await {
            framed
                .send(encode(&response)?)
                .await
                .map_err(|err| ConnError::Framing(err.to_string()))?;
        }
    }

    Ok(())
}
