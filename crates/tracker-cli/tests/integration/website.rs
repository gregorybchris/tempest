//! The tracker's website, served by the binary.

use std::io::{Read, Write};
use std::net::TcpStream;

use crate::common;

/// The tracker's website is at `/`, whether it was built into the binary or not, and every
/// file it names is served.
#[test]
fn the_tracker_serves_its_website() {
    let dir = tempfile::tempdir().unwrap();
    let (_tracker, url) = common::start(dir.path());
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
    assert!(html.contains("<title>Tempest tracker</title>"), "{html}");
    for name in html
        .split('"')
        .filter(|part| part.ends_with(".js") || part.ends_with(".css") || part.ends_with(".wasm"))
    {
        let path = format!("/{}", name.trim_start_matches('/'));
        assert_eq!(page(&path).0, "200", "{path}");
    }
    // The API's paths stay the API's.
    let (status, body) = page("/tempest/v1/nowhere");
    assert_eq!(status, "404");
    assert!(body.contains("not_found"), "{body}");
}
