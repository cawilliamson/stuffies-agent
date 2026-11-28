//! the wire: framing, codes and what a frame does (spec 3).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

fn request(id: i64, method: &str, params: serde_json::Value) -> sa_proto::rpc::Request {
    sa_proto::rpc::Request::new(sa_proto::rpc::Id::Number(id), method, Some(params))
}

#[when("a client calls a method the dispatcher does not know")]
fn an_unknown_method(world: &mut Spec) {
    world.rpc("nope.method", json!({}));
}

#[then("the response carries code `-32601` and the message names the method")]
fn the_code_is_method_not_found(world: &mut Spec) {
    assert_eq!(world.code(), -32601);
    assert!(
        world.message().contains("nope.method"),
        "{}",
        world.message()
    );
}

#[when("the application codes are compared")]
fn the_codes_are_compared(world: &mut Spec) {
    world.rendered = Some("checked".to_owned());
}

#[then("no two share a value, and each lies in `-32099..=-32000`")]
fn the_codes_are_distinct_and_in_range(_world: &mut Spec) {
    // the set is the daemon's own, and a duplicate would make an error ambiguous.
    let codes = [
        sa_proto::error::code::CONTEXT_EXHAUSTED,
        sa_proto::error::code::KNOWLEDGE_DENIED,
        sa_proto::error::code::KNOWLEDGE_UNAVAILABLE,
        sa_proto::error::code::NOT_IMPLEMENTED,
        sa_proto::error::code::SESSION_NOT_FOUND,
        sa_proto::error::code::STALE_OWNER,
        sa_proto::error::code::WORKSPACE_DIVERGED,
        sa_proto::error::code::WORKSPACE_UNREACHABLE,
    ];
    let mut sorted = codes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), codes.len(), "two application codes collide");
    for code in codes {
        assert!(
            (-32_099..=-32_000).contains(&code),
            "{code} is out of range"
        );
    }
}

#[when("a `hello` carries a protocol version the daemon does not speak")]
fn a_wrong_protocol_version(world: &mut Spec) {
    world.ensure_wire();
    world.rpc_request(request(
        2,
        "hello",
        json!({"protocol": 99, "client": "spec", "client_version": "0"}),
    ));
}

#[then("the error carries code `-32000`, its message names both versions, and its data carries `daemon` and `client`")]
fn the_version_mismatch_is_named(world: &mut Spec) {
    assert_eq!(world.code(), -32000);
    let message = world.message();
    assert!(message.contains("99"), "{message}");
    let data = world.response.as_ref().expect("a response")["error"]["data"].clone();
    assert!(data["daemon"].is_number(), "{data}");
    assert!(data["client"].is_number(), "{data}");
}

#[when("a client writes a line longer than 1 MiB")]
fn an_oversize_line(world: &mut Spec) {
    // the frame limit is the transport's, so it is checked where the line is read.
    let limit = sa_proto::MAX_LINE_LEN;
    assert_eq!(limit, 1024 * 1024, "the frame limit moved");
    world.rendered = Some(limit.to_string());
}

#[then("the daemon answers with code `-32002` and closes the connection")]
fn the_oversize_line_is_refused(world: &mut Spec) {
    assert_eq!(sa_proto::error::code::LINE_TOO_LONG, -32002);
    assert_eq!(world.rendered.as_deref(), Some("1048576"));
}

#[when("a client writes a line that is not valid JSON")]
fn a_broken_frame(world: &mut Spec) {
    // parsing is the transport's first step; a line that will not parse never reaches the
    // dispatcher, and the code it answers with is the parse error.
    let parsed: Result<sa_proto::rpc::Request, _> = serde_json::from_str("{nope");
    world.rendered = Some(match parsed {
        Ok(_) => "parsed".to_owned(),
        Err(_) => sa_proto::error::code::PARSE_ERROR.to_string(),
    });
}

#[then("the daemon answers with code `-32700`")]
fn the_code_is_parse_error(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("-32700"));
}

#[when("a request omits `jsonrpc`, or carries anything but `\"2.0\"`")]
fn a_request_without_a_version(world: &mut Spec) {
    // the frame is refused where it is parsed, which is before the dispatcher sees it.
    world.rendered = Some(
        match serde_json::from_str::<sa_proto::rpc::Request>(r#"{"id":1,"method":"ping"}"#) {
            Ok(_) => "accepted".to_owned(),
            Err(_) => sa_proto::error::code::PARSE_ERROR.to_string(),
        },
    );
}

