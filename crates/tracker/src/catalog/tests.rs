//! The catalog's rules, without HTTP.

use std::net::{IpAddr, Ipv4Addr};

use ring::signature::{Ed25519KeyPair, KeyPair};
use tempest_protocol::tracker::{BundleStats, Signature, listing_message};
use tempest_protocol::{BundleId, PeerId};
use tempest_tracker_api::account::{AccountEdit, Poll, SignIn, sign_in_message};
use tempest_tracker_api::codes;
use tempest_tracker_api::listing::{
    Content, ListingEdit, ListingPage, ListingStatus, NewComment, Vote,
};
use tempest_tracker_api::query::{ListingQuery, Sort};

use super::{Catalog, CatalogConfig, verify};

const AUTHORITY: &str = "tracker.example.org";

/// The client every test signs in from.
const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 9));

/// A key made from a seed of `n`s.
struct Key(Ed25519KeyPair);

impl Key {
    fn new(n: u8) -> Self {
        Self(Ed25519KeyPair::from_seed_unchecked(&[n; 32]).unwrap())
    }

    fn id(&self) -> PeerId {
        PeerId::new(self.0.public_key().as_ref().try_into().unwrap())
    }

    fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.0.sign(message).as_ref().try_into().unwrap())
    }
}

fn bundle(n: u8) -> BundleId {
    BundleId::new([n; 32])
}

fn content(name: &str) -> Content {
    Content {
        name: name.into(),
        size: 1000,
        files: 2,
    }
}

fn catalog(invites_required: bool) -> Catalog {
    let config = CatalogConfig {
        invites_required,
        ..CatalogConfig::default()
    };
    Catalog::in_memory(config).unwrap()
}

/// Signs in `key` at time `now` and returns the session token.
fn sign_in(catalog: &mut Catalog, key: &Key, now: u64) -> String {
    let challenge = catalog.challenge(now, CLIENT).unwrap();
    let request = SignIn {
        code: challenge.code.clone(),
        key: key.id(),
        sig: key.sign(&sign_in_message(AUTHORITY, &challenge.code)),
        invite: None,
    };
    catalog.sign_in(now, AUTHORITY, &request).unwrap().token
}

/// The listings matching `query`, when every swarm is empty.
fn browse(catalog: &mut Catalog, query: &ListingQuery) -> ListingPage {
    catalog
        .listings(0, query, None, |_| BundleStats::default())
        .unwrap()
}

/// A listed bundle published by `key`.
fn publish(catalog: &mut Catalog, key: &Key, n: u8, now: u64) -> BundleId {
    let bundle = bundle(n);
    catalog.request_listing(now, &bundle, &key.id()).unwrap();
    catalog
        .set_content(&bundle, &content(&format!("bundle {n}")))
        .unwrap();
    bundle
}

#[test]
fn the_specs_listing_test_vector_verifies() {
    // RFC 8032's first test key.
    let key = Ed25519KeyPair::from_seed_unchecked(&hex32(
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
    ))
    .unwrap();
    let peer = PeerId::new(key.public_key().as_ref().try_into().unwrap());
    assert_eq!(
        peer.to_string(),
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
    );
    let bundle: BundleId = format!("3f9a{}", "0".repeat(60)).parse().unwrap();
    let sig: Signature = "ad66ac47ca8e640246582dcc0000087592145360a7a7f3b7385efd630f83a2aa\
                          706008afbc7f1499e94be591e8e4948f372ed9d26c7077f4a1c8205e86acdd0d"
        .parse()
        .unwrap();
    assert!(verify(&peer, &listing_message(&bundle), &sig));
    assert!(!verify(&peer, &listing_message(&BundleId::ZERO), &sig));
}

fn hex32(text: &str) -> [u8; 32] {
    *text.parse::<PeerId>().unwrap().as_bytes()
}

