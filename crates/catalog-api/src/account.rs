//! Signing in with a Tempest key (D-128), and accounts.
//!
//! There are no passwords. Signing in proves ownership of a peer key:
//!
//! 1. Someone asks for a [`Challenge`] (`POST /catalog/v1/challenges`). Its `code` is short
//!    enough to type, and its `poll` secret stays with whoever asked.
//! 2. The key's owner signs [`sign_in_message`] for the code and sends a [`SignIn`]
//!    (`POST /catalog/v1/sessions`). The answer is a [`Session`].
//! 3. Whoever holds `poll` can also collect that session, once, with a [`Poll`]
//!    (`POST /catalog/v1/challenges/poll`).
//!
//! A command-line client does all three steps itself. A browser does steps 1 and 3 and shows
//! the code, which the user's client signs in step 2, so the key never leaves the client.
//!
//! The signed message names the tracker as the signer reached it, so a tracker can't pass on a
//! code it got from another tracker and use the signature to sign in there.

use serde::{Deserialize, Serialize};
use tempest_protocol::PeerId;
use tempest_protocol::tracker::Signature;

/// The prefix of [`sign_in_message`], which keeps its signatures from meaning anything else.
pub const SIGN_IN_CONTEXT: &[u8] = b"tempest catalog sign-in v1\0";

/// What a key signs to sign in with challenge `code` at the tracker whose URL has `authority`
/// (its host, and port if the URL has one): [`SIGN_IN_CONTEXT`], `authority`, a zero byte and
/// `code`.
///
/// ```
/// use tempest_catalog_api::account::sign_in_message;
/// assert_eq!(
///     sign_in_message("tracker.example.org", "K7QM2-X4PDV"),
///     b"tempest catalog sign-in v1\0tracker.example.org\0K7QM2-X4PDV"
/// );
/// ```
pub fn sign_in_message(authority: &str, code: &str) -> Vec<u8> {
    [
        SIGN_IN_CONTEXT,
        authority.as_bytes(),
        b"\0",
        code.as_bytes(),
    ]
    .concat()
}

/// The authority of `url` as [`sign_in_message`] takes it: see [`normalize_authority`].
///
/// ```
/// use tempest_catalog_api::account::authority;
/// assert_eq!(authority("https://Tracker.example.org:443/x"), Some("tracker.example.org".into()));
/// assert_eq!(authority("http://127.0.0.1:4777"), Some("127.0.0.1:4777".into()));
/// assert_eq!(authority("tracker.example.org"), None);
/// ```
pub fn authority(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // Credentials in the URL aren't part of where it leads.
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!authority.is_empty()).then(|| normalize_authority(authority))
}

/// An authority (a URL's, or a `Host` header) in the form both sides of a sign-in use: lowercase,
/// without the default ports 80 and 443, which clients leave out of `Host`.
pub fn normalize_authority(authority: &str) -> String {
    let authority = authority.trim().to_ascii_lowercase();
    match authority
        .strip_suffix(":443")
        .or(authority.strip_suffix(":80"))
    {
        Some(host) => host.to_owned(),
        None => authority,
    }
}

/// A sign-in challenge: the answer to `POST /catalog/v1/challenges`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    /// The code to sign, such as `K7QM2-X4PDV`.
    pub code: String,
    /// The secret that collects the session once the code is signed.
    pub poll: String,
    /// Seconds until the code expires.
    pub expires_in_secs: u64,
}

/// The body of `POST /catalog/v1/sessions`: a signed challenge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignIn {
    /// The challenge's code.
    pub code: String,
    /// The key signing in.
    pub key: PeerId,
    /// The key's signature over [`sign_in_message`].
    pub sig: Signature,
    /// An invite, for a key's first sign-in to a catalog that requires one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
}

/// The body of `POST /catalog/v1/challenges/poll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Poll {
    /// The challenge's code.
    pub code: String,
    /// The challenge's poll secret.
    pub poll: String,
}

/// The answer to a [`Poll`]: the session once the code is signed, and `null` until then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollResponse {
    /// The session, the first time it is collected.
    pub session: Option<Session>,
}

/// A signed-in session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The bearer token that authenticates requests.
    pub token: String,
    /// Seconds until the token expires.
    pub expires_in_secs: u64,
    /// The account signed in.
    pub account: Account,
}

/// An account: a key that has signed in, or published, at least once. `GET /catalog/v1/me`
/// answers with the session's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The account's key.
    pub key: PeerId,
    /// The name it shows, if it chose one.
    pub name: Option<String>,
    /// When it joined, in seconds since the Unix epoch.
    pub joined: u64,
}

/// The body of `PUT /catalog/v1/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountEdit {
    /// The name to show, or `null` to show none. See [`crate::text::clean_name`].
    pub name: Option<String>,
}

/// The characters of challenge and invite codes: Crockford's base 32, which has no `I`, `L`, `O`
/// or `U`.
pub const CODE_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The number of characters in a code, besides its dash.
pub const CODE_LEN: usize = 10;

/// A code as someone typed it, in its one form: uppercase, with `O` read as `0` and `I` and `L`
/// as `1`, and a dash after the fifth character. Spaces and dashes typed are ignored.
///
/// ```
/// use tempest_catalog_api::account::normalize_code;
/// assert_eq!(normalize_code(" k7qm2 x4pdo "), "K7QM2-X4PD0");
/// ```
pub fn normalize_code(code: &str) -> String {
    let mut normal: String = code
        .chars()
        .filter(|c| !matches!(c, ' ' | '-'))
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    if normal.len() == CODE_LEN && normal.is_ascii() {
        normal.insert(CODE_LEN / 2, '-');
    }
    normal
}

/// The code made of `random`'s bytes, in its one form.
pub fn code_from(random: [u8; CODE_LEN]) -> String {
    let chars = random.map(|byte| CODE_ALPHABET[usize::from(byte % 32)]);
    normalize_code(std::str::from_utf8(&chars).expect("the alphabet is ASCII"))
}
