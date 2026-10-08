//! The catalog's saved state, in SQLite: accounts, sessions, invites, listings, votes,
//! comments, and downloads by day.
//!
//! The store only reads and writes; [`super::Catalog`] decides what is allowed. The schema is
//! created and upgraded by numbered migrations, tracked in SQLite's `user_version`, as the
//! engine's is. Times are seconds since the Unix epoch.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use tempest_catalog_api::listing::{
    Comment, Content, Listing, ListingStatus, Person, Vote, VoteCounts,
};
use tempest_protocol::tracker::BundleStats;
use tempest_protocol::{BundleId, Hash, PeerId};

/// The migrations, in order. Migration `i` takes the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    "
CREATE TABLE accounts (
    key BLOB PRIMARY KEY NOT NULL,
    name TEXT,
    joined INTEGER NOT NULL
) STRICT;
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY NOT NULL,
    key BLOB NOT NULL REFERENCES accounts (key) ON DELETE CASCADE,
    expires INTEGER NOT NULL
) STRICT;
CREATE INDEX sessions_by_expiry ON sessions (expires);
CREATE TABLE invites (
    code TEXT PRIMARY KEY NOT NULL,
    created INTEGER NOT NULL,
    used_by BLOB REFERENCES accounts (key)
) STRICT;
CREATE TABLE listings (
    bundle BLOB PRIMARY KEY NOT NULL,
    publisher BLOB NOT NULL REFERENCES accounts (key),
    status TEXT NOT NULL CHECK (status IN ('pending', 'listed', 'withdrawn')),
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    category TEXT,
    content_name TEXT,
    content_size INTEGER,
    content_files INTEGER,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL
) STRICT;
CREATE INDEX listings_by_publisher ON listings (publisher, updated);
CREATE INDEX listings_by_status ON listings (status, created);
CREATE TABLE listing_tags (
    bundle BLOB NOT NULL REFERENCES listings (bundle) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (bundle, position)
) STRICT;
CREATE TABLE votes (
    bundle BLOB NOT NULL REFERENCES listings (bundle) ON DELETE CASCADE,
    key BLOB NOT NULL REFERENCES accounts (key) ON DELETE CASCADE,
    value INTEGER NOT NULL CHECK (value IN (-1, 1)),
    at INTEGER NOT NULL,
    PRIMARY KEY (bundle, key)
) STRICT;
CREATE TABLE comments (
    id INTEGER PRIMARY KEY,
    bundle BLOB NOT NULL REFERENCES listings (bundle) ON DELETE CASCADE,
    author BLOB NOT NULL REFERENCES accounts (key) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created INTEGER NOT NULL
) STRICT;
CREATE INDEX comments_by_listing ON comments (bundle, created);
",
    "
CREATE TABLE downloads (
    bundle BLOB NOT NULL REFERENCES listings (bundle) ON DELETE CASCADE,
    day INTEGER NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (bundle, day)
) STRICT;
CREATE INDEX downloads_by_day ON downloads (day);
",
];

/// Why the catalog's database failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite failed.
    #[error("catalog database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database was written by a newer version of Tempest.
    #[error("the catalog database has schema version {found}, newer than this version's {known}")]
    TooNew {
        /// The database's version.
        found: u32,
        /// The newest version this build knows.
        known: u32,
    },
    /// A row holds something this build can't read.
    #[error("catalog database: {0}")]
    Corrupt(String),
    /// The search index, which mirrors the database, failed.
    #[error("catalog search index: {0}")]
    Search(#[from] tantivy::TantivyError),
}

/// A listing's own fields, as a publisher sets them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListingFields {
    /// The title, possibly empty.
    pub title: String,
    /// The description, possibly empty.
    pub description: String,
    /// The category's slug.
    pub category: Option<String>,
    /// The tags, normalized.
    pub tags: Vec<String>,
}