#[test]
fn signing_in_proves_the_key_and_the_tracker() {
    let mut catalog = catalog(false);
    let key = Key::new(1);
    let challenge = catalog.challenge(100, CLIENT).unwrap();
    let request = |authority: &str, signer: &Key| SignIn {
        code: challenge.code.to_lowercase(),
        key: key.id(),
        sig: signer.sign(&sign_in_message(authority, &challenge.code)),
        invite: None,
    };
    let failed = |result: Result<_, crate::swarms::Refusal>| {
        let refusal = result.unwrap_err();
        assert_eq!(refusal.status, 401);
        assert_eq!(refusal.body.error.as_str(), codes::SIGN_IN_FAILED);
    };
    // Signed for another tracker, or by another key.
    failed(catalog.sign_in(100, AUTHORITY, &request("evil.example", &key)));
    failed(catalog.sign_in(100, AUTHORITY, &request(AUTHORITY, &Key::new(2))));

    let session = catalog
        .sign_in(100, AUTHORITY, &request(AUTHORITY, &key))
        .unwrap();
    assert_eq!(session.account.key, key.id());
    assert_eq!(session.account.joined, 100);
    assert_eq!(catalog.authenticate(101, &session.token).unwrap(), key.id());
    // A code signs in once.
    failed(catalog.sign_in(100, AUTHORITY, &request(AUTHORITY, &key)));

    // The session lasts its lifetime, and signing out ends it.
    let lifetime = session.expires_in_secs;
    assert!(
        catalog
            .authenticate(100 + lifetime, &session.token)
            .is_err()
    );
    catalog.sign_out(&session.token).unwrap();
    assert!(catalog.authenticate(101, &session.token).is_err());
}

#[test]
fn codes_expire() {
    let mut catalog = catalog(false);
    let key = Key::new(1);
    let challenge = catalog.challenge(0, CLIENT).unwrap();
    let request = SignIn {
        code: challenge.code.clone(),
        key: key.id(),
        sig: key.sign(&sign_in_message(AUTHORITY, &challenge.code)),
        invite: None,
    };
    assert!(
        catalog
            .sign_in(challenge.expires_in_secs, AUTHORITY, &request)
            .is_err()
    );
}

#[test]
fn whoever_asked_collects_the_session_once() {
    let mut catalog = catalog(false);
    let key = Key::new(1);
    let challenge = catalog.challenge(0, CLIENT).unwrap();
    let poll = Poll {
        code: challenge.code.clone(),
        poll: challenge.poll.clone(),
    };
    assert_eq!(catalog.poll(1, &poll).unwrap().session, None);
    let wrong = Poll {
        poll: "0".repeat(64),
        ..poll.clone()
    };
    assert!(catalog.poll(1, &wrong).is_err());

    let request = SignIn {
        code: challenge.code.clone(),
        key: key.id(),
        sig: key.sign(&sign_in_message(AUTHORITY, &challenge.code)),
        invite: None,
    };
    let signed = catalog.sign_in(2, AUTHORITY, &request).unwrap();
    assert_eq!(catalog.poll(3, &poll).unwrap().session, Some(signed));
    assert!(catalog.poll(4, &poll).is_err());
}

#[test]
fn invites_admit_new_keys_once() {
    let mut catalog = catalog(true);
    let (alice, bob) = (Key::new(1), Key::new(2));
    let invite = catalog.create_invite(0).unwrap();
    let attempt = |catalog: &mut Catalog, key: &Key, invite: Option<String>| {
        let challenge = catalog.challenge(0, CLIENT).unwrap();
        let request = SignIn {
            code: challenge.code.clone(),
            key: key.id(),
            sig: key.sign(&sign_in_message(AUTHORITY, &challenge.code)),
            invite,
        };
        catalog.sign_in(0, AUTHORITY, &request)
    };
    let refused = attempt(&mut catalog, &alice, None).unwrap_err();
    assert_eq!(refused.body.error.as_str(), codes::INVITE_REQUIRED);
    attempt(&mut catalog, &alice, Some(invite.to_lowercase())).unwrap();
    // Members sign in again without one; the invite is used up.
    attempt(&mut catalog, &alice, None).unwrap();
    assert!(attempt(&mut catalog, &bob, Some(invite)).is_err());

    // Announces list only for members.
    catalog.request_listing(0, &bundle(1), &bob.id()).unwrap();
    catalog.request_listing(0, &bundle(2), &alice.id()).unwrap();
    assert_eq!(catalog.pending().unwrap(), [(bundle(2), alice.id())]);
}

#[test]
fn listings_show_once_their_content_is_known() {
    let mut catalog = catalog(false);
    let (alice, bob) = (Key::new(1), Key::new(2));
    let id = bundle(7);
    catalog.request_listing(10, &id, &alice.id()).unwrap();
    // Pending: only the publisher sees it.
    assert!(catalog.listing(&id, None).is_err());
    assert!(catalog.listing(&id, Some(&bob.id())).is_err());
    let mine = catalog.listing(&id, Some(&alice.id())).unwrap();
    assert_eq!(mine.status, ListingStatus::Pending);
    assert_eq!(browse(&mut catalog, &ListingQuery::default()).total, 0);

    // Another key's request doesn't take it over.
    catalog.request_listing(11, &id, &bob.id()).unwrap();
    catalog.set_content(&id, &content("weights")).unwrap();
    let listing = catalog.listing(&id, None).unwrap();
    assert_eq!(listing.status, ListingStatus::Listed);
    assert_eq!(listing.publisher.key, alice.id());
    assert_eq!(listing.title, "weights");
    assert_eq!(listing.content, Some(content("weights")));
    assert!(catalog.pending().unwrap().is_empty());
}

