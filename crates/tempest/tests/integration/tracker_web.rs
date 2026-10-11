//! The tracker's website in headless Chrome, against `tempest-tracker --catalog` with bundles
//! published by `tempest share --publish`: searching with a typo, narrowing by category and
//! tag, and signing in by signing the page's code with `tempest sign-in`.
//!
//! It needs the UI built into the binary (`trunk build` in `crates/tracker-web`) and
//! ChromeDriver, named by `CHROMEDRIVER`, so it is ignored by default. Run it with
//! `cargo test -p tempest --test integration -- tracker_web:: --ignored`; it fails without
//! either. CI's web job runs it so.

#![cfg(unix)]

use crate::common::{self, COMMAND_LIMIT, OutputWithin, Running};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

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

/// A request over HTTP/1.1: the status and the body. The body is read by its length, since
/// ChromeDriver keeps connections open.
fn http(address: &str, method: &str, path: &str, body: Option<&Value>) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(LIMIT)).unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line).unwrap();
    let status = status_line.split(' ').nth(1).unwrap().parse().unwrap();
    let mut length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (name, value) = line.split_once(':').unwrap();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse::<usize>().ok(),
            "transfer-encoding" => chunked = value.trim().eq_ignore_ascii_case("chunked"),
            _ => {}
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).unwrap();
            let size = usize::from_str_radix(size.trim(), 16).unwrap();
            let mut chunk = vec![0; size + 2];
            reader.read_exact(&mut chunk).unwrap();
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else if let Some(length) = length {
        body.resize(length, 0);
        reader.read_exact(&mut body).unwrap();
    } else {
        reader.read_to_end(&mut body).unwrap();
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// Starts `tempest-tracker` with listings in `dir`, and returns it with its address.
fn start_tracker(dir: &Path) -> (Running, String) {
    let catalog = dir.join("catalog.db");
    let (tracker, url) = common::start_tracker(&[
        "--catalog".as_ref(),
        catalog.as_os_str(),
        "--allow-local-peers".as_ref(),
    ]);
    let address = url.strip_prefix("http://").expect("an http URL").to_owned();
    (tracker, address)
}

/// Shares a folder named `name` with `identity`, publishing it to the tracker with `listing`.
fn publish(dir: &Path, tracker: &str, identity: &Path, name: &str, listing: &[&str]) -> Running {
    let folder = dir.join(name);
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("README.txt"), format!("{name}\n")).unwrap();
    let child = tempest()
        .args(["share", "--bind", "127.0.0.1", "--port", "0", "--no-lan"])
        .arg("--no-port-mapping")
        .arg("--identity")
        .arg(identity)
        .args(["--publish", &format!("http://{tracker}")])
        .args(listing)
        .arg(&folder)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Running(child)
}

