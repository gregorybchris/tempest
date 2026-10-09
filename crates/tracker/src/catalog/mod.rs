//! The catalog: bundles their sharers chose to list, and the accounts that list, vote and
//! comment on them (D-025, D-128, D-129).
//!
//! [`Catalog`] holds the rules. It saves its state in a [`Store`], mirrors listed bundles in a
//! [`SearchIndex`] and keeps sign-in challenges in memory, since they last minutes. Callers pass
//! the time, in seconds since the Unix epoch, so the rules can be tested without a clock.
//! [`rank`] orders search results, and [`routes`] serves the catalog over HTTP.
//!
//! Operators run a catalog by choice; a tracker serves only the peer-list service without one.
//! What a catalog lists is its operator's responsibility (D-031).

pub mod rank;
pub(crate) mod routes;
pub mod search;
pub mod store;

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, UnparsedPublicKey};
use tempest_protocol::tracker::{BundleStats, ErrorCode, Signature};
use tempest_protocol::{BundleId, Hash, PeerId};
use tempest_tracker_api::account::{
    Account, AccountEdit, CODE_LEN, Challenge, Poll, PollResponse, Session, SignIn, code_from,
    normalize_code, sign_in_message,
};
use tempest_tracker_api::listing::{
    Comment, Content, Listing, ListingEdit, ListingPage, ListingStatus, MAX_PAGE, NewComment,
    NewReport, Vote,
};
use tempest_tracker_api::query::ListingQuery;
use tempest_tracker_api::{Category, TrackerInfo, codes, default_categories, text};

pub use search::SearchIndex;
pub use store::{Report, Store, StoreError};

use crate::limits::client_key;
use crate::swarms::Refusal;

/// The length of a day, in seconds: downloads are counted by day.
const DAY: u64 = 24 * 60 * 60;

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
    /// The most sign-in challenges outstanding for one client IP (an IPv6 /64), so no one
    /// client can fill the table.
    pub max_challenges_per_ip: usize,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            categories: default_categories(),
            invites_required: false,
            session_lifetime: Duration::from_secs(30 * 24 * 60 * 60),
            max_challenges_per_ip: 16,
        }
    }
}

/// A sign-in challenge waiting to be signed, or for its session to be collected.
#[derive(Debug)]
struct Pending {
    /// The SHA-256 of its poll secret.
    poll_hash: Vec<u8>,
    expires: u64,
    /// The client that asked for it, as [`client_key`] groups them.
    client: IpAddr,
    /// The session, once the code is signed, until it is collected.
    session: Option<Session>,
}

/// A tracker's catalog.
#[derive(Debug)]
pub struct Catalog {
    store: Store,
    search: SearchIndex,
    config: CatalogConfig,
    challenges: HashMap<String, Pending>,
    /// The bundles taken down as of the last time the index was brought up to date.
    blocked: HashSet<BundleId>,
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

/// The name of the category whose slug is `slug`.
fn category_name<'a>(categories: &'a [Category], slug: &Option<String>) -> Option<&'a str> {
    let slug = slug.as_deref()?;
    let category = categories.iter().find(|c| c.slug == slug)?;
    Some(&category.name)
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

