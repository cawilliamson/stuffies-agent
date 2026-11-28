//! addressing a line by the hash of its own text (spec 7).

use cucumber::{then, when};
use serde_json::json;

use crate::world::Spec;

/// the hash a hashed line carries, or a panic naming the line.
fn hash_of(line: &str) -> String {
    line.split_once('|')
        .unwrap_or_else(|| panic!("not a hashed line: {line}"))
        .0
        .to_owned()
}

#[when("a file is read")]
async fn a_file_is_read(world: &mut Spec) {
    world.write_file("addr.txt", "alpha\nbeta\ngamma\n");
    world.call_tool("read", json!({"path": "addr.txt"})).await;
}

#[then("each line is `hhhhhh|content` and the hash is derived from the content alone")]
fn each_line_is_addressable(world: &mut Spec) {
    let answered = world.answered().to_owned();
    let line = answered.lines().next().expect("a line");
    let (hash, content) = line.split_once('|').expect("hash|content");
    assert_eq!(content, "alpha");
    assert_eq!(hash.len(), 6, "a six character hash: {line:?}");
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{line:?}");
    // the same text carries the same address wherever it is.
    let again = sa_core::hashline::hash_line("alpha");
    assert_eq!(hash, again, "the hash is not derived from the content");
}

#[when("`read` is called with `hashline: false`")]
async fn read_without_hashes(world: &mut Spec) {
    world.write_file("raw.txt", "alpha\nbeta\n");
    world
        .call_tool("read", json!({"path": "raw.txt", "hashline": false}))
        .await;
}

#[then("the lines carry no prefix")]
fn the_lines_carry_no_prefix(world: &mut Spec) {
    assert_eq!(world.answered().trim_end(), "alpha\nbeta");
}

#[when("an edit names a hash for a line that has shifted position since it was read")]
async fn an_edit_names_a_shifted_hash(world: &mut Spec) {
    world.write_file("shift.txt", "one\ntwo\nthree\n");
    world.call_tool("read", json!({"path": "shift.txt"})).await;
    let read = world.answered().to_owned();
    let hash = hash_of(read.lines().nth(1).expect("the second line"));
    // a line is inserted above it: the position changed, the text did not.
    world.write_file("shift.txt", "zero\none\ntwo\nthree\n");
    world
        .call_tool(
            "edit",
            json!({"path": "shift.txt", "edits": [{"hash": hash, "content": "TWO"}]}),
        )
        .await;
}

#[then("the line is found and replaced")]
fn the_line_is_replaced(world: &mut Spec) {
    assert!(world.answered().contains("TWO"), "{}", world.answered());
    assert!(world.read_file("shift.txt").contains("TWO"));
}

#[when("an edit names a hash no line currently carries")]
async fn an_edit_names_a_dead_hash(world: &mut Spec) {
    world.write_file("dead.txt", "one\ntwo\n");
    world
        .call_tool(
            "edit",
            json!({"path": "dead.txt", "edits": [{"hash": "abcdef", "content": "X"}]}),
        )
        .await;
}

#[then("the edit is refused and nothing is written")]
fn the_edit_is_refused(world: &mut Spec) {
    assert!(!world.refused().is_empty());
    assert_eq!(world.read_file("dead.txt"), "one\ntwo\n");
}

#[when("an edit is refused because the hash is stale")]
async fn an_edit_is_refused_stale(world: &mut Spec) {
    world.write_file("stale.txt", "one\ntwo\nthree\n");
    world.call_tool("read", json!({"path": "stale.txt"})).await;
    let read = world.answered().to_owned();
    let hash = hash_of(read.lines().nth(1).expect("the second line"));
    world.write_file("stale.txt", "one\nchanged\nthree\n");
    world
        .call_tool(
            "edit",
            json!({"path": "stale.txt", "edits": [{"hash": hash, "content": "X", "line": 2}]}),
        )
        .await;
}

#[then("the result carries the hashed lines around the intended position")]
fn the_result_carries_neighbours(world: &mut Spec) {
    let error = world.refused().to_owned();
    assert!(
        error.contains('|'),
        "no hashed lines in the refusal: {error}"
    );
}

#[when("a refusal has no line to aim at and the file is short")]
async fn a_refusal_with_no_target(world: &mut Spec) {
    world.write_file("short.txt", "one\ntwo\n");
    world
        .call_tool(
            "edit",
            json!({"path": "short.txt", "edits": [{"hash": "zzzzzz", "content": "X"}]}),
        )
        .await;
}

#[then("the whole file is returned hashed")]
fn the_whole_file_is_returned(world: &mut Spec) {
    let error = world.refused().to_owned();
    let hashed = error.lines().filter(|line| line.contains('|')).count();
    assert!(hashed >= 2, "the file was not handed back: {error}");
}

#[when("an edit names a hash carried by three identical lines and gives no `line`")]
async fn an_edit_names_an_ambiguous_hash(world: &mut Spec) {
    world.write_file("same.txt", "same\nsame\nsame\n");
    world.call_tool("read", json!({"path": "same.txt"})).await;
    let read = world.answered().to_owned();
    let hash = hash_of(read.lines().next().expect("a line"));
    world
        .call_tool(
            "edit",
            json!({"path": "same.txt", "edits": [{"hash": hash, "content": "DIFFERENT"}]}),
        )
        .await;
}

#[then("it is refused and all three positions are named")]
fn all_three_positions_named(world: &mut Spec) {
    let error = world.refused().to_owned();
    for line in ["1", "2", "3"] {
        assert!(
            error.contains(line),
            "position {line} is not named: {error}"
        );
    }
}

#[when("an edit supplies the line's text where a hash belongs")]
async fn an_edit_supplies_text(world: &mut Spec) {
    world.write_file("text.txt", "one\ntwo\nthree\n");
    world
        .call_tool(
            "edit",
            json!({"path": "text.txt", "edits": [{"hash": "two", "content": "TWO"}]}),
        )
        .await;
}

#[then("the line it names is found and replaced")]
fn the_named_line_is_replaced(world: &mut Spec) {
    assert!(
        world.read_file("text.txt").contains("TWO"),
        "{}",
        world.answered()
    );
}

#[when("`old` carries a hashline prefix")]
async fn old_carries_a_prefix(world: &mut Spec) {
    world.write_file("prefix.txt", "one\ntwo\n");
    world.call_tool("read", json!({"path": "prefix.txt"})).await;
    let read = world.answered().to_owned();
    let hashed = read.lines().nth(1).expect("the second line").to_owned();
    world
        .call_tool(
            "edit",
            json!({"path": "prefix.txt", "old": hashed, "new": "TWO"}),
        )
        .await;
}

#[then("the prefix is removed and the text matches the file")]
fn the_prefix_is_removed(world: &mut Spec) {
    assert!(
        world.read_file("prefix.txt").contains("TWO"),
        "{}",
        world.answered()
    );
}

#[when("`old` matches nothing but its first non-empty line exists in the file")]
async fn old_matches_nothing(world: &mut Spec) {
    world.write_file("near.txt", "one\ntwo\nthree\n");
    world
        .call_tool(
            "edit",
            json!({"path": "near.txt", "old": "two\nnot here\n", "new": "TWO"}),
        )
        .await;
}

#[then("the result shows the hashed lines around that line")]
fn the_result_shows_the_neighbourhood(world: &mut Spec) {
    let error = world.refused().to_owned();
    assert!(error.contains('|'), "no hashed lines: {error}");
    assert!(error.contains("two"), "the neighbourhood is wrong: {error}");
}
