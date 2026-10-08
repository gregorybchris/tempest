//! The catalog: bundles their sharers chose to list, and the accounts that list, vote and
//! comment on them (D-025, D-128, D-129).
//!
//! [`Catalog`] holds the rules. It saves its state in a [`Store`] and keeps sign-in challenges in
//! memory, since they last minutes. Callers pass the time, in seconds since the Unix epoch, so the
//! rules can be tested without a clock. [`routes`] serves it over HTTP.
//!
//! Operators run a catalog by choice; a tracker serves only the peer-list service without one.
//! What a catalog lists is its operator's responsibility (D-031).

pub(crate) mod routes;
pub mod store;

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, UnparsedPublicKey};
use tempest_catalog_api::account::{
    Account, AccountEdit, CODE_LEN, Challenge, Poll, PollResponse, Session, SignIn, code_from,
    normalize_code, sign_in_message,
};
use tempest_catalog_api::listing::{
    Comment, Content, Listing, ListingEdit, ListingPage, ListingQuery, ListingStatus, MAX_PAGE,
    NewComment, Vote,
};
use tempest_catalog_api::{CatalogInfo, Category, codes, default_categories, text};
use tempest_protocol::tracker::{ErrorCode, Signature};
use tempest_protocol::{BundleId, Hash, PeerId};

pub use store::{Store, StoreError};

use crate::swarms::Refusal;

/// How long a sign-in code can be signed and its session collected.
const CHALLENGE_LIFETIME: u64 = 10 * 60;

/// The most sign-in challenges outstanding at once, which bounds their memory.
const MAX_CHALLENGES: usize = 10_000;

/// How a catalog runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogConfig {
    /// The categories a listing can be in.
    pub categories: Vec<Category>,
    /// Whether a key's first sign-in needs an invite (D-128). Announces then list bundles only
    /// for keys that have signed in.
    pub invites_required: bool,
    /// How long a session lasts.
    pub session_lifetime: Duration,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            categories: default_categories(),
            invites_required: false,
            session_lifetime: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

/// A sign-in challenge waiting to be signed, or for its session to be collected.
#[derive(Debug)]
struct Pending {
    /// The SHA-256 of its poll secret.
    poll_hash: Vec<u8>,
    expires: u64,
    /// The session, once the code is signed, until it is collected.
    session: Option<Session>,
}

/// A tracker's catalog.
#[derive(Debug)]
pub struct Catalog {
    store: Store,
    config: CatalogConfig,
    challenges: HashMap<String, Pending>,
}

/// Whether `sig` is `key`'s Ed25519 signature over `message`.
pub fn verify(key: &PeerId, message: &[u8], sig: &Signature) -> bool {
    UnparsedPublicKey::new(&ED25519, key.as_bytes())
        .verify(message, &sig.0)
        .is_ok()
}

/// The time now, in seconds since the Unix epoch.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn sha256(text: &str) -> Vec<u8> {
    digest(&SHA256, text.as_bytes()).as_ref().to_vec()
}

fn random<const N: usize>() -> Result<[u8; N], Refusal> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|_| {
        Refusal::new(
            ErrorCode::Unavailable,
            "the tracker has no randomness for this",
        )
    })?;
    Ok(bytes)
}

/// A refusal with one of the catalog's codes.
fn refusal(status: u16, code: &str, message: impl Into<String>) -> Refusal {
    Refusal {
        status,
        ..Refusal::new(ErrorCode::Other(code.into()), message)
    }
}

fn not_found(what: &str) -> Refusal {
    refusal(404, codes::NOT_FOUND, format!("no such {what}"))
}

fn forbidden(message: &str) -> Refusal {
    refusal(403, codes::FORBIDDEN, message)
}

fn invalid(error: text::TextError) -> Refusal {
    Refusal::new(ErrorCode::InvalidRequest, error.0)
}

impl From<StoreError> for Refusal {
    fn from(error: StoreError) -> Self {
        let refusal = Refusal::new(ErrorCode::Unavailable, error.to_string());
        Refusal {
            status: 500,
            ..refusal
        }
    }
}