#[test]
fn publishers_edit_and_unpublish_their_listings() {
    let mut catalog = catalog(false);
    let (alice, bob) = (Key::new(1), Key::new(2));
    let id = publish(&mut catalog, &alice, 1, 0);
    let edit = ListingEdit {
        title: Some("  Weather data ".into()),
        description: Some("Hourly.\r\nSince 1990.".into()),
        category: Some(Some("datasets".into())),
        tags: Some(vec!["Climate".into(), "open data".into(), "climate".into()]),
    };
    let listing = catalog.edit_listing(5, &id, &alice.id(), &edit).unwrap();
    assert_eq!(listing.title, "Weather data");
    assert_eq!(listing.description, "Hourly.\nSince 1990.");
    assert_eq!(listing.category.as_deref(), Some("datasets"));
    assert_eq!(listing.tags, ["climate", "open-data"]);
    assert_eq!(listing.updated, 5);

    // Fields left out stay; a null category clears it.
    let clear = ListingEdit {
        category: Some(None),
        ..ListingEdit::default()
    };
    let listing = catalog.edit_listing(6, &id, &alice.id(), &clear).unwrap();
    assert_eq!(
        (listing.title.as_str(), listing.category),
        ("Weather data", None)
    );

    // Others can't edit; unknown categories and bad text are refused.
    assert_eq!(
        catalog
            .edit_listing(7, &id, &bob.id(), &edit)
            .unwrap_err()
            .status,
        403
    );
    let unknown = ListingEdit {
        category: Some(Some("warez".into())),
        ..ListingEdit::default()
    };
    assert_eq!(
        catalog
            .edit_listing(7, &id, &alice.id(), &unknown)
            .unwrap_err()
            .status,
        400
    );
    let bad = ListingEdit {
        title: Some("two\nlines".into()),
        ..ListingEdit::default()
    };
    assert_eq!(
        catalog
            .edit_listing(7, &id, &alice.id(), &bad)
            .unwrap_err()
            .status,
        400
    );

    // Unpublished listings hide, and announces don't bring them back; an edit does.
    assert_eq!(
        catalog.unpublish(8, &id, &bob.id()).unwrap_err().status,
        403
    );
    catalog.unpublish(8, &id, &alice.id()).unwrap();
    assert!(catalog.listing(&id, None).is_err());
    catalog.request_listing(9, &id, &alice.id()).unwrap();
    assert!(catalog.listing(&id, None).is_err());
    catalog
        .edit_listing(10, &id, &alice.id(), &ListingEdit::default())
        .unwrap();
    assert_eq!(
        catalog.listing(&id, None).unwrap().status,
        ListingStatus::Listed
    );
}

#[test]
fn an_edit_can_ask_for_a_listing() {
    let mut catalog = catalog(false);
    let alice = Key::new(1);
    sign_in(&mut catalog, &alice, 0);
    let edit = ListingEdit {
        title: Some("Model".into()),
        ..ListingEdit::default()
    };
    let listing = catalog
        .edit_listing(1, &bundle(3), &alice.id(), &edit)
        .unwrap();
    assert_eq!(listing.status, ListingStatus::Pending);
    assert_eq!(catalog.pending().unwrap(), [(bundle(3), alice.id())]);
    let page = catalog.published_by(&alice.id(), 0, 10).unwrap();
    assert_eq!(page.total, 1);
}

#[test]
fn listings_that_stay_pending_are_dropped() {
    let mut catalog = catalog(false);
    let alice = Key::new(1);
    let lifetime = std::time::Duration::from_secs(100);
    catalog.request_listing(0, &bundle(1), &alice.id()).unwrap();
    catalog.request_listing(50, &bundle(2), &alice.id()).unwrap();
    catalog.request_listing(50, &bundle(3), &alice.id()).unwrap();
    catalog.set_content(&bundle(3), &content("listed")).unwrap();

    assert_eq!(catalog.drop_pending(100, lifetime).unwrap(), 0);
    // An edit keeps a pending listing longer.
    sign_in(&mut catalog, &alice, 120);
    let edit = ListingEdit {
        title: Some("Kept".into()),
        ..ListingEdit::default()
    };
    catalog
        .edit_listing(120, &bundle(2), &alice.id(), &edit)
        .unwrap();
    assert_eq!(catalog.drop_pending(151, lifetime).unwrap(), 1);
    assert_eq!(catalog.pending().unwrap(), [(bundle(2), alice.id())]);
    // Listed bundles stay, and a dropped one can be asked for again.
    assert!(catalog.listing(&bundle(3), None).is_ok());
    catalog.request_listing(160, &bundle(1), &alice.id()).unwrap();
    assert_eq!(catalog.pending().unwrap().len(), 2);
}

