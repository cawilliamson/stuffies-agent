//! typed client library: connect, handshake, call.
//!
//! this is the only thing a client needs. it holds no authority (spec 2.4): it is a cache
//! with a cursor.

use std::path::Path;

use futures_util::{SinkExt, StreamExt};
use sa_proto::error::RpcError;
use sa_proto::rpc::{
    HelloParams, HelloResult, Id, PingResult, Request, Response, DAEMON_VERSION, PROTOCOL_VERSION,
};
use sa_proto::MAX_LINE_LEN;
use serde_json::Value;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LinesCodec};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("rpc: {0}")]
    Rpc(#[from] RpcError),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("daemon closed the connection")]
    Closed,
}

pub struct Client {
    framed: Framed<UnixStream, LinesCodec>,
    next_id: i64,
    queued: std::collections::VecDeque<serde_json::Value>,
}

/// connects to the daemon socket without any framing, for byte relays such as `sa rpc`.
pub async fn connect_raw(path: &Path) -> Result<UnixStream, ClientError> {
    Ok(UnixStream::connect(path).await?)
}

impl Client {
    pub async fn connect(path: &Path) -> Result<Self, ClientError> {
        let stream = connect_raw(path).await?;
        Ok(Self {
            framed: Framed::new(stream, LinesCodec::new_with_max_length(MAX_LINE_LEN)),
            next_id: 0,
            queued: std::collections::VecDeque::new(),
        })
    }

