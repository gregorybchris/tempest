//! `tempest web`: with no daemon it runs the engine, which serves the web UI beside its API; a
//! second `tempest web` finds it. Whether the UI's files were built into the binary or not, `/`
//! answers, and every file the page names is served.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// How long a process may take to start or answer.
const LIMIT: Duration = Duration::from_secs(60);

fn tempest(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tempest"));
    command
        .env_remove("TEMPEST_IDENTITY")
        .env_remove("TEMPEST_CONFIG")
        .env("TEMPEST_DATA_DIR", data_dir);
    command
}

/// An HTTP answer.
struct Answer {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// `GET path` from the server at `address` (`host:port`), over HTTP/1.1 without keep-alive.
fn get(address: &str, path: &str) -> Answer {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(LIMIT)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header block");
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let (status_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    let status = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let headers = headers.to_owned();
    let mut body = raw[split + 4..].to_vec();
    if headers
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = unchunk(&body);
    }
    Answer {
        status,
        headers,
        body,
    }
}

fn unchunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = body.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..line_end]).unwrap(), 16).unwrap();
        if size == 0 {
            return out;
        }
        let start = line_end + 2;
        out.extend_from_slice(&body[start..start + size]);
        body = &body[start + size + 2..];
    }
}

/// The paths of the files a page loads: links and the module script's imports, from `/`.
fn linked(html: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for attribute in ["href=\"/", "src=\"/", "from '/", "module_or_path: '/"] {
        for (i, _) in html.match_indices(attribute) {
            let start = i + attribute.len() - 1;
            let rest = &html[start..];
            let end = rest.find(['"', '\'']).unwrap();
            paths.push(rest[..end].to_owned());
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn tempest_web_serves_the_ui_beside_the_api() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = tempest(dir.path())
        .args([
            "web",
            "--no-open",
            "--set",
            "api.port=0",
            "--set",
            "peers.port=0",
        ])
        .args([
            "--set",
            r#"peers.bind=["127.0.0.1"]"#,
            "--set",
            "lan.enabled=false",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut first = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut first)
        .unwrap();
    let running = Running(child);
    let url = first.trim().to_owned();
    let address = url.strip_prefix("http://").expect("a local URL").to_owned();

    let page = get(&address, "/");
    assert_eq!(page.status, 200);
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(page.header("x-frame-options"), Some("DENY"));
    let html = page.text();
    let files = linked(&html);
    if html.contains("built without its web UI") {
        assert!(files.is_empty(), "the fallback page stands alone");
    } else {
        assert!(
            files.iter().any(|f| f.ends_with(".wasm")),
            "the page loads its WebAssembly: {files:?}"
        );
        for file in files {
            let answer = get(&address, &file);
            assert_eq!(answer.status, 200, "{file}");
            assert!(!answer.body.is_empty(), "{file}");
            if file.ends_with(".wasm") {
                assert_eq!(answer.header("content-type"), Some("application/wasm"));
                assert_eq!(
                    answer.header("cache-control"),
                    Some("public, max-age=31536000, immutable")
                );
            }
        }
    }

    // The API answers beside it, and keeps the paths the UI doesn't use.
    let engine = get(&address, "/v1/engine");
    assert_eq!(engine.status, 200);
    assert!(engine.text().contains("peer_id"));
    let missing = get(&address, "/no-such-file.js");
    assert_eq!(missing.status, 404);
    assert!(missing.text().contains("not_found"));

    // A second `tempest web` finds the engine the first runs.
    let found = tempest(dir.path())
        .args(["web", "--no-open"])
        .output()
        .unwrap();
    assert!(found.status.success());
    assert_eq!(String::from_utf8_lossy(&found.stdout).trim(), url);
    drop(running);
}
