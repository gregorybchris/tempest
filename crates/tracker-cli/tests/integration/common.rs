//! Running `tempest-tracker` as a process, and talking to it over HTTP.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;

/// How long anything here may take.
pub const LIMIT: Duration = Duration::from_secs(60);

pub fn tracker() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tempest-tracker"))
}

/// A child process, killed when dropped.
pub struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The lines `stream` yields, read on another thread.
/// It keeps reading once the receiver is gone, so the child never writes to a closed pipe,
/// which would make its next `eprintln!` panic.
fn lines(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (lines, received) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let _ = lines.send(line);
        }
    });
    received
}

/// Starts a tracker with listings (a catalog file) in `dir` and returns it with its URL.
pub fn start(dir: &Path) -> (Running, String) {
    start_on(&dir.join("catalog.db"))
}

/// Starts a tracker with the catalog `file`.
pub fn start_on(file: &Path) -> (Running, String) {
    let mut child = Running(
        tracker()
            .args(["--bind", "127.0.0.1", "--port", "0", "--catalog"])
            .arg(file)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let received = lines(child.0.stderr.take().unwrap());
    // Log events come first.
    let url = std::iter::from_fn(|| received.recv_timeout(LIMIT).ok())
        .find_map(|line| {
            let rest = line.strip_prefix("tracker listening on ")?;
            Some(rest.split(';').next()?.to_owned())
        })
        .expect("the tracker never said where it listens");
    (child, url)
}

/// `GET` of `path` on the tracker at `url`: the status and the JSON body.
pub fn get(url: &str, path: &str) -> (u16, Value) {
    let address = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}
