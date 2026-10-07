//! `tempest share` and `tempest get` as separate processes on localhost.

use std::fs;
use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use tempest_protocol::Link;

/// How long a sharer may take to print its link.
const STARTUP: Duration = Duration::from_secs(60);

fn tempest() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tempest"));
    // Never touch the real identity, whatever the test passes.
    command.env_remove("TEMPEST_IDENTITY");
    command
}

/// A running `tempest share`, killed when dropped.
struct Sharer {
    child: Child,
    /// The `peer` and `link` lines it printed.
    peer: String,
    link: String,
}

impl Sharer {
    /// Shares `path` on 127.0.0.1 with a port the system picks, and waits for its link. The
    /// identity file is passed with `--identity`, or in the environment if `by_env` is set.
    fn start(path: &Path, identity: &Path, by_env: bool) -> Self {
        let mut command = tempest();
        command.args(["share", "--bind", "127.0.0.1", "--port", "0"]);
        if by_env {
            command.env("TEMPEST_IDENTITY", identity);
        } else {
            command.arg("--identity").arg(identity);
        }
        command
            .arg(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let field = |name: &str| loop {
            let line = received
                .recv_timeout(STARTUP)
                .unwrap_or_else(|_| panic!("share printed no {name} line"));
            if let Some(value) = line.strip_prefix(name) {
                return value.trim().to_owned();
            }
        };
        let peer = field("peer");
        let link = field("link");
        Self { child, peer, link }
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn get(link: &str, output: &Path, identity: &Path, extra: &[&str]) -> Output {
    tempest()
        .args(["get", link, "--output"])
        .arg(output)
        .arg("--identity")
        .arg(identity)
        .args(extra)
        .output()
        .unwrap()
}

/// Asserts that `actual` has the same files, contents, empty directories and symlinks as
/// `expected`.
fn assert_same_tree(expected: &Path, actual: &Path) {
    let names = |dir: &Path| {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(expected), names(actual), "{}", actual.display());
    for name in names(expected) {
        let (want, got) = (expected.join(&name), actual.join(&name));
        let kind = fs::symlink_metadata(&want).unwrap().file_type();
        if kind.is_symlink() {
            assert_eq!(fs::read_link(&want).unwrap(), fs::read_link(&got).unwrap());
        } else if kind.is_dir() {
            assert_same_tree(&want, &got);
        } else {
            assert!(
                fs::read(&want).unwrap() == fs::read(&got).unwrap(),
                "{name:?}"
            );
        }
    }
}

/// A folder with nested and empty directories, an empty file, a file of many chunks and, on
/// Unix, a symlink.
fn sample_folder(root: &Path) {
    fs::create_dir_all(root.join("docs/empty-dir")).unwrap();
    fs::write(root.join("readme.txt"), "hello, swarm\n").unwrap();
    fs::write(root.join("docs/empty.txt"), "").unwrap();
    let big: Vec<u8> = (0..2_500_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(root.join("docs/big.bin"), big).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("../readme.txt", root.join("docs/link")).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn get_downloads_what_share_serves() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photos");
    sample_folder(&source);
    let sharer = Sharer::start(&source, &dir.path().join("sharer.pem"), false);

    let link: Link = sharer.link.parse().unwrap();
    assert_eq!(link.peer.unwrap().to_string(), sharer.peer);
    assert_eq!(link.name.as_deref(), Some("photos"));
    assert!(!link.addrs.is_empty());
    assert!(link.addrs.iter().all(|addr| addr.ip().is_loopback()));

    let output = dir.path().join("downloads");
    let getter = dir.path().join("getter.pem");
    let got = get(&sharer.link, &output, &getter, &[]);
    assert!(got.status.success(), "{}", stderr(&got));
    let folder = output.join("photos");
    assert_eq!(
        String::from_utf8_lossy(&got.stdout).trim(),
        folder.display().to_string()
    );
    assert_same_tree(&source, &folder);

    // A second download refuses to write into the first one's folder.
    let again = get(&sharer.link, &output, &getter, &[]);
    assert!(!again.status.success());
    assert!(stderr(&again).contains("photos"), "{}", stderr(&again));
}

#[test]
fn the_identity_persists_and_can_be_set_by_environment() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    fs::write(&file, "notes").unwrap();
    let identity = dir.path().join("keys/identity.pem");

    let first = Sharer::start(&file, &identity, false).peer.clone();
    let second = Sharer::start(&file, &identity, true).peer.clone();
    assert_eq!(first, second);

    let other = Sharer::start(&file, &dir.path().join("other.pem"), false)
        .peer
        .clone();
    assert_ne!(first, other);
}

#[test]
fn get_needs_a_peer_and_an_address() {
    let dir = tempfile::tempdir().unwrap();
    let identity = dir.path().join("identity.pem");
    let bundle = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    let peer = "5c988adc6dcd30b09514f566977b3500f65b3ff749ef37312ed02545bf4b0823";

    let cases = [
        (format!("tempest:?bundle={bundle}"), "names no peer"),
        (
            format!("tempest:?bundle={bundle}&peer={peer}"),
            "no address",
        ),
        (
            "https://example.com".to_owned(),
            "not a valid tempest: link",
        ),
    ];
    for (link, message) in cases {
        let output = get(&link, dir.path(), &identity, &[]);
        assert!(!output.status.success());
        assert!(stderr(&output).contains(message), "{}", stderr(&output));
    }
}

#[test]
fn get_gives_up_on_an_unreachable_sharer() {
    let dir = tempfile::tempdir().unwrap();
    // A port nobody is listening on.
    let port = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let bundle = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    let peer = "5c988adc6dcd30b09514f566977b3500f65b3ff749ef37312ed02545bf4b0823";
    let link = format!("tempest:?bundle={bundle}&peer={peer}&addr=127.0.0.1:{port}");

    let identity = dir.path().join("identity.pem");
    let output = get(&link, dir.path(), &identity, &["--timeout", "1"]);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains(&format!(
            "could not get the bundle from 127.0.0.1:{port} within 1 s"
        )),
        "{message}"
    );
}

#[test]
fn the_default_port_is_4773() {
    let output = tempest().args(["share", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("[default: 4773]"), "{help}");
}