impl Catalog {
    /// The catalog saved at `path`, created if there is none.
    pub fn open(path: &Path, config: CatalogConfig) -> Result<Self, StoreError> {
        Ok(Self::new(Store::open(path)?, config))
    }

    /// A catalog kept in memory, for tests.
    pub fn in_memory(config: CatalogConfig) -> Result<Self, StoreError> {
        Ok(Self::new(Store::in_memory()?, config))
    }

    fn new(store: Store, config: CatalogConfig) -> Self {
        Self {
            store,
            config,
            challenges: HashMap::new(),
        }
    }

    /// What the catalog is like.
    pub fn info(&self) -> CatalogInfo {
        CatalogInfo {
            categories: self.config.categories.clone(),
            invites_required: self.config.invites_required,
        }
    }

    /// Makes an invite, for a key's first sign-in, and returns its code.
    pub fn create_invite(&mut self, now: u64) -> Result<String, Refusal> {
        let code = code_from(random::<CODE_LEN>()?);
        self.store.create_invite(&code, now)?;
        Ok(code)
    }

    // Signing in.

    /// A new sign-in challenge.
    pub fn challenge(&mut self, now: u64) -> Result<Challenge, Refusal> {
        self.challenges.retain(|_, pending| pending.expires > now);
        if self.challenges.len() >= MAX_CHALLENGES {
            let refusal = Refusal::new(ErrorCode::Unavailable, "too many sign-ins at once");
            return Err(refusal.retry_after(Duration::from_secs(60)));
        }
        let code = code_from(random::<CODE_LEN>()?);
        let poll = Hash::new(random::<32>()?).to_string();
        let pending = Pending {
            poll_hash: sha256(&poll),
            expires: now + CHALLENGE_LIFETIME,
            session: None,
        };
        // A repeated code, about one in 2^50, replaces an old challenge, which then fails.
        self.challenges.insert(code.clone(), pending);
        Ok(Challenge {
            code,
            poll,
            expires_in_secs: CHALLENGE_LIFETIME,
        })
    }

    /// Signs in with a signed challenge that reached the tracker at `authority` (see
    /// [`tempest_catalog_api::account::normalize_authority`]).
    pub fn sign_in(
        &mut self,
        now: u64,
        authority: &str,
        request: &SignIn,
    ) -> Result<Session, Refusal> {
        let failed = |message: &str| refusal(401, codes::SIGN_IN_FAILED, message);
        let code = normalize_code(&request.code);
        let Some(pending) = self.challenges.get(&code) else {
            return Err(failed("the code is unknown or expired"));
        };
        if pending.expires <= now || pending.session.is_some() {
            return Err(failed("the code is expired or already used"));
        }
        let message = sign_in_message(authority, &code);
        if !verify(&request.key, &message, &request.sig) {
            return Err(failed(
                "the signature doesn't verify for this key, code and tracker address",
            ));
        }
        let account = match self.store.account(&request.key)? {
            Some(_) => self.account(&request.key)?,
            None => self.join(&request.key, request.invite.as_deref(), now)?,
        };

        let token = Hash::new(random::<32>()?).to_string();
        let lifetime = self.config.session_lifetime.as_secs();
        self.store
            .create_session(&sha256(&token), &request.key, now + lifetime, now)?;
        let session = Session {
            token,
            expires_in_secs: lifetime,
            account,
        };
        if let Some(pending) = self.challenges.get_mut(&code) {
            pending.session = Some(session.clone());
        }
        Ok(session)
    }

    /// Makes `key`'s account, using `invite` if the catalog requires one.
    fn join(&mut self, key: &PeerId, invite: Option<&str>, now: u64) -> Result<Account, Refusal> {
        let joined = match (self.config.invites_required, invite) {
            (false, _) => {
                self.store.create_account(key, now)?;
                true
            }
            (true, Some(invite)) => {
                self.store
                    .join_with_invite(key, &normalize_code(invite), now)?
            }
            (true, None) => false,
        };
        if !joined {
            let message = "this catalog needs an invite to join; the one given is unknown or used";
            return Err(refusal(403, codes::INVITE_REQUIRED, message));
        }
        self.account(key)
    }

