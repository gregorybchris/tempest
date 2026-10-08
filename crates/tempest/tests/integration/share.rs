//! `tempest share --dry-run` on a real folder.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

const TIME: u64 = 1_700_000_000_000;

fn tempest(args: &[&str], path: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tempest"))
        .args(args)
        .arg(path)
        .output()
        .unwrap()
}

fn write_file(path: &Path, contents: &[u8]) {
    fs::write(path, contents).unwrap();
    let time = SystemTime::UNIX_EPOCH + Duration::from_millis(TIME);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

/// Rebuilds the manifest test vector from `protocol/content.md` on disk and checks that sharing
/// it gives the vector's bundle ID.
#[cfg(unix)]
#[test]
fn dry_run_reproduces_the_manifest_vector() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("example");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::create_dir(root.join("empty-dir")).unwrap();
    write_file(&root.join("a.txt"), b"abc");
    write_file(&root.join("docs/empty.txt"), b"");
    std::os::unix::fs::symlink("../a.txt", root.join("docs/link")).unwrap();

    let output = tempest(
        &["share", "--dry-run", "--created", &TIME.to_string()],
        &root,
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");

    let bundle = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    assert!(
        stdout.contains(&format!("bundle    {bundle}\n")),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "link      tempest:?bundle={bundle}&name=example\n"
        )),
        "{stdout}"
    );
    assert!(stdout.contains("manifest  212 bytes"), "{stdout}");
}

#[test]
fn dry_run_shares_a_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Holiday Photos.zip");
    write_file(&path, b"not really a zip");

    let output = tempest(&["share", "--dry-run"], &path);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("&name=Holiday%20Photos.zip\n"), "{stdout}");
    assert!(stdout.contains("size      16 bytes\n"), "{stdout}");
    assert!(stdout.contains("files     1 file\n"), "{stdout}");
}

#[test]
fn invalid_names_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    write_file(&dir.path().join("CON.txt"), b"x");
    let output = tempest(&["share", "--dry-run"], dir.path());
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("CON.txt") && stderr.contains("reserved"),
        "{stderr}"
    );
}