#[test]
fn browsing_pages_newest_first_and_filters_by_category() {
    let mut catalog = catalog(false);
    let alice = Key::new(1);
    for n in 1..=5 {
        let id = publish(&mut catalog, &alice, n, u64::from(n));
        let category = if n % 2 == 0 { "models" } else { "datasets" };
        let edit = ListingEdit {
            category: Some(Some(category.into())),
            ..ListingEdit::default()
        };
        catalog
            .edit_listing(u64::from(n), &id, &alice.id(), &edit)
            .unwrap();
    }
    let query = ListingQuery {
        sort: Sort::Newest,
        offset: 1,
        limit: 2,
        ..ListingQuery::default()
    };
    let page = browse(&mut catalog, &query);
    assert_eq!(page.total, 5);
    let order: Vec<_> = page.listings.iter().map(|l| l.bundle).collect();
    assert_eq!(order, [bundle(4), bundle(3)]);

    let models = ListingQuery {
        category: Some("models".into()),
        ..ListingQuery::default()
    };
    let page = browse(&mut catalog, &models);
    assert_eq!(page.total, 2);
    assert!(
        page.listings
            .iter()
            .all(|l| l.category.as_deref() == Some("models"))
    );
}

#[test]
fn votes_count_once_per_account() {
    let mut catalog = catalog(false);
    let (alice, bob) = (Key::new(1), Key::new(2));
    let id = publish(&mut catalog, &alice, 1, 0);
    sign_in(&mut catalog, &bob, 0);
    catalog.vote(1, &id, &alice.id(), Vote::Up).unwrap();
    catalog.vote(1, &id, &bob.id(), Vote::Up).unwrap();
    let listing = catalog.vote(2, &id, &bob.id(), Vote::Down).unwrap();
    assert_eq!((listing.votes.up, listing.votes.down), (1, 1));
    assert_eq!(listing.my_vote, Vote::Down);
    assert_eq!(
        catalog.listing(&id, Some(&alice.id())).unwrap().my_vote,
        Vote::Up
    );
    assert_eq!(catalog.listing(&id, None).unwrap().my_vote, Vote::None);
    let listing = catalog.vote(3, &id, &bob.id(), Vote::None).unwrap();
    assert_eq!((listing.votes.up, listing.votes.down), (1, 0));

    // Only listed bundles take votes.
    catalog.request_listing(4, &bundle(9), &alice.id()).unwrap();
    assert_eq!(
        catalog
            .vote(4, &bundle(9), &bob.id(), Vote::Up)
            .unwrap_err()
            .status,
        404
    );
}

#[test]
fn comments_belong_to_their_authors() {
    let mut catalog = catalog(false);
    let (alice, bob) = (Key::new(1), Key::new(2));
    let id = publish(&mut catalog, &alice, 1, 0);
    sign_in(&mut catalog, &bob, 0);
    catalog
        .edit_account(
            &bob.id(),
            &AccountEdit {
                name: Some(" Bob ".into()),
            },
        )
        .unwrap();
    let say = |body: &str| NewComment { body: body.into() };
    let first = catalog.comment(1, &id, &bob.id(), &say("Thanks!")).unwrap();
    assert_eq!(first.author.name.as_deref(), Some("Bob"));
    catalog
        .comment(2, &id, &alice.id(), &say("You're welcome"))
        .unwrap();
    assert_eq!(
        catalog
            .comment(3, &id, &bob.id(), &say(" "))
            .unwrap_err()
            .status,
        400
    );

    let comments = catalog.comments(&id, None).unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0], first);
    assert_eq!(catalog.listing(&id, None).unwrap().comments, 2);

    assert_eq!(
        catalog
            .delete_comment(first.id, &alice.id())
            .unwrap_err()
            .status,
        403
    );
    catalog.delete_comment(first.id, &bob.id()).unwrap();
    assert_eq!(
        catalog
            .delete_comment(first.id, &bob.id())
            .unwrap_err()
            .status,
        404
    );
    assert_eq!(catalog.comments(&id, None).unwrap().len(), 1);
}