/// Waits for `ready`, polling.
fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Waits for `ready`, polling, and says what was seen if it never is.
fn wait_until(what: &str, mut seen: impl FnMut() -> Result<(), String>) {
    let deadline = Instant::now() + LIMIT;
    while let Err(last) = seen() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {last}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A ChromeDriver session in headless Chrome.
struct Browser {
    driver: String,
    session: String,
    _chromedriver: Running,
}

impl Browser {
    fn start(chromedriver: &str) -> Browser {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new(chromedriver)
            .arg(format!("--port={port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let chromedriver = Running(child);
        let driver = format!("127.0.0.1:{port}");
        wait_for("ChromeDriver", || TcpStream::connect(&driver).is_ok());
        let args = ["--headless=new", "--window-size=1280,900", "--no-sandbox"];
        let capabilities = json!({
            "capabilities": { "alwaysMatch": { "goog:chromeOptions": { "args": args } } }
        });
        let (status, body) = http(&driver, "POST", "/session", Some(&capabilities));
        assert_eq!(status, 200, "{body}");
        let answer: Value = serde_json::from_str(&body).unwrap();
        let session = answer["value"]["sessionId"].as_str().unwrap().to_owned();
        Browser {
            driver,
            session,
            _chromedriver: chromedriver,
        }
    }

    fn call(&self, method: &str, path: &str, body: Value) -> Value {
        let path = format!("/session/{}{path}", self.session);
        let body = (method != "GET").then_some(&body);
        let (status, text) = http(&self.driver, method, &path, body);
        assert_eq!(status, 200, "{method} {path}: {text}");
        serde_json::from_str::<Value>(&text).unwrap()["value"].take()
    }

    fn go(&self, url: &str) {
        self.call("POST", "/url", json!({ "url": url }));
    }

    /// Runs `script` in the page, with `arguments` empty, and returns its value.
    fn run(&self, script: &str) -> Value {
        self.call(
            "POST",
            "/execute/sync",
            json!({ "script": script, "args": [] }),
        )
    }

    /// The element matching the CSS `selector`.
    fn find(&self, selector: &str) -> String {
        let found = self.call(
            "POST",
            "/element",
            json!({ "using": "css selector", "value": selector }),
        );
        found
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The first element matching `xpath`.
    fn find_xpath(&self, xpath: &str) -> String {
        let found = self.call(
            "POST",
            "/element",
            json!({ "using": "xpath", "value": xpath }),
        );
        found
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn click(&self, element: &str) {
        self.call("POST", &format!("/element/{element}/click"), json!({}));
    }

    fn type_into(&self, element: &str, text: &str) {
        self.call(
            "POST",
            &format!("/element/{element}/value"),
            json!({ "text": text }),
        );
    }

    /// Presses each key in turn, wherever the focus is.
    fn press(&self, keys: &[&str]) {
        let actions: Vec<Value> = keys
            .iter()
            .flat_map(|key| {
                [
                    json!({ "type": "keyDown", "value": key }),
                    json!({ "type": "keyUp", "value": key }),
                ]
            })
            .collect();
        let body = json!({ "actions": [{ "type": "key", "id": "keyboard", "actions": actions }] });
        self.call("POST", "/actions", body);
    }

    /// The titles of the results shown.
    fn titles(&self) -> Vec<String> {
        let titles = self
            .run("return [...document.querySelectorAll('.result__title')].map(e => e.textContent)");
        serde_json::from_value(titles).unwrap()
    }

    fn hash(&self) -> String {
        self.run("return location.hash")
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Waits until the results' titles are `expected`, in any order.
    fn wait_for_titles(&self, expected: &[&str]) {
        let mut expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
        expected.sort();
        let deadline = Instant::now() + LIMIT;
        loop {
            let mut seen = self.titles();
            seen.sort();
            if seen == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "results {seen:?}, not {expected:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let path = format!("/session/{}", self.session);
        let _ = http(&self.driver, "DELETE", &path, None);
    }
}

#[test]
#[ignore = "needs ChromeDriver in CHROMEDRIVER and the tracker website built in; run with --ignored"]
fn search_filter_and_sign_in_in_a_browser() {
    let chromedriver = std::env::var("CHROMEDRIVER")
        .expect("set CHROMEDRIVER to ChromeDriver's path to run the browser test");
    let dir = tempfile::tempdir().unwrap();
    let (_tracker, tracker) = start_tracker(dir.path());
    let (_, page) = http(&tracker, "GET", "/", None);
    assert!(
        page.contains("tempest-tracker-web"),
        "this binary was built without the tracker website: run trunk build in crates/tracker-web"
    );

    // One publisher lists two bundles, another a third. The first share creates the shared
    // key file, so the second waits for it.
    let data = dir.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let mine = data.join("identity.pem");
    let listed = |count: usize| {
        let tracker = tracker.clone();
        move || {
            let (_, page) = http(&tracker, "GET", "/tempest/v1/listings", None);
            match page.contains(&format!(r#""total":{count}"#)) {
                true => Ok(()),
                false => Err(page),
            }
        }
    };
    let _rain = publish(
        dir.path(),
        &tracker,
        &mine,
        "rain",
        &[
            "--title",
            "Rainfall 2025",
            "--category",
            "datasets",
            "--tag",
            "weather",
            "--tag",
            "csv",
        ],
    );
    wait_until("the first listing", listed(1));
    let _snow = publish(
        dir.path(),
        &tracker,
        &mine,
        "snow",
        &[
            "--title",
            "Snowfall archive",
            "--category",
            "datasets",
            "--tag",
            "weather",
        ],
    );
    let _model = publish(
        dir.path(),
        &tracker,
        &dir.path().join("other.pem"),
        "model",
        &[
            "--title",
            "Tiny model weights",
            "--category",
            "models",
            "--tag",
            "llm",
        ],
    );
    wait_until("three listings", listed(3));

    let browser = Browser::start(&chromedriver);
    let home = format!("http://{tracker}/");
    browser.go(&home);
    browser.wait_for_titles(&["Rainfall 2025", "Snowfall archive", "Tiny model weights"]);

    // Searching narrows the results as it is typed, despite the typo, and the address follows.
    let search = browser.find(".bigsearch__input");
    browser.type_into(&search, "rainfal");
    browser.wait_for_titles(&["Rainfall 2025"]);
    assert_eq!(browser.hash(), "#/?q=rainfal");
    // Backspace, as a person would; WebDriver's clear sends no input events.
    browser.type_into(&search, &"\u{E003}".repeat("rainfal".len()));
    browser.wait_for_titles(&["Rainfall 2025", "Snowfall archive", "Tiny model weights"]);

    // A category facet narrows to it; a tag facet narrows further; removing the filters
    // widens again.
    browser.click(&browser.find_xpath("//a[contains(@class,'facet')][contains(.,'Datasets')]"));
    browser.wait_for_titles(&["Rainfall 2025", "Snowfall archive"]);
    assert_eq!(browser.hash(), "#/?category=datasets");
    browser.click(&browser.find_xpath("//a[contains(@class,'tagfacet')][contains(.,'csv')]"));
    browser.wait_for_titles(&["Rainfall 2025"]);
    assert_eq!(browser.hash(), "#/?category=datasets&tag=csv");
    browser.click(&browser.find_xpath("//button[contains(.,'Clear filters')]"));
    browser.wait_for_titles(&["Rainfall 2025", "Snowfall archive", "Tiny model weights"]);

    // The keyboard: `j` moves to the first result and `enter` opens it.
    browser.run("document.activeElement.blur()");
    browser.press(&["j", "\u{E007}"]);
    wait_for("a listing's page", || {
        browser.hash().starts_with("#/listings/")
    });
    let listing_hash = browser.hash();

    // Signing in: the page shows a code and the command, the command signs it, and the page
    // signs in by itself and returns to where it was.
    browser.click(&browser.find(".account-button"));
    let mut code = String::new();
    wait_for("a sign-in code", || {
        code = browser
            .run("const c = document.querySelector('.signin__code'); return c ? c.textContent : ''")
            .as_str()
            .unwrap()
            .to_owned();
        !code.is_empty()
    });
    let command = browser
        .run("return document.querySelector('.copyable__text').textContent")
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        command,
        format!(
            "tempest sign-in {} --code {code}",
            home.trim_end_matches('/')
        )
    );
    let signed = tempest()
        .args([
            "sign-in",
            home.trim_end_matches('/'),
            "--code",
            &code,
            "--data-dir",
        ])
        .arg(&data)
        .output_within(COMMAND_LIMIT)
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    wait_for("the page to sign in", || {
        browser.run("return !!document.querySelector('.account-link')") == json!(true)
    });
    wait_for("the page to return", || browser.hash() == listing_hash);

    // Signed in, the account's listings are its own two.
    browser.go(&format!("{home}#/me"));
    wait_for("my listings", || {
        let rows = browser.run(
            "return [...document.querySelectorAll('.mine tbody a:not(.btn)')].map(e => e.textContent).sort()",
        );
        rows == json!(["Rainfall 2025", "Snowfall archive"])
    });
}