    /// Collects the session of a signed challenge, once.
    pub fn poll(&mut self, now: u64, request: &Poll) -> Result<PollResponse, Refusal> {
        let code = normalize_code(&request.code);
        let matches = self.challenges.get(&code).is_some_and(|pending| {
            pending.expires > now && pending.poll_hash == sha256(&request.poll)
        });
        if !matches {
            let message = "the code is unknown or expired, or the poll secret is wrong";
            return Err(refusal(401, codes::SIGN_IN_FAILED, message));
        }
        let session = match self.challenges.get(&code).and_then(|p| p.session.as_ref()) {
            Some(_) => self.challenges.remove(&code).and_then(|p| p.session),
            None => None,
        };
        Ok(PollResponse { session })
    }

    /// The key of the session whose token is `token`.
    pub fn authenticate(&self, now: u64, token: &str) -> Result<PeerId, Refusal> {
        match self.store.session(&sha256(token), now)? {
            Some(key) => Ok(key),
            None => Err(refusal(
                401,
                codes::UNAUTHORIZED,
                "the session is unknown or expired; sign in again",
            )),
        }
    }

    /// Ends the session whose token is `token`.
    pub fn sign_out(&mut self, token: &str) -> Result<(), Refusal> {
        Ok(self.store.delete_session(&sha256(token))?)
    }

    // Accounts.

    /// `key`'s account.
    pub fn account(&self, key: &PeerId) -> Result<Account, Refusal> {
        let (name, joined) = self
            .store
            .account(key)?
            .ok_or_else(|| not_found("account"))?;
        Ok(Account {
            key: *key,
            name,
            joined,
        })
    }

    /// Changes `key`'s account.
    pub fn edit_account(&mut self, key: &PeerId, edit: &AccountEdit) -> Result<Account, Refusal> {
        let name = edit
            .name
            .as_deref()
            .map(text::clean_name)
            .transpose()
            .map_err(invalid)?;
        self.store.set_name(key, name.as_deref())?;
        self.account(key)
    }

    // Listings.

    /// Handles an announce's verified `listing` field: `key` asks for `bundle` to be listed.
    ///
    /// The bundle's first publisher keeps it, and a withdrawn listing stays withdrawn. Where
    /// invites are required, only keys with accounts can list; elsewhere listing makes one.
    pub fn request_listing(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
    ) -> Result<(), StoreError> {
        if self.store.account(key)?.is_none() {
            if self.config.invites_required {
                return Ok(());
            }
            self.store.create_account(key, now)?;
        }
        if self.store.listing_owner(bundle)?.is_none() {
            self.store.create_listing(bundle, key, now)?;
        }
        Ok(())
    }

    /// Lists `bundle` for `key`, or changes `key`'s listing of it. A withdrawn listing is listed
    /// again.
    pub fn edit_listing(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
        edit: &ListingEdit,
    ) -> Result<Listing, Refusal> {
        let title = edit.title.as_deref().map(text::clean_title);
        let description = edit.description.as_deref().map(text::clean_description);
        let tags = edit.tags.as_deref().map(text::normalize_tags);
        let (title, description, tags) = (
            title.transpose().map_err(invalid)?,
            description.transpose().map_err(invalid)?,
            tags.transpose().map_err(invalid)?,
        );
        if let Some(Some(category)) = &edit.category
            && !self.config.categories.iter().any(|c| &c.slug == category)
        {
            let message = format!("this catalog has no category {category:?}");
            return Err(Refusal::new(ErrorCode::InvalidRequest, message));
        }

        match self.store.listing_owner(bundle)? {
            None => self.store.create_listing(bundle, key, now)?,
            Some((owner, _)) if owner != *key => {
                return Err(forbidden("another key listed this bundle"));
            }
            Some((_, ListingStatus::Withdrawn)) => self.store.relist(bundle, now)?,
            Some(_) => {}
        }
        let mut fields = self.store.listing_fields(bundle)?;
        fields.title = title.unwrap_or(fields.title);
        fields.description = description.unwrap_or(fields.description);
        fields.category = edit.category.clone().unwrap_or(fields.category);
        fields.tags = tags.unwrap_or(fields.tags);
        self.store.set_listing_fields(bundle, &fields, now)?;
        self.listing(bundle, Some(key))
    }

