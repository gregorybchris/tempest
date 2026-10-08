//! Who may use the API (D-125).
//!
//! Locally, anyone on this machine may, but not web pages from elsewhere: a request must be
//! addressed to a loopback host, which defeats DNS rebinding, and a request from a browser must
//! come from a page on a loopback host. Remotely, every request must carry the access token,
//! as a header or as the session cookie that signing in with the token sets (D-222).
//!
//! A session is stateless: its cookie holds an expiry and a MAC of it keyed by the token, so
//! sessions survive a restart, end when they expire, and all end when the token changes.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use super::error::ApiError;

/// Who may use the API.
#[derive(Debug, Clone)]
pub enum Access {
    /// Programs on this machine, and web pages served from it.
    Local,
    /// Whoever presents this token as `Authorization: Bearer <token>`, or holds a session
    /// that signing in with it started.
    Token(Arc<str>),
}

/// The session cookie's name. The `__Host-` prefix makes browsers accept it only over HTTPS,
/// for this host alone.
pub const SESSION_COOKIE: &str = "__Host-tempest-session";

/// How long a session lasts.
pub const SESSION_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;

/// Seconds since the Unix epoch.
pub(super) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// HMAC-SHA-256 (RFC 2104).
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let inner = Sha256::new()
        .chain_update(padded.map(|byte| byte ^ 0x36))
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(padded.map(|byte| byte ^ 0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The session value that `token` allows until `expires`: the expiry and its MAC.
fn session_value(token: &str, expires: u64) -> String {
    let message = format!("tempest-session\0{expires}");
    format!(
        "{expires}.{}",
        hex(&hmac(token.as_bytes(), message.as_bytes()))
    )
}

/// Whether `value` is a session value that `token` made and that hasn't expired by `now`.
fn session_is_valid(token: &str, value: &str, now: u64) -> bool {
    let Some((expires, _)) = value.split_once('.') else {
        return false;
    };
    let Ok(expires) = expires.parse::<u64>() else {
        return false;
    };
    // Comparing digests takes the same time however much of the MAC matches.
    expires > now && Sha256::digest(value) == Sha256::digest(session_value(token, expires))
}

/// The `Set-Cookie` value that starts a session for `token` at `now`.
pub(super) fn session_cookie(token: &str, now: u64) -> HeaderValue {
    let value = session_value(token, now + SESSION_LIFETIME_SECS);
    let cookie = format!(
        "{SESSION_COOKIE}={value}; Max-Age={SESSION_LIFETIME_SECS}; Path=/; HttpOnly; Secure; \
         SameSite=Strict"
    );
    HeaderValue::from_str(&cookie).expect("session values are ASCII")
}

/// The `Set-Cookie` value that clears the session.
pub(super) fn clearing_cookie() -> HeaderValue {
    let cookie = format!("{SESSION_COOKIE}=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Strict");
    HeaderValue::from_str(&cookie).expect("ASCII")
}

/// Whether the request carries `token` as a bearer token or a live session cookie.
pub(super) fn is_authorized(headers: &HeaderMap, token: &str, now: u64) -> bool {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if let Some(presented) = bearer {
        return token_matches(presented, token);
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .any(|(name, value)| name == SESSION_COOKIE && session_is_valid(token, value, now))
}

/// Whether `presented` is `token`, in time that doesn't reveal how much of it matches.
pub(super) fn token_matches(presented: &str, token: &str) -> bool {
    Sha256::digest(presented.trim()) == Sha256::digest(token)
}

/// Refuses requests to the session routes that a web page from elsewhere makes. Anyone may try
/// to sign in remotely, since that is how a browser gets access.
pub(super) async fn session_guard(
    State(access): State<Access>,
    request: Request,
    next: Next,
) -> Response {
    match &access {
        Access::Local => match local_refusal(request.headers()) {
            Some(error) => error.into_response(),
            None => next.run(request).await,
        },
        Access::Token(_) => next.run(request).await,
    }
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
    let forbidden = |message: &str| {
        Some(ApiError::new(
            StatusCode::FORBIDDEN.as_u16(),
            "forbidden",
            message,
        ))
    };
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
    match is_authorized(headers, token, unix_now()) {
        true => None,
        false => Some(ApiError::new(
            StatusCode::UNAUTHORIZED.as_u16(),
            "unauthorized",
            "this request needs the engine's access token, as `Authorization: Bearer <token>`, \
             or a session from signing in with it",
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

    #[test]
    fn hmac_matches_the_rfc_4231_vectors() {
        let mac = hmac(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        let mac = hmac(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // A key longer than the block is hashed first.
        let mac = hmac(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn sessions_expire_and_end_with_the_token() {
        let value = session_value("token", 1_000);
        assert!(session_is_valid("token", &value, 999));
        assert!(!session_is_valid("token", &value, 1_000));
        assert!(!session_is_valid("other", &value, 999));
        // The expiry can't be extended without the token.
        let extended = value.replacen("1000", "9999", 1);
        assert!(!session_is_valid("token", &extended, 999));
        for junk in ["", ".", "x.y", "5.", "-1.00"] {
            assert!(!session_is_valid("token", junk, 0), "{junk}");
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
            if let Ok(value) = auth.parse() {
                headers.insert(header::COOKIE, value);
                let _ = token_refusal(&headers, "x");
            }
        }
    }
}
