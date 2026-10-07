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
    /// Shares `path` on 127.0.0.1 with a port the system picks and LAN discovery off, and waits
    /// for its link. The identity file is passed with `--identity`, or in the environment if
    /// `by_env` is set.
    fn start(path: &Path, identity: &Path, by_env: bool) -> Self {
        Self::start_with(path, identity, by_env, &["--no-lan"])
    }

    /// [`Sharer::start`] with `extra` arguments.
    fn start_with(path: &Path, identity: &Path, by_env: bool, extra: &[&str]) -> Self {
        let mut command = tempest();
        command.args(["share", "--bind", "127.0.0.1", "--port", "0"]);
        command.args(extra);
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
        let received = lines(child.stdout.take().unwrap());
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

/// The lines `stream` yields, read on another thread.
fn lines(stream: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    received
}

/// A running `tempest tracker` on localhost, killed when dropped.
struct TrackerProcess {
    child: Child,
    url: String,
}

impl TrackerProcess {
    fn start() -> Self {
        let mut child = tempest()
            .args(["tracker", "--bind", "127.0.0.1", "--port", "0"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let received = lines(child.stderr.take().unwrap());
        let line = received
            .recv_timeout(STARTUP)
            .expect("the tracker printed nothing");
        let url = line
            .strip_prefix("tracker listening on ")
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("unexpected line: {line}"))
            .to_owned();
        Self { child, url }
    }
}

impl Drop for TrackerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    let got = get(&sharer.link, &output, &getter, &["--no-lan"]);
    assert!(got.status.success(), "{}", stderr(&got));
    let folder = output.join("photos");
    assert_eq!(
        String::from_utf8_lossy(&got.stdout).trim(),
        folder.display().to_string()
    );
    assert_same_tree(&source, &folder);

    // A second download refuses to write into the first one's folder.
    let again = get(&sharer.link, &output, &getter, &["--no-lan"]);
    assert!(!again.status.success());
    assert!(stderr(&again).contains("photos"), "{}", stderr(&again));
}

#[test]
fn get_finds_the_sharer_through_a_tracker() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photos");
    sample_folder(&source);
    let tracker = TrackerProcess::start();
    let identity = dir.path().join("sharer.pem");
    let sharer = Sharer::start_with(
        &source,
        &identity,
        false,
        &["--tracker", &tracker.url, "--no-lan"],
    );
    let link: Link = sharer.link.parse().unwrap();
    assert_eq!(link.trackers, std::slice::from_ref(&tracker.url));

    // Only the bundle and the tracker: the sharer is found by asking the tracker.
    let mut bare = Link::new(link.bundle);
    bare.trackers = link.trackers.clone();
    let output = dir.path().join("downloads");
    let got = get(
        &bare.to_string(),
        &output,
        &dir.path().join("getter.pem"),
        &["--no-lan"],
    );
    assert!(got.status.success(), "{}", stderr(&got));
    assert_same_tree(&source, &output.join("photos"));
}

/// A port of the test's own for LAN discovery, so concurrent tests don't hear each other.
fn lan_port() -> String {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
        .to_string()
}

#[test]
fn get_finds_the_sharer_on_the_local_network() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photos");
    sample_folder(&source);
    let lan_port = lan_port();
    let identity = dir.path().join("sharer.pem");
    let sharer = Sharer::start_with(&source, &identity, false, &["--lan-port", &lan_port]);

    // Only the bundle: the sharer is found through LAN discovery, on loopback.
    let bare = Link::new(sharer.link.parse::<Link>().unwrap().bundle);
    let output = dir.path().join("downloads");
    let getter = dir.path().join("getter.pem");
    let extra = ["--bind", "127.0.0.1", "--lan-port", &lan_port];
    let got = get(&bare.to_string(), &output, &getter, &extra);
    assert!(got.status.success(), "{}", stderr(&got));
    assert_same_tree(&source, &output.join("photos"));
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
fn get_needs_a_peer_with_an_address_a_tracker_or_lan_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let identity = dir.path().join("identity.pem");
    let bundle = "207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    let peer = "5c988adc6dcd30b09514f566977b3500f65b3ff749ef37312ed02545bf4b0823";

    let cases = [
        (format!("tempest:?bundle={bundle}"), "names neither"),
        (
            format!("tempest:?bundle={bundle}&peer={peer}"),
            "names neither",
        ),
        (
            "https://example.com".to_owned(),
            "not a valid tempest: link",
        ),
    ];
    for (link, message) in cases {
        let output = get(&link, dir.path(), &identity, &["--no-lan"]);
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
    let output = get(
        &link,
        dir.path(),
        &identity,
        &["--timeout", "1", "--no-lan"],
    );
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
fn get_gives_up_when_nobody_on_the_local_network_has_the_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let link = "tempest:?bundle=207855f013f5a4066c9987ee3fa4b0ae63c8eeee02035634587286457cf5b85a";
    let identity = dir.path().join("identity.pem");
    let lan_port = lan_port();
    let extra = [
        "--timeout",
        "1",
        "--bind",
        "127.0.0.1",
        "--lan-port",
        &lan_port,
    ];
    let output = get(link, dir.path(), &identity, &extra);
    assert!(!output.status.success());
    let message = stderr(&output);
    assert!(
        message.contains("could not get the bundle from peers on the local network within 1 s"),
        "{message}"
    );
}

#[test]
fn the_default_port_is_4773() {
    let output = tempest().args(["share", "--help"]).output().unwrap();
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("[default: 4773]"), "{help}");
}

/// A running `tempest relay` on 127.0.0.1, killed when dropped.
struct RelayProcess {
    child: Child,
    /// How peers name it: `<key>@127.0.0.1:<port>`.
    identity: String,
}

impl RelayProcess {
    fn start(identity: &Path) -> Self {
        let mut child = tempest()
            .args(["relay", "--bind", "127.0.0.1", "--port", "0"])
            .args(["--public-ip", "127.0.0.1", "--identity"])
            .arg(identity)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let received = lines(child.stderr.take().unwrap());
        let identity = loop {
            let line = received
                .recv_timeout(STARTUP)
                .expect("the relay printed no identity");
            if let Some(identity) = line.strip_prefix("peers name it ") {
                break identity.to_owned();
            }
        };
        Self { child, identity }
    }
}

impl Drop for RelayProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn get_reaches_the_sharer_through_its_relay() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("photos");
    sample_folder(&source);
    let relay = RelayProcess::start(&dir.path().join("relay.pem"));
    let sharer = Sharer::start_with(
        &source,
        &dir.path().join("sharer.pem"),
        false,
        &["--no-lan", "--relay", &relay.identity],
    );
    let link: Link = sharer.link.parse().unwrap();
    let relay_address = link.relay.expect("the link has the relay address");
    assert!(relay_address.ip().is_loopback());

    // Without its direct addresses, the sharer can only be reached through the relay.
    let relayed = Link {
        addrs: Vec::new(),
        ..link
    };
    let output = dir.path().join("downloads");
    let got = get(
        &relayed.to_string(),
        &output,
        &dir.path().join("getter.pem"),
        &["--no-lan"],
    );
    assert!(got.status.success(), "{}", stderr(&got));
    assert_same_tree(&source, &output.join("photos"));
}
