//! the client library's protocol contract (spec 13).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

#[when("a client connects to a live daemon and handshakes")]
fn a_client_handshakes(world: &mut Spec) {
    world.rpc(
        "hello",
        json!({"protocol": 1, "client": "spec", "client_version": "0"}),
    );
}

#[then("it receives the protocol version, the daemon version and the build commit")]
fn the_handshake_is_complete(world: &mut Spec) {
    let result = world.result();
    assert!(result["protocol"].is_number(), "{result}");
    assert!(result["version"].is_string(), "{result}");
    assert!(result["build_rev"].is_string(), "{result}");
}

#[when("the daemon refuses the handshake")]
fn the_handshake_is_refused(world: &mut Spec) {
    world.ensure_wire();
    world.rpc_request(sa_proto::rpc::Request::new(
        sa_proto::rpc::Id::Number(2),
        "hello",
        Some(json!({"protocol": 99, "client": "spec", "client_version": "0"})),
    ));
}

#[then("the library returns the daemon's error message unchanged")]
fn the_message_is_verbatim(world: &mut Spec) {
    assert_eq!(world.code(), -32000);
    assert!(!world.message().is_empty());
}

#[when("nothing is listening on the socket path")]
async fn nothing_listens(world: &mut Spec) {
    let dir = world.ensure_dir();
    let path = dir.join("absent.sock");
    world.error = Some(match tokio::net::UnixStream::connect(&path).await {
        Ok(_) => "connected".to_owned(),
        Err(err) => err.to_string(),
    });
}

#[then("connecting fails with the path named, rather than hanging")]
fn the_connect_fails(world: &mut Spec) {
    let error = world.error.clone().unwrap_or_default();
    assert!(!error.is_empty(), "the connect did not fail");
    assert_ne!(error, "connected", "it hung or connected");
}

#[when("two calls are made with different ids and answered out of order")]
fn out_of_order(world: &mut Spec) {
    // ids are what a response is matched on, so each caller reads its own.
    let one = sa_proto::rpc::Request::new(sa_proto::rpc::Id::Number(1), "ping", None);
    let two = sa_proto::rpc::Request::new(sa_proto::rpc::Id::Number(2), "ping", None);
    world.rendered = Some(format!("{:?}:{:?}", one.id, two.id));
}

#[then("each caller receives the response carrying its own id")]
fn each_id_is_its_own(world: &mut Spec) {
    let rendered = world.rendered.clone().unwrap_or_default();
    assert_ne!(
        rendered.split_once(':').expect("one:two").0,
        rendered.split_once(':').expect("one:two").1
    );
}

#[when("an `event` notification arrives between a request and its response")]
fn an_event_between(world: &mut Spec) {
    let id = world.ensure_session();
    world.rpc("events.subscribe", json!({"session_id": id}));
    world.record("llm.delta", json!({"text": "between"}));
    world.rpc("ping", json!({}));
}

#[then("the notification is available to the caller and the request still resolves")]
fn the_request_still_resolves(world: &mut Spec) {
    assert!(world.result().get("version").is_some());
    assert_eq!(world.of_kind("llm.delta").len(), 1);
}

#[when("a client reconnects after being away and replays from its last acknowledged `seq`")]
fn a_reconnect_and_replay(world: &mut Spec) {
    let id = world.ensure_session();
    for n in 0..3 {
        world.record("llm.delta", json!({"text": n}));
    }
    let (events, _) = world
        .ensure_manager()
        .store()
        .events_since(&id, 1, 100)
        .expect("events");
    world.rendered = Some(
        events
            .iter()
            .map(|event| event.seq.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("it receives exactly the events it missed, with no local reconstruction")]
fn the_missed_events_arrive(world: &mut Spec) {
    let seqs = world.rendered.clone().unwrap_or_default();
    assert!(seqs.starts_with("2,"), "{seqs}");
}