/// What the search index holds of a listed bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    /// The bundle.
    pub bundle: BundleId,
    /// The publisher's fields.
    pub fields: ListingFields,
    /// The content's name.
    pub content_name: String,
    /// When the listing was first asked for.
    pub created: u64,
}

/// The catalog's database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn hash_of(bytes: Vec<u8>) -> Result<Hash, StoreError> {
    let bytes = <[u8; 32]>::try_from(bytes)
        .map_err(|_| StoreError::Corrupt("a key or bundle ID isn't 32 bytes".into()))?;
    Ok(Hash::new(bytes))
}

fn status_text(status: ListingStatus) -> &'static str {
    match status {
        ListingStatus::Pending => "pending",
        ListingStatus::Listed => "listed",
        ListingStatus::Withdrawn => "withdrawn",
    }
}

fn parse_status(text: &str) -> Result<ListingStatus, StoreError> {
    match text {
        "pending" => Ok(ListingStatus::Pending),
        "listed" => Ok(ListingStatus::Listed),
        "withdrawn" => Ok(ListingStatus::Withdrawn),
        other => Err(StoreError::Corrupt(format!("unknown status {other:?}"))),
    }
}

/// The columns [`Store::read_listing`] reads, from `listings l` joined with `accounts a` on the
/// publisher. The one parameter, `?1`, is the viewer's key or `NULL`.
const LISTING_COLUMNS: &str = "
    l.bundle, l.title, l.description, l.category, l.publisher, a.name, l.status, l.created,
    l.updated, l.content_name, l.content_size, l.content_files,
    (SELECT count(*) FROM votes v WHERE v.bundle = l.bundle AND v.value = 1),
    (SELECT count(*) FROM votes v WHERE v.bundle = l.bundle AND v.value = -1),
    (SELECT v.value FROM votes v WHERE v.bundle = l.bundle AND v.key = ?1),
    (SELECT count(*) FROM comments c WHERE c.bundle = l.bundle),
    (SELECT coalesce(sum(d.count), 0) FROM downloads d WHERE d.bundle = l.bundle)";

