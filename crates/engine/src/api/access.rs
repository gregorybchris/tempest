//! Who may use the API (D-125).
//!
//! Locally, anyone on this machine may, but not web pages from elsewhere: a request must be
//! addressed to a loopback host, which defeats DNS rebinding, and a request from a browser must
//! come from a page on a loopback host. Remotely, every request must carry the access token.

use std::net::IpAddr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use super::error::ApiError;

/// Who may use the API.
#[derive(Debug, Clone)]
pub enum Access {
    /// Programs on this machine, and web pages served from it.
    Local,
    /// Whoever presents this token as `Authorization: Bearer <token>`.
    Token(Arc<str>),
}

/// Refuses requests that `access` doesn't allow.
pub(super) async fn guard(State(access): State<Access>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let refusal = match &access {
        Access::Local => local_refusal(headers),
        Access::Token(token) => token_refusal(headers, token),
    };
    match refusal {
        Some(error) => error.into_response(),
        None => next.run(request).await,
    }
}

fn local_refusal(headers: &HeaderMap) -> Option<ApiError> {
    let forbidden =
        |message: &str| Some(ApiError::new(StatusCode::FORBIDDEN, "forbidden", message));
    let host = headers.get(header::HOST).map(|value| value.to_str());
    if let Some(host) = host
        && !host.is_ok_and(is_loopback_host)
    {
        return forbidden(
            "without remote access, the API answers only requests to a loopback host",
        );
    }
    let origin = headers.get(header::ORIGIN).map(|value| value.to_str());
    if let Some(origin) = origin {
        let host = origin
            .ok()
            .and_then(|origin| origin.split_once("://"))
            .map(|(_, rest)| rest);
        if !host.is_some_and(is_loopback_host) {
            return forbidden("the API answers web pages only from this machine");
        }
    }
    None
}

/// Whether a `Host` header's value, or an origin's host and port, names this machine.
fn is_loopback_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        // An IPv6 address, as in `[::1]:4776`.
        Some(rest) => rest.split_once(']').map_or(rest, |(ip, _)| ip),
        None => host.rsplit_once(':').map_or(host, |(name, _)| name),
    };
    name.eq_ignore_ascii_case("localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

fn token_refusal(headers: &HeaderMap, token: &str) -> Option<ApiError> {
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    // Comparing digests takes the same time however much of the token matches.
    let matches = presented
        .is_some_and(|presented| Sha256::digest(presented.trim()) == Sha256::digest(token));
    match matches {
        true => None,
        false => Some(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "this request needs the engine's access token, as `Authorization: Bearer <token>`",
        )),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn loopback_hosts_are_recognized() {
        for host in [
            "localhost",
            "localhost:4776",
            "127.0.0.1:4776",
            "127.3.2.1",
            "[::1]:4776",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "example.org",
            "evil.localhost.example:80",
            "10.0.0.1:4776",
            "[fe80::1]",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    proptest! {
        #[test]
        fn any_header_is_judged_without_panicking(host in ".*", origin in ".*", auth in ".*") {
            let mut headers = HeaderMap::new();
            for (name, value) in [
                (header::HOST, &host),
                (header::ORIGIN, &origin),
                (header::AUTHORIZATION, &auth),
            ] {
                if let Ok(value) = value.parse() {
                    headers.insert(name, value);
                }
            }
            let _ = local_refusal(&headers);
            let _ = token_refusal(&headers, "x");
        }
    }
}
