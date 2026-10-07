//! Records the git commit the simulator is built from, so results can name it.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    // Rebuild when HEAD moves: it changes on checkout, and the branch's ref on commit.
    let mut watched = vec![format!("{dir}/HEAD"), format!("{dir}/packed-refs")];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(format!("{dir}/{branch}"));
    }
    for path in watched.iter().filter(|p| Path::new(p).exists()) {
        println!("cargo:rerun-if-changed={path}");
    }
    if let Some(commit) = git(&["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=TEMPEST_GIT_COMMIT={commit}");
    }
}
