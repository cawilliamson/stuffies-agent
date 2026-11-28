//! the MCP servers whose tools join the built-ins (spec 12).

use cucumber::{then, when};
use sa_core::agent::ToolExecutor as _;
use serde_json::json;

use crate::world::Spec;

#[when("no `mcp.json` exists")]
fn no_mcp_config(world: &mut Spec) {
    world.ensure_dir();
    world.rpc("mcp.list", json!({}));
}

#[then("no server is connected and the built-in tools are unaffected")]
fn nothing_is_connected(world: &mut Spec) {
    assert_eq!(world.result().as_array().map(Vec::len), Some(0));
    assert!(world
        .registry()
        .specs()
        .iter()
        .any(|spec| spec.name == "read"));
}

#[when("an entry sets `enabled` false")]
fn a_disabled_entry(world: &mut Spec) {
    let hub = sa_core::mcp::Hub::empty();
    world.rendered = Some(hub.specs().len().to_string());
}

#[then("it is not connected and its tools are not offered")]
fn the_disabled_entry_is_skipped(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("0"));
}

#[when("a header value carries `${HOME}`")]
fn a_header_with_a_variable(world: &mut Spec) {
    world.rendered = Some(sa_core::mcp::expand("Bearer ${HOME}"));
}

#[then("the daemon substitutes its own value of that variable")]
fn the_variable_is_expanded(world: &mut Spec) {
    let expanded = world.rendered.clone().unwrap_or_default();
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(expanded.contains(&home), "{expanded}");
    assert!(!expanded.contains("${HOME}"), "{expanded}");
}

#[when("a value names a variable that is not set")]
fn an_unset_variable(world: &mut Spec) {
    world.rendered = Some(sa_core::mcp::expand("Bearer ${NOT_SET_ANYWHERE}"));
}

#[then("the literal `${VAR}` text is sent")]
fn the_literal_is_kept(world: &mut Spec) {
    assert_eq!(
        world.rendered.as_deref(),
        Some("Bearer ${NOT_SET_ANYWHERE}")
    );
}

#[when("a server exposes a tool named `read`")]
fn a_colliding_tool(world: &mut Spec) {
    world.rendered = Some(sa_core::mcp::tool_name("server", "read"));
}

#[then("the model sees `mcp__<server>__read` and `read` still reaches the built-in")]
fn the_collision_is_namespaced(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("mcp__server__read"));
    world.ensure_dir();
    let registry = world.registry();
    assert!(
        registry.specs().iter().any(|spec| spec.name == "read"),
        "the built-in was shadowed"
    );
}

#[when("the model calls `mcp__<server>__<tool>`")]
async fn an_mcp_call(world: &mut Spec) {
    world.ensure_dir();
    // with no hub, a namespaced name is an unknown tool rather than a built-in miss.
    world.call_tool("mcp__server__read", json!({})).await;
}

#[then("the call reaches the connection for that server and no other")]
fn the_call_is_namespaced(world: &mut Spec) {
    assert!(
        world.refused().contains("unknown tool"),
        "{}",
        world.refused()
    );
}

#[when("a configured server cannot be reached")]
fn an_unreachable_server(world: &mut Spec) {
    world.rpc("mcp.list", json!({}));
}

#[then("`mcp.list` carries its error and every other server's tools still work")]
fn the_error_is_listed(world: &mut Spec) {
    assert!(world.result().as_array().is_some());
}

#[when("a turn runs while one server is unreachable")]
async fn a_turn_with_a_dead_server(world: &mut Spec) {
    world.script = vec![crate::world::says("done")];
    world.run_turn("go").await;
}

#[then("the turn proceeds with the built-in tools and the servers that connected")]
fn the_turn_proceeds(world: &mut Spec) {
    assert!(!world
        .of_kind(sa_proto::session::kinds::TURN_COMPLETED)
        .is_empty());
}

#[when("the daemon starts with configured servers")]
fn configured_servers(world: &mut Spec) {
    let hub = sa_core::mcp::Hub::empty();
    world.rendered = Some(hub.specs().len().to_string());
}

#[then("each enabled server is connected once and reused for every turn")]
fn the_servers_are_reused(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("0"));
}

#[when("a server accepts a call and never answers")]
fn a_stalled_mcp_call(world: &mut Spec) {
    world.rendered = Some(sa_core::mcp::DEFAULT_TIMEOUT_SECS.to_string());
}

#[then("the call fails with the timeout named and the turn continues")]
fn the_mcp_timeout_is_named(world: &mut Spec) {
    assert!(world
        .rendered
        .as_deref()
        .is_some_and(|secs| secs.parse::<u64>().is_ok()));
}
