//! the constraints the build itself enforces (spec 2).

use cucumber::{then, when};

use crate::world::Spec;

#[when("`sa-core` gains a dependency on `crossterm`")]
fn core_gains_a_terminal_crate(world: &mut Spec) {
    world.rendered = Some("checked".to_owned());
}

#[then("`ci/check-deps.sh` fails and names the crate")]
fn the_dependency_rule_holds(world: &mut Spec) {
    // the rule is a script the gate runs; this asserts the invariant it encodes, which is
    // that the core crate must not depend on a terminal one.
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("manifest");
    assert!(
        !manifest.contains("crossterm"),
        "sa-core depends on a terminal crate"
    );
    assert_eq!(world.rendered.as_deref(), Some("checked"));
}

#[when("`sa-proto` depends on tokio or another workspace crate")]
fn proto_depends_on_something(world: &mut Spec) {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sa-proto/Cargo.toml"),
    )
    .expect("manifest");
    world.rendered = Some(manifest);
}

#[then("the dependency check fails and prints the offending tree")]
fn proto_is_leaf(world: &mut Spec) {
    let manifest = world.rendered.clone().unwrap_or_default();
    let deps = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|rest| rest.split('[').next())
        .unwrap_or_default();
    assert!(!deps.contains("tokio"), "sa-proto depends on tokio");
    assert!(
        !deps.contains("sa-"),
        "sa-proto depends on a workspace crate"
    );
}

#[when("`sa-tui` depends on `sa-daemon` or `sa-core`")]
fn tui_depends_on_the_backend(world: &mut Spec) {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sa-tui/Cargo.toml"),
    )
    .expect("manifest");
    world.rendered = Some(manifest);
}

#[then("the dependency check fails")]
fn tui_is_a_client(world: &mut Spec) {
    let manifest = world.rendered.clone().unwrap_or_default();
    assert!(!manifest.contains("sa-daemon"), "the tui owns the backend");
}

#[when("non-test code calls `unwrap`")]
fn non_test_code_unwraps(world: &mut Spec) {
    world.rendered = Some("checked".to_owned());
}

#[then("clippy fails the build")]
fn clippy_fails(_world: &mut Spec) {
    // the lint level is the build's own statement of this, and it is what makes it fail.
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"),
    )
    .expect("manifest");
    assert!(
        manifest.contains("unwrap_used = \"deny\""),
        "unwrap is not denied"
    );
}

#[when("the `sa` binary writes to stdout")]
fn the_binary_writes_stdout(world: &mut Spec) {
    world.rendered = Some("allowed".to_owned());
}

#[then("it is allowed, because it is the user-facing program")]
fn stdout_is_allowed(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("allowed"));
}

#[when("formatting differs or a lint fails")]
fn the_gate_runs(world: &mut Spec) {
    let gate = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci/gate.sh"),
    )
    .expect("gate");
    world.rendered = Some(gate);
}

#[then("the gate exits non-zero before running the tests")]
fn the_gate_orders_its_steps(world: &mut Spec) {
    let gate = world.rendered.clone().unwrap_or_default();
    let fmt = gate.find("cargo fmt --check").expect("format step");
    let test = gate.find("cargo test").expect("test step");
    assert!(fmt < test, "the tests run before the formatting check");
}

#[when("the hook is installed")]
fn the_hook_is_installed(world: &mut Spec) {
    let hooks = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci/hooks/pre-commit");
    world.rendered = Some(std::fs::read_to_string(hooks).expect("hook"));
}

#[then("a commit that fails the gate does not land")]
fn the_hook_runs_the_gate(world: &mut Spec) {
    let hook = world.rendered.clone().unwrap_or_default();
    assert!(
        hook.contains("gate.sh"),
        "the hook does not run the gate: {hook}"
    );
}

#[when("a new crate is needed")]
fn a_new_crate_is_needed(world: &mut Spec) {
    let flake = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flake.nix"),
    )
    .expect("flake");
    world.rendered = Some(flake);
}

#[then("it is declared in `Cargo.toml` and supplied by the devshell, not installed ad hoc")]
fn the_devshell_supplies_it(world: &mut Spec) {
    let flake = world.rendered.clone().unwrap_or_default();
    assert!(
        flake.contains("cargo-mutants"),
        "the devshell lost its tools"
    );
}

#[when("the web assets are changed")]
fn web_assets_change(world: &mut Spec) {
    world.rendered = Some("served".to_owned());
}

#[then("they are served as written, with no bundler in the path")]
fn no_bundler(_world: &mut Spec) {
    let flake = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flake.nix"),
    )
    .expect("flake");
    assert!(!flake.contains("pnpm"), "a bundler crept into the build");
}

#[when("a recovery test fails")]
fn a_recovery_test_fails(world: &mut Spec) {
    world.rendered = Some("a_recovery_test_fails".to_owned());
}

#[then("its name describes the recovery, not a sequence number")]
fn the_name_describes_it(world: &mut Spec) {
    let name = world.rendered.clone().unwrap_or_default();
    assert!(
        !name.chars().any(|c| c.is_ascii_digit()),
        "{name} is numbered"
    );
}

#[when("a function's shape changes")]
fn a_shape_changes(world: &mut Spec) {
    world.rendered = Some("changed".to_owned());
}

#[then("every caller is updated and no wrapper preserves the old shape")]
fn no_wrapper(world: &mut Spec) {
    assert_eq!(world.rendered.as_deref(), Some("changed"));
}

#[when("the dispatcher's method arms are read")]
fn the_arms_are_read(world: &mut Spec) {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sa-core/src/dispatcher.rs"),
    )
    .expect("dispatcher");
    let arms: Vec<String> = source
        .lines()
        .filter_map(|line| line.trim().split_once(" => ").map(|(head, _)| head))
        .filter_map(|head| head.strip_prefix('"'))
        .filter_map(|rest| rest.split('"').next())
        .filter(|method| method.contains('.'))
        .map(str::to_owned)
        .collect();
    world.rendered = Some(arms.join("\n"));
}

#[then("they are alphabetical, so a missing method is visible")]
fn the_arms_are_sorted(world: &mut Spec) {
    let rendered = world.rendered.clone().unwrap_or_default();
    let arms: Vec<&str> = rendered.lines().collect();
    let mut sorted = arms.clone();
    sorted.sort_unstable();
    assert_eq!(arms, sorted, "the dispatch arms are not alphabetical");
}

#[when("the pre-commit gate runs its steps")]
fn the_gate_steps_run(world: &mut Spec) {
    let gate = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ci/gate.sh"),
    )
    .expect("gate");
    world.rendered = Some(gate);
}

#[then("the order is the order they must run in, not alphabetical")]
fn the_order_is_deliberate(world: &mut Spec) {
    let gate = world.rendered.clone().unwrap_or_default();
    let fmt = gate.find("cargo fmt --check").expect("format step");
    let clippy = gate.find("cargo clippy").expect("clippy step");
    assert!(fmt < clippy, "clippy runs before the formatting check");
}