fn banned() -> Refusal {
    forbidden("this key is banned from the catalog")
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
        Self::new(Store::open(path)?, config)
    }

    /// A catalog kept in memory, for tests.
    pub fn in_memory(config: CatalogConfig) -> Result<Self, StoreError> {
        Self::new(Store::in_memory()?, config)
    }

    /// The catalog of `store`, with its search index built from it.
    fn new(store: Store, config: CatalogConfig) -> Result<Self, StoreError> {
        let mut catalog = Self {
            store,
            search: SearchIndex::new()?,
            config,
            challenges: HashMap::new(),
            blocked: HashSet::new(),
        };
        catalog.blocked = catalog.store.blocked()?;
        let listed = catalog.store.all_indexed()?;
        let categories = &catalog.config.categories;
        catalog.search.rebuild(
            listed
                .iter()
                .map(|listing| (listing, category_name(categories, &listing.fields.category))),
        )?;
        Ok(catalog)
    }

    /// Brings the search index up to date with `bundle`'s listing: there if listed, else not.
    fn reindex(&mut self, bundle: &BundleId) -> Result<(), StoreError> {
        match self.store.indexed(bundle)? {
            Some(listing) => {
                let name = category_name(&self.config.categories, &listing.fields.category);
                self.search.put(&listing, name)?;
            }
            None => self.search.remove(bundle)?,
        }
        Ok(())
    }

    /// Brings the index up to date with listings taken down or restored by the operator's
    /// commands, which may run in another process and can't update this index.
    fn sync_blocked(&mut self) -> Result<(), StoreError> {
        let now = self.store.blocked()?;
        let changed: Vec<BundleId> = now.symmetric_difference(&self.blocked).copied().collect();
        for bundle in &changed {
            self.reindex(bundle)?;
        }
        self.blocked = now;
        Ok(())
    }

    /// What the catalog is like.
    pub fn info(&self) -> TrackerInfo {
        TrackerInfo {
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

    // The operator's tools. The operator's commands run on the database, possibly while a
    // tracker serves it from another process, so reads check the database rather than rely on
    // this process's index alone.

    /// Hides `bundle`'s listing from everyone but its publisher, or shows it again. Fails if
    /// there is no such listing.
    pub fn set_hidden(&mut self, bundle: &BundleId, hidden: bool) -> Result<(), Refusal> {
        if !self.store.set_hidden(bundle, hidden)? {
            return Err(not_found("listing"));
        }
        Ok(self.reindex(bundle)?)
    }

    /// Bans `key`, or lifts the ban. A banned key's listings are hidden, and it can't list,
    /// edit, vote, comment or report. Fails if there is no such account.
    pub fn set_banned(&mut self, key: &PeerId, banned: bool) -> Result<(), Refusal> {
        if !self.store.set_banned(key, banned)? {
            return Err(not_found("account"));
        }
        for bundle in self.store.bundles_of(key)? {
            self.reindex(&bundle)?;
        }
        Ok(())
    }

    /// Whether `key` is banned.
    pub fn is_banned(&self, key: &PeerId) -> Result<bool, StoreError> {
        self.store.is_banned(key)
    }

    /// The reports, newest first: those of `bundle` if given, else all.
    pub fn reports(&self, bundle: Option<&BundleId>) -> Result<Vec<Report>, StoreError> {
        self.store.reports(bundle)
    }

    // Signing in.

    /// A new sign-in challenge, for a client at `ip`.
    pub fn challenge(&mut self, now: u64, ip: IpAddr) -> Result<Challenge, Refusal> {
        self.challenges.retain(|_, pending| pending.expires > now);
        let client = client_key(ip);
        let mine = self.challenges.values().filter(|p| p.client == client);
        if mine.count() >= self.config.max_challenges_per_ip {
            let refusal = Refusal::new(ErrorCode::RateLimited, "too many sign-in codes at once");
            return Err(refusal.retry_after(Duration::from_secs(60)));
        }
        if self.challenges.len() >= MAX_CHALLENGES {
            let refusal = Refusal::new(ErrorCode::Unavailable, "too many sign-ins at once");
            return Err(refusal.retry_after(Duration::from_secs(60)));
        }
        let code = code_from(random::<CODE_LEN>()?);
        let poll = Hash::new(random::<32>()?).to_string();
        let pending = Pending {
            poll_hash: sha256(&poll),
            expires: now + CHALLENGE_LIFETIME,
            client,
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

    /// Signs in with a challenge signed for the tracker at `authority`, its public URL's (see
    /// [`crate::Tracker::public_url`]).
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
            return Err(failed(&format!(
                "the signature doesn't verify for this key, code and the tracker address {authority}"
            )));
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
    /// The bundle's first publisher keeps it, and a unpublished listing stays unpublished. Where
    /// invites are required, only keys with accounts can list; elsewhere listing makes one.
    pub fn request_listing(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
    ) -> Result<(), StoreError> {
        if self.store.is_banned(key)? {
            return Ok(());
        }
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

    /// Whether `bundle` has a listing in any state.
    pub fn knows_listing(&self, bundle: &BundleId) -> Result<bool, StoreError> {
        Ok(self.store.listing_owner(bundle)?.is_some())
    }

    /// Lists `bundle` for `key`, or changes `key`'s listing of it. A unpublished listing is listed
    /// again.
    pub fn edit_listing(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
        edit: &ListingEdit,
    ) -> Result<Listing, Refusal> {
        if self.store.is_banned(key)? {
            return Err(banned());
        }
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
            Some((_, ListingStatus::Unpublished)) => self.store.relist(bundle, now)?,
            Some(_) => {}
        }
        let mut fields = self.store.listing_fields(bundle)?;
        fields.title = title.unwrap_or(fields.title);
        fields.description = description.unwrap_or(fields.description);
        fields.category = edit.category.clone().unwrap_or(fields.category);
        fields.tags = tags.unwrap_or(fields.tags);
        self.store.set_listing_fields(bundle, &fields, now)?;
        self.reindex(bundle)?;
        self.listing(bundle, Some(key))
    }

    /// Takes `key`'s listing of `bundle` down.
    pub fn unpublish(&mut self, now: u64, bundle: &BundleId, key: &PeerId) -> Result<(), Refusal> {
        match self.store.listing_owner(bundle)? {
            None => Err(not_found("listing")),
            Some((owner, _)) if owner != *key => {
                Err(forbidden("only the listing's publisher can unpublish it"))
            }
            Some(_) => {
                self.store
                    .set_status(bundle, ListingStatus::Unpublished, now)?;
                Ok(self.reindex(bundle)?)
            }
        }
    }

    /// Records what `bundle` holds, from its manifest, which lists it if it was pending.
    pub fn set_content(&mut self, bundle: &BundleId, content: &Content) -> Result<(), StoreError> {
        self.store.set_content(bundle, content)?;
        self.reindex(bundle)
    }

    /// Counts a download of `bundle` at `now`, if it has a listing: a peer announced that it
    /// completed the bundle.
    pub fn record_download(&mut self, now: u64, bundle: &BundleId) -> Result<(), StoreError> {
        self.store.record_download(bundle, now / DAY)
    }

    /// The bundles whose listings wait for their manifests, with their publishers, oldest
    /// first.
    pub fn pending(&self) -> Result<Vec<(BundleId, PeerId)>, StoreError> {
        self.store.pending()
    }

    /// `bundle`'s listing as `viewer` sees it: only its publisher sees it unless it is listed
    /// and neither hidden nor its publisher banned.
    /// Its swarm counts are zero; the swarms aren't the catalog's.
    pub fn listing(&self, bundle: &BundleId, viewer: Option<&PeerId>) -> Result<Listing, Refusal> {
        match self.store.listing(bundle, viewer)? {
            Some(listing)
                if (listing.status == ListingStatus::Listed
                    && !self.store.is_blocked(bundle)?)
                    || Some(&listing.publisher.key) == viewer =>
            {
                Ok(listing)
            }
            _ => Err(not_found("listing")),
        }
    }

    /// The listing of `bundle` if it is listed and not taken down, for votes, comments and
    /// reports, by `key`, who must not be banned.
    fn listed(&self, bundle: &BundleId, key: &PeerId) -> Result<(), Refusal> {
        if self.store.is_banned(key)? {
            return Err(banned());
        }
        match self.store.listing_owner(bundle)? {
            Some((_, ListingStatus::Listed)) if !self.store.is_blocked(bundle)? => Ok(()),
            _ => Err(not_found("listing")),
        }
    }

    /// A page of the listed bundles that match `query`, as `viewer` sees them, with how the
    /// matches spread over categories and tags. `swarm` gives a bundle's swarm now, for ranking
    /// and to show.
    pub fn listings(
        &mut self,
        now: u64,
        query: &ListingQuery,
        viewer: Option<&PeerId>,
        swarm: impl Fn(&BundleId) -> BundleStats,
    ) -> Result<ListingPage, Refusal> {
        self.sync_blocked()?;
        let found = self
            .search
            .search(&query.q, query.category.as_deref(), &query.tags)
            .map_err(StoreError::from)?;
        let today = now / DAY;
        let mut votes = self.store.vote_counts()?;
        let mut downloads = self
            .store
            .downloads_since(today.saturating_sub(rank::TRENDING_DAYS))?;
        let mut ranked: Vec<rank::Ranked> = found
            .matches
            .into_iter()
            .map(|found| rank::Ranked {
                bundle: found.bundle,
                text_score: found.score,
                signals: rank::Signals {
                    created: found.created,
                    swarm: swarm(&found.bundle),
                    votes: votes.remove(&found.bundle).unwrap_or_default(),
                    trending: rank::trending(
                        downloads.remove(&found.bundle).unwrap_or_default(),
                        today,
                    ),
                },
            })
            .collect();
        rank::sort(&mut ranked, query.sort);

        let total = ranked.len() as u64;
        let start = usize::try_from(query.offset).unwrap_or(usize::MAX);
        let limit = query.limit.min(MAX_PAGE) as usize;
        let mut listings = Vec::new();
        for ranked in ranked.iter().skip(start).take(limit) {
            // The store and the index change together under the catalog's lock.
            if let Some(mut listing) = self.store.listing(&ranked.bundle, viewer)? {
                listing.swarm = ranked.signals.swarm;
                listings.push(listing);
            }
        }
        Ok(ListingPage {
            listings,
            total,
            facets: found.facets,
        })
    }

    /// A page of the listings `key` asked for, whatever their status.
    pub fn published_by(
        &self,
        key: &PeerId,
        offset: u64,
        limit: u32,
    ) -> Result<ListingPage, Refusal> {
        let (listings, total) = self.store.published_by(key, offset, limit.min(MAX_PAGE))?;
        Ok(ListingPage {
            listings,
            total,
            facets: Default::default(),
        })
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
        self.listed(bundle, key)?;
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
        self.listed(bundle, key)?;
        Ok(self.store.add_comment(bundle, key, &body, now)?)
    }

    /// Records `key`'s report of `bundle`'s listing, for the operator. A key's later report of
    /// the same listing replaces its earlier one.
    pub fn report(
        &mut self,
        now: u64,
        bundle: &BundleId,
        key: &PeerId,
        report: &NewReport,
    ) -> Result<(), Refusal> {
        let reason = text::clean_report(&report.reason).map_err(invalid)?;
        self.listed(bundle, key)?;
        Ok(self.store.add_report(bundle, key, &reason, now)?)
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
