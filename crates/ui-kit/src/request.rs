//! Requests from a page to the server that served it, with the browser's `fetch`.
//!
//! Each app names its server with a [`Server`]: where its API is, what its error answers look
//! like and how to speak of it. Every write sends a JSON body, `{}` when it has nothing to say,
//! so a server can refuse bodies that aren't JSON, as other sites' forms send.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The server an app talks to.
pub trait Server: 'static {
    /// The body of its error answers.
    type Error: DeserializeOwned + Clone + fmt::Debug + PartialEq;
    /// What every request's path is relative to, such as `/tempest/v1/`.
    const PREFIX: &'static str;
    /// What the UI calls it at the start of a sentence, such as "The engine".
    const NAME: &'static str;
    /// What to tell the user when it doesn't answer.
    const UNREACHABLE: &'static str;
    /// An error's short, stable code, such as `not_found`.
    fn code(error: &Self::Error) -> &str;
    /// An error's explanation, as a clause.
    fn message(error: &Self::Error) -> &str;
}

/// Why a request to `S` failed.
pub enum RequestError<S: Server> {
    /// The server didn't answer.
    Unreachable,
    /// The server refused the request, with its HTTP status and why.
    Refused(u16, S::Error),
    /// The answer wasn't what the UI expected.
    Unexpected(String),
}

impl<S: Server> RequestError<S> {
    /// The HTTP status of a refusal.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Refused(status, _) => Some(*status),
            _ => None,
        }
    }

    /// The code of a refusal, such as `not_found`.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused(_, error) => Some(S::code(error)),
            _ => None,
        }
    }
}

impl<S: Server> fmt::Display for RequestError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str(S::UNREACHABLE),
            Self::Refused(_, error) => f.write_str(&sentence(S::message(error))),
            Self::Unexpected(why) => {
                write!(f, "{} returned an unexpected response: {why}", S::NAME)
            }
        }
    }
}

impl<S: Server> fmt::Debug for RequestError<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable => f.write_str("Unreachable"),
            Self::Refused(status, error) => {
                f.debug_tuple("Refused").field(status).field(error).finish()
            }
            Self::Unexpected(why) => f.debug_tuple("Unexpected").field(why).finish(),
        }
    }
}

impl<S: Server> Clone for RequestError<S> {
    fn clone(&self) -> Self {
        match self {
            Self::Unreachable => Self::Unreachable,
            Self::Refused(status, error) => Self::Refused(*status, error.clone()),
            Self::Unexpected(why) => Self::Unexpected(why.clone()),
        }
    }
}

impl<S: Server> PartialEq for RequestError<S> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unreachable, Self::Unreachable) => true,
            (Self::Refused(a, x), Self::Refused(b, y)) => a == b && x == y,
            (Self::Unexpected(x), Self::Unexpected(y)) => x == y,
            _ => false,
        }
    }
}

/// `text` with a capital and a full stop, as servers' messages are clauses.
pub fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The answer to a request, from its HTTP status and body: the value, or the server's error.
pub fn decode<S: Server, T: DeserializeOwned>(
    status: u16,
    body: &str,
) -> Result<T, RequestError<S>> {
    if (200..300).contains(&status) {
        // An empty body (204) reads as `null`, which `()` and `Option` accept.
        let body = if body.trim().is_empty() { "null" } else { body };
        return serde_json::from_str(body).map_err(|e| RequestError::Unexpected(e.to_string()));
    }
    match serde_json::from_str::<S::Error>(body) {
        Ok(error) => Err(RequestError::Refused(status, error)),
        Err(_) => Err(RequestError::Unexpected(format!("status {status}"))),
    }
}

/// HTTP methods, by name.
pub mod method {
    /// Reads.
    pub const GET: &str = "GET";
    /// Acts.
    pub const POST: &str = "POST";
    /// Replaces.
    pub const PUT: &str = "PUT";
    /// Changes some fields.
    pub const PATCH: &str = "PATCH";
    /// Removes.
    pub const DELETE: &str = "DELETE";
}

/// Sends `method` to `path` on `S`, relative to its prefix, with `body` as JSON, and reads
/// the answer.
pub async fn send<S: Server, T: DeserializeOwned>(
    method: &str,
    path: &str,
    body: Option<&impl Serialize>,
) -> Result<T, RequestError<S>> {
    let body = body
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| RequestError::Unexpected(e.to_string()))?;
    let url = format!("{}{path}", S::PREFIX);
    let (status, text) = fetch(method, &url, body)
        .await
        .map_err(|_| RequestError::Unreachable)?;
    decode(status, &text)
}

/// `GET` of `path` on `S`.
pub async fn get<S: Server, T: DeserializeOwned>(path: &str) -> Result<T, RequestError<S>> {
    send(method::GET, path, None::<&()>).await
}

async fn fetch(method: &str, url: &str, body: Option<String>) -> Result<(u16, String), JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if method != method::GET {
        let headers = web_sys::Headers::new()?;
        headers.set("content-type", "application/json")?;
        init.set_headers(&headers);
        init.set_body(&JsValue::from_str(body.as_deref().unwrap_or("{}")));
    }
    let window = web_sys::window().ok_or("no window")?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await?
        .dyn_into()?;
    let text = JsFuture::from(response.text()?).await?;
    Ok((response.status(), text.as_string().unwrap_or_default()))
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Clone, Debug, PartialEq, Deserialize)]
    struct Refusal {
        error: String,
        message: String,
    }

    struct Test;

    impl Server for Test {
        type Error = Refusal;
        const PREFIX: &'static str = "/api/";
        const NAME: &'static str = "The server";
        const UNREACHABLE: &'static str = "Couldn't reach the server.";

        fn code(error: &Refusal) -> &str {
            &error.error
        }

        fn message(error: &Refusal) -> &str {
            &error.message
        }
    }

    type Answer<T> = Result<T, RequestError<Test>>;

    #[test]
    fn answers_decode_into_values_or_the_servers_errors() {
        assert_eq!(decode::<Test, Vec<u8>>(200, "[1,2]"), Ok(vec![1, 2]));
        assert_eq!(decode::<Test, ()>(204, ""), Ok(()));
        let refused: Answer<()> = decode(
            409,
            r#"{"error":"already_added","message":"the bundle is already added"}"#,
        );
        let error = refused.unwrap_err();
        assert_eq!(
            (error.status(), error.code()),
            (Some(409), Some("already_added"))
        );
        assert_eq!(error.to_string(), "The bundle is already added.");
        let odd: Answer<()> = decode(502, "<html>");
        assert!(matches!(odd, Err(RequestError::Unexpected(_))));
        assert_eq!(
            odd.unwrap_err().to_string(),
            "The server returned an unexpected response: status 502"
        );
        let wrong: Answer<u8> = decode(200, "\"x\"");
        assert!(matches!(wrong, Err(RequestError::Unexpected(_))));
        assert_eq!(
            RequestError::<Test>::Unreachable.to_string(),
            "Couldn't reach the server."
        );
    }

    #[test]
    fn clauses_become_sentences() {
        assert_eq!(sentence(" sign in first "), "Sign in first.");
        assert_eq!(sentence("Done!"), "Done!");
        assert_eq!(sentence(""), "");
    }
}