    /// Takes `key`'s listing of `bundle` down.
    pub fn withdraw(&mut self, now: u64, bundle: &BundleId, key: &PeerId) -> Result<(), Refusal> {
        match self.store.listing_owner(bundle)? {
            None => Err(not_found("listing")),
            Some((owner, _)) if owner != *key => {
                Err(forbidden("only the listing's publisher can withdraw it"))
            }
            Some(_) => Ok(self
                .store
                .set_status(bundle, ListingStatus::Withdrawn, now)?),
        }
    }

    /// Records what `bundle` holds, from its manifest, which lists it if it was pending.
    pub fn set_content(&mut self, bundle: &BundleId, content: &Content) -> Result<(), StoreError> {
        self.store.set_content(bundle, content)
    }

    /// The bundles whose listings wait for their manifests, with their publishers, oldest
    /// first.
    pub fn pending(&self) -> Result<Vec<(BundleId, PeerId)>, StoreError> {
        self.store.pending()
    }

    /// `bundle`'s listing as `viewer` sees it: only its publisher sees it unless it is listed.
    /// Its swarm counts are zero; the swarms aren't the catalog's.
    pub fn listing(&self, bundle: &BundleId, viewer: Option<&PeerId>) -> Result<Listing, Refusal> {
        match self.store.listing(bundle, viewer)? {
            Some(listing)
                if listing.status == ListingStatus::Listed
                    || Some(&listing.publisher.key) == viewer =>
            {
                Ok(listing)
            }
            _ => Err(not_found("listing")),
        }
    }

    /// The listing of `bundle` if it is listed, for votes and comments.
    fn listed(&self, bundle: &BundleId) -> Result<(), Refusal> {
        match self.store.listing_owner(bundle)? {
            Some((_, ListingStatus::Listed)) => Ok(()),
            _ => Err(not_found("listing")),
        }
    }

    /// A page of listed bundles, newest first.
    pub fn listings(
        &self,
        query: &ListingQuery,
        viewer: Option<&PeerId>,
    ) -> Result<ListingPage, Refusal> {
        let limit = query.limit.min(MAX_PAGE);
        let (listings, total) =
            self.store
                .listed(viewer, query.category.as_deref(), query.offset, limit)?;
        Ok(ListingPage { listings, total })
    }

    /// A page of the listings `key` asked for, whatever their status.
    pub fn published_by(
        &self,
        key: &PeerId,
        offset: u64,
        limit: u32,
    ) -> Result<ListingPage, Refusal> {
        let (listings, total) = self.store.published_by(key, offset, limit.min(MAX_PAGE))?;
        Ok(ListingPage { listings, total })
    }

    // Votes and comments.

    /// Records `key`'s vote on `bundle`'s listing, and returns the listing.
    pub fn vote(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
        vote: Vote,
    ) -> Result<Listing, Refusal> {
        self.listed(bundle)?;
        self.store.vote(bundle, key, vote, now)?;
        self.listing(bundle, Some(key))
    }

    /// The comments on `bundle`'s listing, oldest first, if `viewer` can see it.
    pub fn comments(
        &self,
        bundle: &BundleId,
        viewer: Option<&PeerId>,
    ) -> Result<Vec<Comment>, Refusal> {
        self.listing(bundle, viewer)?;
        Ok(self.store.comments(bundle)?)
    }

    /// Adds `key`'s comment to `bundle`'s listing.
    pub fn comment(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
        comment: &NewComment,
    ) -> Result<Comment, Refusal> {
        let body = text::clean_comment(&comment.body).map_err(invalid)?;
        self.listed(bundle)?;
        Ok(self.store.add_comment(bundle, key, &body, now)?)
    }

    /// Deletes `key`'s comment `id`.
    pub fn delete_comment(&mut self, id: u64, key: &PeerId) -> Result<(), Refusal> {
        match self.store.comment(id)? {
            None => Err(not_found("comment")),
            Some(comment) if comment.author.key != *key => {
                Err(forbidden("only a comment's author can delete it"))
            }
            Some(_) => Ok(self.store.delete_comment(id)?),
        }
    }
}

#[cfg(test)]
mod tests;
