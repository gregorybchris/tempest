//! `tempest share --publish` against `tempest tracker --catalog`, as separate processes on
//! localhost: the bundle shows in the catalog with its content and the metadata given.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long anything here may take.
const LIMIT: Duration = Duration::from_secs(60);

fn tempest() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tempest"));
    // Never touch the real identity or engine, whatever the test passes.
    command
        .env_remove("TEMPEST_IDENTITY")
        .env_remove("TEMPEST_CONFIG")
        .env_remove("TEMPEST_DATA_DIR");
    command
}

/// The lines `stream` yields, read on another thread.
fn lines(stream: impl Read + Send + 'static) -> mpsc::Receiver<String> {
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

/// A child process, killed when dropped.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `tempest tracker` with a catalog in `dir` and returns it with its URL.
fn start_tracker(dir: &Path) -> (Running, String) {
    let mut child = tempest()
        .args(["tracker", "--bind", "127.0.0.1", "--port", "0", "--catalog"])
        .arg(dir.join("catalog.db"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let received = lines(child.stderr.take().unwrap());
    let line = received
        .recv_timeout(LIMIT)
        .expect("the tracker printed nothing");
    let url = line
        .strip_prefix("tracker listening on ")
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("unexpected line: {line}"))
        .to_owned();
    (Running(child), url)
}

/// `GET` of `path` on the tracker at `url`: the status and the JSON body.
fn get(url: &str, path: &str) -> (u16, Value) {
    request(url, "GET", path, "")
}

/// A request to the tracker at `url`, with `body` as JSON unless it is empty: the status and
/// the JSON body.
fn request(url: &str, method: &str, path: &str, body: &str) -> (u16, Value) {
    let address = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

#[test]
fn a_published_share_appears_in_the_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let (_tracker, url) = start_tracker(dir.path());
    let folder = dir.path().join("Rainfall 2025");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("january.csv"), "day,mm\n1,4.2\n").unwrap();
    std::fs::write(folder.join("february.csv"), "day,mm\n1,0.0\n2,1.5\n").unwrap();

    let mut child = tempest()
        .args(["share", "--bind", "127.0.0.1", "--port", "0", "--no-lan"])
        .arg("--no-port-mapping")
        .arg("--identity")
        .arg(dir.path().join("identity.pem"))
        .args(["--publish", &url, "--title", "Rainfall, 2025"])
        .args(["--description", "Daily rainfall by month."])
        .args(["--category", "datasets", "--tag", "weather", "--tag", "CSV"])
        .arg(&folder)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let received = lines(child.stdout.take().unwrap());
    let _sharer = Running(child);
    let mut printed = Vec::new();
    let link = loop {
        let line = received
            .recv_timeout(LIMIT)
            .unwrap_or_else(|_| panic!("share printed no link: {printed:#?}"));
        if let Some(link) = line.strip_prefix("link") {
            break link.trim().to_owned();
        }
        printed.push(line);
    };
    let catalog_line = printed.iter().find(|line| line.starts_with("catalog"));
    assert!(
        catalog_line.is_some_and(|line| line.contains("pending")),
        "{printed:#?}"
    );
    let bundle = link
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix("bundle="))
        .unwrap()
        .to_owned();
    assert!(
        link.contains("&tracker="),
        "the link names the tracker: {link}"
    );

    // Nobody signed in, as anyone browsing: shown once the tracker has fetched the manifest.
    let deadline = Instant::now() + LIMIT;
    let listing = loop {
        let (status, body) = get(&url, &format!("/catalog/v1/listings/{bundle}"));
        if status == 200 {
            break body;
        }
        assert_eq!(status, 404, "{body}");
        assert!(Instant::now() < deadline, "never listed");
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(listing["status"], "listed");
    assert_eq!(listing["title"], "Rainfall, 2025");
    assert_eq!(listing["description"], "Daily rainfall by month.");
    assert_eq!(listing["category"], "datasets");
    assert_eq!(listing["tags"], serde_json::json!(["weather", "csv"]));
    assert_eq!(listing["content"]["name"], "Rainfall 2025");
    assert_eq!(listing["content"]["files"], 2);
    assert_eq!(listing["content"]["size"], 32);
    assert_eq!(listing["swarm"]["complete"], 1);

    let (status, page) = get(&url, "/catalog/v1/listings?category=datasets");
    assert_eq!(status, 200);
    assert_eq!(page["total"], 1);
    assert_eq!(page["listings"][0]["bundle"], bundle.as_str());

    // Searching finds it despite a typo, and counts its category and tags.
    let (status, page) = get(&url, "/catalog/v1/listings?q=rainfal+csv&tag=weather");
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["total"], 1);
    assert_eq!(page["listings"][0]["bundle"], bundle.as_str());
    assert_eq!(
        page["facets"]["categories"],
        serde_json::json!([{ "value": "datasets", "count": 1 }])
    );
    let (_, page) = get(&url, "/catalog/v1/listings?q=snowfall");
    assert_eq!(page["total"], 0);
}

#[test]
fn listing_fields_need_publish() {
    let dir = tempfile::tempdir().unwrap();
    let output = tempest()
        .args(["share", "--dry-run", "--title", "Untitled"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("--publish"), "{stderr}");
}

/// A browser's sign-in code, signed by `tempest catalog sign-in` with the engine's key while no
/// daemon runs; the browser then collects the session.
#[test]
fn catalog_sign_in_signs_a_browsers_code() {
    let dir = tempfile::tempdir().unwrap();
    let (_tracker, url) = start_tracker(dir.path());
    let (status, challenge) = request(&url, "POST", "/catalog/v1/challenges", "");
    assert_eq!(status, 200, "{challenge}");
    let code = challenge["code"].as_str().unwrap();

    let output = tempest()
        .args(["catalog", "sign-in", &url, "--code", code, "--data-dir"])
        .arg(dir.path().join("data"))
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "{stderr}");
    let key = stderr
        .lines()
        .find_map(|line| line.strip_prefix("signed in as "))
        .unwrap_or_else(|| panic!("{stderr}"));

    let poll = serde_json::json!({ "code": code, "poll": challenge["poll"] }).to_string();
    let (status, collected) = request(&url, "POST", "/catalog/v1/challenges/poll", &poll);
    assert_eq!(status, 200, "{collected}");
    assert_eq!(collected["session"]["account"]["key"], key);
}

/// The catalog's page is at `/`, whether its UI was built into the binary or not, and every
/// file it names is served.
#[test]
fn the_tracker_serves_the_catalog_page() {
    let dir = tempfile::tempdir().unwrap();
    let (_tracker, url) = start_tracker(dir.path());
    let address = url.strip_prefix("http://").unwrap();
    let page = |path: &str| {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        (head.split(' ').nth(1).unwrap().to_owned(), body.to_owned())
    };
    let (status, html) = page("/");
    assert_eq!(status, "200");
    assert!(html.contains("<title>Tempest catalog</title>"), "{html}");
    for name in html
        .split('"')
        .filter(|part| part.ends_with(".js") || part.ends_with(".css") || part.ends_with(".wasm"))
    {
        let path = format!("/{}", name.trim_start_matches('/'));
        assert_eq!(page(&path).0, "200", "{path}");
    }
    // The API's paths stay the API's.
    let (status, body) = page("/catalog/v1/nowhere");
    assert_eq!(status, "404");
    assert!(body.contains("not_found"), "{body}");
}