    /// a client over an already-connected stream. used by tests, which stand a scripted
    /// daemon on the other end instead of a real socket.
    #[must_use]
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            framed: Framed::new(stream, LinesCodec::new_with_max_length(MAX_LINE_LEN)),
            next_id: 0,
            queued: std::collections::VecDeque::new(),
        }
    }

    /// performs the mandatory `hello` handshake (spec 3.1).
    pub async fn handshake(&mut self, client: &str) -> Result<HelloResult, ClientError> {
        let params = HelloParams {
            protocol: PROTOCOL_VERSION,
            client: client.to_owned(),
            client_version: DAEMON_VERSION.to_owned(),
        };
        let value = self
            .call(
                "hello",
                serde_json::to_value(params).map_err(|e| ClientError::Protocol(e.to_string()))?,
            )
            .await?;
        let result: HelloResult =
            serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))?;
        // a daemon built from different code is a different program. saying so here turns a
        // mystery into a one-line answer.
        if !result.build_rev.is_empty()
            && result.build_rev != "unknown"
            && result.build_rev != sa_proto::BUILD_REV
        {
            return Err(ClientError::Protocol(format!(
                "build mismatch: daemon is {} (v{}), client is {} (v{}); restart the daemon",
                result.build_rev,
                result.version,
                sa_proto::BUILD_REV,
                DAEMON_VERSION
            )));
        }
        Ok(result)
    }

    /// sends a request and waits for its response, queueing notifications that arrive in
    /// the meantime so the caller can render them afterwards.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.request(method, params).await?;
        loop {
            // read the socket, never the queue: a queued notification is not this call's
            // response, and reading it back from the queue spins forever.
            let value = self.read_frame().await?;
            if value.get("id").and_then(Value::as_i64) == Some(id) {
                let response: Response = serde_json::from_value(value)
                    .map_err(|e| ClientError::Protocol(e.to_string()))?;
                return response.into_result().map_err(ClientError::from);
            }
            // a notification is rendered later. anything else is the response to a call
            // that already timed out, so it is dropped rather than re-queued.
            if value.get("method").is_some() {
                self.queued.push_back(value);
            }
        }
    }

    /// sends a request without waiting for its response.
    pub async fn request(&mut self, method: &str, params: Value) -> Result<i64, ClientError> {
        self.next_id += 1;
        let id = self.next_id;
        let request = Request::new(Id::Number(id), method, Some(params));
        let line =
            serde_json::to_string(&request).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.framed
            .send(line)
            .await
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        Ok(id)
    }

    /// one raw frame from the daemon: a response or a notification.
    pub async fn next_frame(&mut self) -> Result<Value, ClientError> {
        if let Some(queued) = self.queued.pop_front() {
            return Ok(queued);
        }
        self.read_frame().await
    }

    /// one frame straight off the socket, bypassing anything already queued.
    async fn read_frame(&mut self) -> Result<Value, ClientError> {
        let raw = self
            .framed
            .next()
            .await
            .ok_or(ClientError::Closed)?
            .map_err(|e| ClientError::Protocol(e.to_string()))?;
        serde_json::from_str(&raw).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// the next notification, discarding responses that arrive out of band.
    pub async fn next_notification(&mut self) -> Result<Value, ClientError> {
        loop {
            let value = self.next_frame().await?;
            if value.get("method").is_some() {
                return Ok(value);
            }
        }
    }

    pub async fn ping(&mut self) -> Result<PingResult, ClientError> {
        let value = self
            .call("ping", Value::Object(serde_json::Map::new()))
            .await?;
        serde_json::from_value(value).map_err(|e| ClientError::Protocol(e.to_string()))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    /// a notification that arrives before the response must not stop `call` from
    /// finding it. the previous implementation pushed the frame back onto `queued`
    /// and then read it from there again, spinning without ever touching the socket.
    #[tokio::test]
    async fn call_skips_a_notification_and_still_returns_its_response() {
        let (ours, theirs) = UnixStream::pair().expect("socketpair");
        tokio::spawn(async move {
            let mut server = Framed::new(theirs, LinesCodec::new_with_max_length(MAX_LINE_LEN));
            let request: Value =
                serde_json::from_str(&server.next().await.expect("frame").expect("line"))
                    .expect("json");
            let id = request.get("id").cloned().expect("id");
            server
                .send(r#"{"jsonrpc":"2.0","method":"event","params":{}}"#.to_owned())
                .await
                .expect("notify");
            server
                .send(
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"ok": true}})
                        .to_string(),
                )
                .await
                .expect("respond");
        });

        let mut client = Client {
            framed: Framed::new(ours, LinesCodec::new_with_max_length(MAX_LINE_LEN)),
            next_id: 0,
            queued: std::collections::VecDeque::new(),
        };

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.call("ping", serde_json::json!({})),
        )
        .await
        .expect("call must not spin on the notification")
        .expect("response");
        assert_eq!(result, serde_json::json!({"ok": true}));

        // and the notification is still available to render.
        let queued = client
            .next_notification()
            .await
            .expect("queued notification");
        assert_eq!(queued["method"], "event");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod build_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    /// answers the handshake with a daemon built from a different commit.
    async fn handshake_against(build_rev: &str) -> Result<HelloResult, ClientError> {
        let (ours, theirs) = UnixStream::pair().expect("socketpair");
        let rev = build_rev.to_owned();
        tokio::spawn(async move {
            let mut server = Framed::new(theirs, LinesCodec::new_with_max_length(MAX_LINE_LEN));
            let request: Value =
                serde_json::from_str(&server.next().await.expect("frame").expect("line"))
                    .expect("json");
            let id = request.get("id").cloned().expect("id");
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"protocol": PROTOCOL_VERSION, "version": "0.1.0", "build_rev": rev},
            });
            server.send(reply.to_string()).await.expect("respond");
        });

        let mut client = Client::from_stream(ours);
        client.handshake("test").await
    }

    #[tokio::test]
    async fn a_daemon_built_from_another_commit_is_refused() {
        let error = handshake_against("deadbeefcafe")
            .await
            .expect_err("a different build must not be accepted");
        let text = error.to_string();
        assert!(text.contains("build mismatch"), "{text}");
        assert!(text.contains("deadbeefcafe"), "names the daemon: {text}");
        assert!(
            text.contains(sa_proto::BUILD_REV),
            "names the client: {text}"
        );
    }

    #[tokio::test]
    async fn a_daemon_from_the_same_commit_is_accepted() {
        let result = handshake_against(sa_proto::BUILD_REV)
            .await
            .expect("the same build must be accepted");
        assert_eq!(result.build_rev, sa_proto::BUILD_REV);
    }

    #[tokio::test]
    async fn a_daemon_that_does_not_say_its_build_is_accepted() {
        // an older daemon sends no revision at all; refusing it would break every upgrade.
        let result = handshake_against("")
            .await
            .expect("an unstated build is not a mismatch");
        assert_eq!(result.build_rev, "", "an unstated build stays empty");
    }
}