impl Store {
    /// Opens the database at `path`, creating it if needed, and brings its schema up to date.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::init(Connection::open(path)?)
    }

    /// A database in memory, for tests.
    pub fn in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let known = MIGRATIONS.len() as u32;
        let tx = self.conn.transaction()?;
        let found: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if found > known {
            return Err(StoreError::TooNew { found, known });
        }
        for migration in &MIGRATIONS[found as usize..] {
            tx.execute_batch(migration)?;
        }
        tx.pragma_update(None, "user_version", known)?;
        tx.commit()?;
        Ok(())
    }

    // Accounts and sessions.

    /// The account of `key`, if it has one.
    pub fn account(&self, key: &PeerId) -> Result<Option<(Option<String>, u64)>, StoreError> {
        let row = self
            .conn
            .query_row(
                "SELECT name, joined FROM accounts WHERE key = ?1",
                [key.as_bytes()],
                |row| Ok((row.get(0)?, to_u64(row.get(1)?))),
            )
            .optional()?;
        Ok(row)
    }

    /// Makes an account for `key`, which has none yet.
    pub fn create_account(&self, key: &PeerId, now: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO accounts (key, joined) VALUES (?1, ?2)",
            params![key.as_bytes(), to_i64(now)],
        )?;
        Ok(())
    }

    /// Sets the name `key`'s account shows.
    pub fn set_name(&self, key: &PeerId, name: Option<&str>) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE accounts SET name = ?2 WHERE key = ?1",
            params![key.as_bytes(), name],
        )?;
        Ok(())
    }

    /// Saves a session for `key`, identified by its token's hash, and forgets expired ones.
    pub fn create_session(
        &self,
        token_hash: &[u8],
        key: &PeerId,
        expires: u64,
        now: u64,
    ) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sessions WHERE expires <= ?1", [to_i64(now)])?;
        self.conn.execute(
            "INSERT INTO sessions (token_hash, key, expires) VALUES (?1, ?2, ?3)",
            params![token_hash, key.as_bytes(), to_i64(expires)],
        )?;
        Ok(())
    }

    /// The key of the unexpired session whose token has `token_hash`.
    pub fn session(&self, token_hash: &[u8], now: u64) -> Result<Option<PeerId>, StoreError> {
        let key: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT key FROM sessions WHERE token_hash = ?1 AND expires > ?2",
                params![token_hash, to_i64(now)],
                |row| row.get(0),
            )
            .optional()?;
        key.map(hash_of).transpose()
    }

    /// Forgets a session.
    pub fn delete_session(&self, token_hash: &[u8]) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
        Ok(())
    }

    // Invites.

    /// Saves a new invite.
    pub fn create_invite(&self, code: &str, now: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO invites (code, created) VALUES (?1, ?2)",
            params![code, to_i64(now)],
        )?;
        Ok(())
    }

    /// Makes an account for `key` using the invite `code`, if the invite exists and is unused.
    /// Returns whether it did.
    pub fn join_with_invite(
        &mut self,
        key: &PeerId,
        code: &str,
        now: u64,
    ) -> Result<bool, StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO accounts (key, joined) VALUES (?1, ?2)",
            params![key.as_bytes(), to_i64(now)],
        )?;
        let used = tx.execute(
            "UPDATE invites SET used_by = ?2 WHERE code = ?1 AND used_by IS NULL",
            params![code, key.as_bytes()],
        )?;
        if used == 1 {
            tx.commit()?;
        }
        Ok(used == 1)
    }

    // Listings.

    /// The publisher and status of `bundle`'s listing, if it has one.
    pub fn listing_owner(
        &self,
        bundle: &BundleId,
    ) -> Result<Option<(PeerId, ListingStatus)>, StoreError> {
        let row: Option<(Vec<u8>, String)> = self
            .conn
            .query_row(
                "SELECT publisher, status FROM listings WHERE bundle = ?1",
                [bundle.as_bytes()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(key, status)| Ok((hash_of(key)?, parse_status(&status)?)))
            .transpose()
    }

    /// Makes a pending listing of `bundle` for `publisher`.
    pub fn create_listing(
        &self,
        bundle: &BundleId,
        publisher: &PeerId,
        now: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO listings (bundle, publisher, status, created, updated)
             VALUES (?1, ?2, 'pending', ?3, ?3)",
            params![bundle.as_bytes(), publisher.as_bytes(), to_i64(now)],
        )?;
        Ok(())
    }

    /// The publisher's fields of `bundle`'s listing.
    pub fn listing_fields(&self, bundle: &BundleId) -> Result<ListingFields, StoreError> {
        let (title, description, category) = self.conn.query_row(
            "SELECT title, description, category FROM listings WHERE bundle = ?1",
            [bundle.as_bytes()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        Ok(ListingFields {
            title,
            description,
            category,
            tags: self.tags(bundle)?,
        })
    }

    /// Replaces the publisher's fields of `bundle`'s listing.
    pub fn set_listing_fields(
        &mut self,
        bundle: &BundleId,
        fields: &ListingFields,
        now: u64,
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE listings SET title = ?2, description = ?3, category = ?4, updated = ?5
             WHERE bundle = ?1",
            params![
                bundle.as_bytes(),
                fields.title,
                fields.description,
                fields.category,
                to_i64(now)
            ],
        )?;
        tx.execute(
            "DELETE FROM listing_tags WHERE bundle = ?1",
            [bundle.as_bytes()],
        )?;
        for (position, tag) in fields.tags.iter().enumerate() {
            tx.execute(
                "INSERT INTO listing_tags (bundle, position, tag) VALUES (?1, ?2, ?3)",
                params![bundle.as_bytes(), position as i64, tag],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets the status of `bundle`'s listing.
    pub fn set_status(
        &self,
        bundle: &BundleId,
        status: ListingStatus,
        now: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE listings SET status = ?2, updated = ?3 WHERE bundle = ?1",
            params![bundle.as_bytes(), status_text(status), to_i64(now)],
        )?;
        Ok(())
    }

    /// Lists `bundle` again after it was withdrawn: pending until its content is known.
    pub fn relist(&self, bundle: &BundleId, now: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE listings
             SET status = CASE WHEN content_name IS NULL THEN 'pending' ELSE 'listed' END,
                 updated = ?2
             WHERE bundle = ?1",
            params![bundle.as_bytes(), to_i64(now)],
        )?;
        Ok(())
    }

    /// Records what `bundle` holds, and lists it if it was pending.
    pub fn set_content(&self, bundle: &BundleId, content: &Content) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE listings
             SET content_name = ?2, content_size = ?3, content_files = ?4,
                 status = CASE status WHEN 'pending' THEN 'listed' ELSE status END
             WHERE bundle = ?1",
            params![
                bundle.as_bytes(),
                content.name,
                to_i64(content.size),
                to_i64(content.files)
            ],
        )?;
        Ok(())
    }

    /// Every pending listing's bundle and publisher, oldest first.
    pub fn pending(&self) -> Result<Vec<(BundleId, PeerId)>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT bundle, publisher FROM listings WHERE status = 'pending'
             ORDER BY created, bundle",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|row| {
            let (bundle, publisher) = row?;
            Ok((hash_of(bundle)?, hash_of(publisher)?))
        })
        .collect()
    }

    fn tags(&self, bundle: &BundleId) -> Result<Vec<String>, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT tag FROM listing_tags WHERE bundle = ?1 ORDER BY position")?;
        let rows = statement.query_map([bundle.as_bytes()], |row| row.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// `bundle`'s listing as `viewer` sees it, whatever its status. Its swarm counts are zero:
    /// the swarms aren't the store's.
    pub fn listing(
        &self,
        bundle: &BundleId,
        viewer: Option<&PeerId>,
    ) -> Result<Option<Listing>, StoreError> {
        let sql = format!(
            "SELECT {LISTING_COLUMNS} FROM listings l JOIN accounts a ON a.key = l.publisher
             WHERE l.bundle = ?2"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let mut rows = statement.query(params![
            viewer.map(|key| key.as_bytes().as_slice()),
            bundle.as_bytes()
        ])?;
        match rows.next()? {
            Some(row) => Ok(Some(self.read_listing(row)?)),
            None => Ok(None),
        }
    }

    /// A page of listings matching `filter` (SQL over `l`, with its parameters from `?2`), in
    /// `order`, and how many match in all.
    fn listing_page(
        &self,
        viewer: Option<&PeerId>,
        filter: &str,
        filter_params: &[&dyn rusqlite::ToSql],
        order: &str,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<Listing>, u64), StoreError> {
        let viewer = viewer.map(|key| key.as_bytes().as_slice());
        let mut all: Vec<&dyn rusqlite::ToSql> = vec![&viewer];
        all.extend_from_slice(filter_params);
        // The count doesn't use the viewer, `?1`, but binding it keeps the filter's numbering.
        let total: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM listings l WHERE {filter}"),
            all.as_slice(),
            |row| row.get(0),
        )?;
        let sql = format!(
            "SELECT {LISTING_COLUMNS} FROM listings l JOIN accounts a ON a.key = l.publisher
             WHERE {filter} ORDER BY {order} LIMIT {limit} OFFSET {offset}",
            offset = to_i64(offset),
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let mut rows = statement.query(all.as_slice())?;
        let mut listings = Vec::new();
        while let Some(row) = rows.next()? {
            listings.push(self.read_listing(row)?);
        }
        Ok((listings, to_u64(total)))
    }

    /// Every listing `publisher` asked for, whatever its status, most recently changed first.
    pub fn published_by(
        &self,
        publisher: &PeerId,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<Listing>, u64), StoreError> {
        let key = publisher.as_bytes().as_slice();
        self.listing_page(
            Some(publisher),
            "l.publisher = ?2",
            &[&key],
            "l.updated DESC, l.bundle",
            offset,
            limit,
        )
    }

    fn read_listing(&self, row: &Row) -> Result<Listing, StoreError> {
        let bundle = hash_of(row.get(0)?)?;
        let title: String = row.get(1)?;
        let content_name: Option<String> = row.get(9)?;
        let content = match content_name {
            Some(name) => Some(Content {
                name,
                size: to_u64(row.get(10)?),
                files: to_u64(row.get(11)?),
            }),
            None => None,
        };
        let my_vote = match row.get::<_, Option<i64>>(14)? {
            Some(1) => Vote::Up,
            Some(_) => Vote::Down,
            None => Vote::None,
        };
        Ok(Listing {
            bundle,
            title: match (title.is_empty(), &content) {
                (true, Some(content)) => content.name.clone(),
                _ => title,
            },
            description: row.get(2)?,
            category: row.get(3)?,
            tags: self.tags(&bundle)?,
            publisher: Person {
                key: hash_of(row.get(4)?)?,
                name: row.get(5)?,
            },
            status: parse_status(&row.get::<_, String>(6)?)?,
            created: to_u64(row.get(7)?),
            updated: to_u64(row.get(8)?),
            content,
            votes: VoteCounts {
                up: to_u64(row.get(12)?),
                down: to_u64(row.get(13)?),
            },
            my_vote,
            comments: to_u64(row.get(15)?),
            downloads: to_u64(row.get(16)?),
            swarm: BundleStats::default(),
        })
    }

    // Search and ranking.

    /// What the search index holds of `bundle`, if it is listed.
    pub fn indexed(&self, bundle: &BundleId) -> Result<Option<Indexed>, StoreError> {
        Ok(self
            .indexed_where("AND bundle = ?1", &[&bundle.as_bytes().as_slice()])?
            .pop())
    }

    /// What the search index holds of every listed bundle.
    pub fn all_indexed(&self) -> Result<Vec<Indexed>, StoreError> {
        self.indexed_where("", &[])
    }

    fn indexed_where(
        &self,
        filter: &str,
        filter_params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Indexed>, StoreError> {
        let sql = format!(
            "SELECT bundle, title, description, category, content_name, created FROM listings
             WHERE status = 'listed' {filter}"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let rows = statement.query_map(filter_params, |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;
        let mut indexed = Vec::new();
        for row in rows {
            let (bundle, title, description, category, content_name, created) = row?;
            let bundle = hash_of(bundle)?;
            indexed.push(Indexed {
                bundle,
                fields: ListingFields {
                    title,
                    description,
                    category,
                    tags: self.tags(&bundle)?,
                },
                content_name: content_name.unwrap_or_default(),
                created: to_u64(created),
            });
        }
        Ok(indexed)
    }

    /// Counts a download of `bundle` on `day` (days since the Unix epoch), if it has a listing.
    pub fn record_download(&self, bundle: &BundleId, day: u64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO downloads (bundle, day, count)
             SELECT ?1, ?2, 1 WHERE EXISTS (SELECT 1 FROM listings WHERE bundle = ?1)
             ON CONFLICT (bundle, day) DO UPDATE SET count = count + 1",
            params![bundle.as_bytes(), to_i64(day)],
        )?;
        Ok(())
    }

    /// Every listed bundle's downloads on each day from `since` on, by bundle.
    pub fn downloads_since(
        &self,
        since: u64,
    ) -> Result<HashMap<BundleId, Vec<(u64, u64)>>, StoreError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT d.bundle, d.day, d.count FROM downloads d
             JOIN listings l ON l.bundle = d.bundle
             WHERE d.day >= ?1 AND l.status = 'listed'",
        )?;
        let rows = statement.query_map([to_i64(since)], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut downloads: HashMap<BundleId, Vec<(u64, u64)>> = HashMap::new();
        for row in rows {
            let (bundle, day, count) = row?;
            downloads
                .entry(hash_of(bundle)?)
                .or_default()
                .push((to_u64(day), to_u64(count)));
        }
        Ok(downloads)
    }

    /// Every listed bundle's votes, for those that have any.
    pub fn vote_counts(&self) -> Result<HashMap<BundleId, VoteCounts>, StoreError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT v.bundle, sum(v.value = 1), sum(v.value = -1) FROM votes v
             JOIN listings l ON l.bundle = v.bundle
             WHERE l.status = 'listed' GROUP BY v.bundle",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut votes = HashMap::new();
        for row in rows {
            let (bundle, up, down) = row?;
            let counts = VoteCounts {
                up: to_u64(up),
                down: to_u64(down),
            };
            votes.insert(hash_of(bundle)?, counts);
        }
        Ok(votes)
    }

    // Votes and comments.

    /// Records `key`'s vote on `bundle`, replacing any earlier one.
    pub fn vote(
        &self,
        bundle: &BundleId,
        key: &PeerId,
        vote: Vote,
        now: u64,
    ) -> Result<(), StoreError> {
        let value = match vote {
            Vote::Up => 1,
            Vote::Down => -1,
            Vote::None => {
                self.conn.execute(
                    "DELETE FROM votes WHERE bundle = ?1 AND key = ?2",
                    params![bundle.as_bytes(), key.as_bytes()],
                )?;
                return Ok(());
            }
        };
        self.conn.execute(
            "INSERT INTO votes (bundle, key, value, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (bundle, key) DO UPDATE SET value = ?3, at = ?4",
            params![bundle.as_bytes(), key.as_bytes(), value, to_i64(now)],
        )?;
        Ok(())
    }

    /// Saves a comment and returns it.
    pub fn add_comment(
        &self,
        bundle: &BundleId,
        author: &PeerId,
        body: &str,
        now: u64,
    ) -> Result<Comment, StoreError> {
        self.conn.execute(
            "INSERT INTO comments (bundle, author, body, created) VALUES (?1, ?2, ?3, ?4)",
            params![bundle.as_bytes(), author.as_bytes(), body, to_i64(now)],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(self
            .comment(to_u64(id))?
            .expect("the comment was just saved"))
    }

    /// The comment `id`.
    pub fn comment(&self, id: u64) -> Result<Option<Comment>, StoreError> {
        self.comments_where("c.id = ?1", &[&to_i64(id)])
            .map(|comments| comments.into_iter().next())
    }

    /// The comments on `bundle`, oldest first.
    pub fn comments(&self, bundle: &BundleId) -> Result<Vec<Comment>, StoreError> {
        self.comments_where("c.bundle = ?1", &[&bundle.as_bytes().as_slice()])
    }

    fn comments_where(
        &self,
        filter: &str,
        filter_params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Comment>, StoreError> {
        let sql = format!(
            "SELECT c.id, c.author, a.name, c.body, c.created
             FROM comments c JOIN accounts a ON a.key = c.author
             WHERE {filter} ORDER BY c.created, c.id"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let rows = statement.query_map(filter_params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        let mut comments = Vec::new();
        for row in rows {
            let (id, author, name, body, created) = row?;
            comments.push(Comment {
                id: to_u64(id),
                author: Person {
                    key: hash_of(author)?,
                    name,
                },
                body,
                created: to_u64(created),
            });
        }
        Ok(comments)
    }

    /// Deletes the comment `id`.
    pub fn delete_comment(&self, id: u64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM comments WHERE id = ?1", [to_i64(id)])?;
        Ok(())
    }
}