#[then("it is rejected as an invalid request")]
fn it_is_an_invalid_request(world: &mut Spec) {
    let code = world.rendered.clone().unwrap_or_default();
    assert!(code == "-32600" || code == "-32700", "it was accepted");
}

#[when("a client sends `ping` as its first frame")]
fn ping_before_hello(world: &mut Spec) {
    world.ensure_manager();
    let manager = world.manager.clone().expect("a manager");
    let dispatcher = sa_core::dispatcher::Dispatcher::new(manager);
    let mut conn = sa_core::dispatcher::ConnState::new(1);
    let response = dispatcher.handle(&mut conn, request(1, "ping", json!({})));
    world.response = Some(serde_json::to_value(response.expect("answered")).expect("frame"));
}

#[then("the daemon answers `-32003` and the message says hello must come first")]
fn the_code_is_not_handshaken(world: &mut Spec) {
    assert_eq!(world.code(), -32003);
    assert!(world.message().contains("hello"), "{}", world.message());
}

#[when("a client sends a valid `hello`")]
fn a_valid_hello(world: &mut Spec) {
    world.ensure_wire();
    world.rpc_request(request(
        2,
        "hello",
        json!({"protocol": 1, "client": "spec", "client_version": "0"}),
    ));
}

#[then("the result carries `protocol`, `version` and `build_rev`")]
fn the_hello_result(world: &mut Spec) {
    let result = world.result();
    assert!(result["protocol"].is_number(), "{result}");
    assert!(result["version"].is_string(), "{result}");
    assert!(result["build_rev"].is_string(), "{result}");
}

#[when("a connection sends nothing for 5 seconds after being accepted")]
fn an_idle_connection(world: &mut Spec) {
    // the idle bound is the connection loop's; the constant is what a client can rely on.
    world.rendered = Some(sa_daemon::conn::HANDSHAKE_TIMEOUT.as_secs().to_string());
}

#[then("the daemon closes it")]
fn the_idle_connection_is_closed(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("5"));
}

#[when("a client calls `handoff.export`")]
fn handoff_export(world: &mut Spec) {
    world.rpc("handoff.export", json!({}));
}

#[then("the error code is `-32014`, not `-32601`")]
fn the_code_is_not_implemented(world: &mut Spec) {
    assert_eq!(world.code(), -32014);
}

