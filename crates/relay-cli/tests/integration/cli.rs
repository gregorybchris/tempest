//! `tempest-relay` as a process.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn relay() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tempest-relay"))
}

#[test]
fn the_version_names_the_program() {
    let output = relay().arg("--version").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("tempest-relay "), "{text}");
}

/// A relay starts, says how peers name it, and on SIGTERM stops cleanly.
#[test]
fn the_relay_says_how_peers_name_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = relay()
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--public-ip",
            "127.0.0.1",
        ])
        .arg("--identity")
        .arg(dir.path().join("relay.pem"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap())
        .lines()
        .map_while(Result::ok);
    let named = lines
        .by_ref()
        .find_map(|line| line.strip_prefix("peers name it ").map(str::to_owned));
    let Some(named) = named else {
        let _ = child.kill();
        panic!("the relay printed no identity: {:?}", child.wait());
    };
    let (key, address) = named.split_once('@').expect("<key>@<host>:<port>");
    assert_eq!(key.len(), 64, "{named}");
    assert!(address.starts_with("127.0.0.1:"), "{named}");

    #[cfg(unix)]
    {
        let sent = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(sent.success());
        assert!(lines.any(|line| line == "stopped"), "no clean stop");
        assert!(child.wait().unwrap().success());
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}