#[test]
fn the_catalog_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    let alice = Key::new(1);
    let token = {
        let mut catalog = Catalog::open(&path, CatalogConfig::default()).unwrap();
        publish(&mut catalog, &alice, 1, 0);
        sign_in(&mut catalog, &alice, 0)
    };
    let catalog = Catalog::open(&path, CatalogConfig::default()).unwrap();
    assert_eq!(catalog.authenticate(1, &token).unwrap(), alice.id());
    assert_eq!(catalog.listing(&bundle(1), None).unwrap().title, "bundle 1");
}

#[test]
fn reopening_rebuilds_the_search_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    let alice = Key::new(1);
    {
        let mut catalog = Catalog::open(&path, CatalogConfig::default()).unwrap();
        let id = publish(&mut catalog, &alice, 1, 0);
        let edit = ListingEdit {
            title: Some("Orbital imagery".into()),
            category: Some(Some("datasets".into())),
            ..ListingEdit::default()
        };
        catalog.edit_listing(1, &id, &alice.id(), &edit).unwrap();
        publish(&mut catalog, &alice, 2, 0);
        catalog.unpublish(2, &bundle(2), &alice.id()).unwrap();
        // Pending listings aren't searchable either.
        catalog.request_listing(3, &bundle(3), &alice.id()).unwrap();
    }
    let mut catalog = Catalog::open(&path, CatalogConfig::default()).unwrap();
    let mut search = |q: &str| {
        let query = ListingQuery {
            q: q.into(),
            ..ListingQuery::default()
        };
        let page = browse(&mut catalog, &query);
        page.listings.iter().map(|l| l.bundle).collect::<Vec<_>>()
    };
    assert_eq!(search("orbitl"), [bundle(1)]);
    // The category's name is searchable as well as its slug's facet.
    assert_eq!(search("Datasets"), [bundle(1)]);
    assert_eq!(search(""), [bundle(1)]);
}

#[test]
fn downloads_count_by_day_and_trend() {
    const DAY: u64 = 24 * 60 * 60;
    let mut catalog = catalog(false);
    let alice = Key::new(1);
    let (old, new) = (
        publish(&mut catalog, &alice, 1, 0),
        publish(&mut catalog, &alice, 2, 0),
    );
    let today = 100 * DAY;
    for _ in 0..4 {
        catalog.record_download(today - 10 * DAY, &old).unwrap();
    }
    catalog.record_download(today, &new).unwrap();
    catalog.record_download(today + 5, &new).unwrap();
    // A bundle nobody listed isn't counted, and isn't an error.
    catalog.record_download(today, &bundle(9)).unwrap();

    let query = ListingQuery {
        sort: Sort::Trending,
        ..ListingQuery::default()
    };
    let page = catalog
        .listings(today, &query, None, |_| BundleStats::default())
        .unwrap();
    let order: Vec<_> = page
        .listings
        .iter()
        .map(|l| (l.bundle, l.downloads))
        .collect();
    // Four downloads ten days ago weigh less than two today.
    assert_eq!(order, [(new, 2), (old, 4)]);
}

#[test]
fn seeders_sort_uses_the_live_swarms() {
    let mut catalog = catalog(false);
    let alice = Key::new(1);
    for n in 1..=3 {
        publish(&mut catalog, &alice, n, u64::from(n));
    }
    let query = ListingQuery {
        sort: Sort::Seeders,
        ..ListingQuery::default()
    };
    let swarm = |id: &BundleId| BundleStats {
        complete: u64::from(id.as_bytes()[0] == 1),
        incomplete: u64::from(id.as_bytes()[0]),
    };
    let page = catalog.listings(0, &query, None, swarm).unwrap();
    let order: Vec<_> = page.listings.iter().map(|l| l.bundle).collect();
    assert_eq!(order, [bundle(1), bundle(3), bundle(2)]);
    assert_eq!(page.listings[1].swarm.incomplete, 3);
}

#[test]
fn one_client_cannot_hold_every_challenge() {
    let config = CatalogConfig {
        max_challenges_per_ip: 2,
        ..CatalogConfig::default()
    };
    let mut catalog = Catalog::in_memory(config).unwrap();
    catalog.challenge(0, CLIENT).unwrap();
    catalog.challenge(0, CLIENT).unwrap();
    let refused = catalog.challenge(0, CLIENT).unwrap_err();
    assert_eq!(refused.status, 429);
    assert_eq!(refused.body.retry_after_secs, Some(60));
    // Others still get codes, and the client gets more once its codes expire.
    let other = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 10));
    catalog.challenge(0, other).unwrap();
    catalog.challenge(10 * 60, CLIENT).unwrap();
}