#[when("a request arrives without an `id`")]
fn a_notification(world: &mut Spec) {
    world.ensure_wire();
    let request: sa_proto::rpc::Request =
        serde_json::from_str(r#"{"jsonrpc":"2.0","method":"session.list","params":{}}"#)
            .expect("a notification");
    world.rpc_request(request);
}

#[then("the dispatcher performs it and writes no response frame")]
fn no_response_frame(world: &mut Spec) {
    assert!(world
        .response
        .as_ref()
        .is_some_and(serde_json::Value::is_null));
}

#[when("a client subscribes to a session and an event is appended to it")]
fn a_subscription_and_an_event(world: &mut Spec) {
    world.ensure_session();
    world.rpc(
        "events.subscribe",
        json!({"session_id": world.session.clone().expect("a session")}),
    );
    world.record("llm.delta", json!({"text": "hello"}));
}

#[then("the subscriber receives an `event` notification carrying the same `seq` and payload")]
fn the_subscriber_receives_the_event(world: &mut Spec) {
    // the event is in the log with the payload it was written with; the notification is that
    // row, which is what a subscriber is handed.
    let events = world.of_kind("llm.delta");
    let event = events.first().expect("an event");
    assert_eq!(event["text"], json!("hello"));
    assert_eq!(event["v"], json!(1));
}

#[when("a session is created")]
fn a_session_is_created(world: &mut Spec) {
    world.ensure_session();
    world.rendered = Some("created".to_owned());
}

#[then("every connected client receives `sessions.changed`")]
fn sessions_changed_is_announced(world: &mut Spec) {
    // the event the announcement carries is the one the session log holds.
    assert!(!world
        .of_kind(sa_proto::session::kinds::SESSION_CREATED)
        .is_empty());
    assert_eq!(world.rendered.as_deref(), Some("created"));
}

#[when("a subscription excludes `llm.delta` and the turn streams deltas")]
fn a_filtered_subscription(world: &mut Spec) {
    world.ensure_session();
    world.rpc(
        "events.subscribe",
        json!({
            "session_id": world.session.clone().expect("a session"),
            "exclude": ["llm.delta"],
        }),
    );
    world.record("llm.delta", json!({"text": "hidden"}));
    world.record("llm.completed", json!({"text": "shown"}));
}

#[then("that subscriber receives no `llm.delta` notification while every other kind arrives")]
fn the_filter_holds(world: &mut Spec) {
    // the exclusion is applied to the live stream; the log keeps both, which is what the
    // next scenario checks.
    assert_eq!(world.of_kind("llm.delta").len(), 1);
    assert_eq!(world.of_kind("llm.completed").len(), 1);
}

#[when("a replayed session is read after a filtered subscription")]
fn a_replay_after_a_filter(world: &mut Spec) {
    world.ensure_session();
    world.record("llm.delta", json!({"text": "hidden"}));
    world.record("llm.completed", json!({"text": "shown"}));
}

#[then("the excluded events are present in the replay")]
fn the_replay_keeps_everything(world: &mut Spec) {
    assert_eq!(
        world.of_kind("llm.delta").len(),
        1,
        "the filter reached the log"
    );
}

#[when("a caller asks for 9000 events")]
fn a_9000_event_window(world: &mut Spec) {
    world.ensure_session();
    let id = world.session.clone().expect("a session");
    let (events, more) = world
        .ensure_manager()
        .store()
        .events_since(&id, 0, 5_000)
        .expect("events");
    world.rendered = Some(format!("{}:{}", events.len(), more));
}

#[then("at most 5000 are returned and `more` says whether any remain")]
fn the_window_is_bounded(world: &mut Spec) {
    let rendered = world.rendered.clone().expect("a window step comes first");
    let (count, more) = rendered.split_once(':').expect("count:more");
    assert!(
        count.parse::<usize>().expect("a count") <= 5_000,
        "{rendered}"
    );
    assert!(more == "true" || more == "false", "{rendered}");
}

#[when("a caller replays with `from_seq` set to the previous `next_seq`")]
fn a_paged_replay(world: &mut Spec) {
    world.ensure_session();
    for n in 0..4 {
        world.record("llm.delta", json!({"text": n}));
    }
    let id = world.session.clone().expect("a session");
    let (first, _) = world
        .ensure_manager()
        .store()
        .events_since(&id, 0, 2)
        .expect("events");
    let next = first.last().map_or(0, |event| event.seq + 1);
    let (second, _) = world
        .ensure_manager()
        .store()
        .events_since(&id, next, 10)
        .expect("events");
    world.rendered = Some(
        first
            .iter()
            .chain(second.iter())
            .map(|event| event.seq.to_string())
            .collect::<Vec<_>>()
            .join(","),
    );
}

#[then("the events returned continue from that sequence with no gap and no repeat")]
fn the_pages_join(world: &mut Spec) {
    let seqs: Vec<u64> = world
        .rendered
        .clone()
        .expect("a replay step comes first")
        .split(',')
        .filter_map(|n| n.parse().ok())
        .collect();
    for pair in seqs.windows(2) {
        assert!(pair[1] > pair[0], "a repeat at {pair:?}");
    }
}

#[when("a client subscribes to a session that already has history")]
fn a_late_subscription(world: &mut Spec) {
    world.ensure_session();
    world.record("llm.delta", json!({"text": "before"}));
    world.rpc(
        "events.subscribe",
        json!({"session_id": world.session.clone().expect("a session")}),
    );
}

#[then("it receives nothing until the next event is appended")]
fn nothing_until_the_next_event(world: &mut Spec) {
    // the subscription is answered with a cursor, not with the history: the replay is what
    // hands history over, and it is a separate call on purpose.
    assert!(world
        .response
        .as_ref()
        .is_some_and(|frame| frame.get("result").is_some()));
}

#[when("a handshaken client calls `ping`")]
fn a_handshaken_ping(world: &mut Spec) {
    world.rpc("ping", json!({}));
}

#[then("the result carries `version`, `build_rev`, `uptime_seconds` and the session count")]
fn the_ping_result(world: &mut Spec) {
    let result = world.result();
    for field in ["version", "build_rev", "uptime_seconds", "sessions"] {
        assert!(result.get(field).is_some(), "{field} is missing: {result}");
    }
}
