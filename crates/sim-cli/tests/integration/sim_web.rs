//! `tempest-sim web`: serves the results API over a database `tempest-sim run` recorded, and
//! the results UI's page, whether its files were built into the binary or not.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Stops the server when the test ends, however it ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The status and body of `GET path` from `address` (`host:port`).
fn get(address: &str, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").expect("a header block");
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, body.to_owned())
}

#[test]
fn serves_the_api_and_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("results.duckdb");
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sim/scenarios/two-peers.yaml");
    let tempest = env!("CARGO_BIN_EXE_tempest-sim");
    let recorded = Command::new(tempest)
        .args(["run", "--seeds", "2", "--db"])
        .arg(&db)
        .arg(&scenario)
        .output()
        .unwrap();
    assert!(recorded.status.success(), "{recorded:?}");

    let mut child = Command::new(tempest)
        .args(["web", "--no-open", "--listen", "127.0.0.1:0", "--db"])
        .arg(&db)
        .arg("--baselines")
        .arg(dir.path().join("none"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let _server = Server(child);
    let mut url = String::new();
    BufReader::new(stdout).read_line(&mut url).unwrap();
    let address = url
        .trim()
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix('/'))
        .unwrap_or_else(|| panic!("an address, not {url:?}"))
        .to_owned();

    let (status, body) = get(&address, "/v1/experiments");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"two-peers""#), "{body}");
    let (status, body) = get(&address, "/v1/runs?seed=2");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""total":1"#), "{body}");
    let (status, body) = get(&address, "/v1/compare?variants=1:0");
    assert_eq!(status, 200, "{body}");
    let (status, body) = get(&address, "/v1/runs?sort=nonsense");
    assert_eq!(status, 400, "{body}");
    let (status, body) = get(&address, "/");
    assert_eq!(status, 200);
    assert!(body.contains("<html"), "{body}");
}

#[test]
fn stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tempest-sim"))
        .args(["web", "--no-open", "--listen", "127.0.0.1:0", "--db"])
        .arg(dir.path().join("results.duckdb"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The address is printed once the server listens, after the signal handler is set.
    let stdout = child.stdout.take().unwrap();
    let (sent, printed) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sent.send(line);
    });
    let mut server = Server(child);
    let line = printed.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(line.starts_with("http://"), "{line:?}");

    let pid = server.0.id().to_string();
    let killed = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(killed.success());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success(), "{status:?}");
            return;
        }
        assert!(Instant::now() < deadline, "still serving after SIGTERM");
        std::thread::sleep(Duration::from_millis(50));
    }
}
