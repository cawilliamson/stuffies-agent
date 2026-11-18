//! stamps the build with the commit it was built from, so a client can tell whether the
//! daemon it is talking to is the same code.

use std::process::Command;

fn main() {
    // the nix build has no .git, so it passes the revision in explicitly.
    let rev = std::env::var("SA_BUILD_REV")
        .ok()
        .filter(|rev| !rev.trim().is_empty())
        .or_else(git_rev)
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SA_BUILD_REV={rev}");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SA_BUILD_REV");
}

fn git_rev() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rev = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!rev.is_empty()).then_some(rev)
}
