//! The catalog's saved state, in SQLite: accounts, sessions, invites, listings, votes,
//! comments, and downloads by day.
//!
//! The store only reads and writes; [`super::Catalog`] decides what is allowed. The schema is
//! created and upgraded by numbered migrations, tracked in SQLite's `user_version`, as the
//! engine's is. Times are seconds since the Unix epoch.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, Row, params};
use tempest_protocol::tracker::BundleStats;
use tempest_protocol::{BundleId, Hash, PeerId};
use tempest_tracker_api::account::{Count, ProfileComment, ProfileStats};
use tempest_tracker_api::listing::{
    Comment, Content, Listing, ListingRef, ListingStatus, Person, Vote, VoteCounts,
};
use tempest_tracker_api::text::name_key;

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
    "
ALTER TABLE listings ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
ALTER TABLE accounts ADD COLUMN banned INTEGER NOT NULL DEFAULT 0;
CREATE TABLE reports (
    id INTEGER PRIMARY KEY,
    bundle BLOB NOT NULL REFERENCES listings (bundle) ON DELETE CASCADE,
    reporter BLOB NOT NULL REFERENCES accounts (key) ON DELETE CASCADE,
    reason TEXT NOT NULL,
    created INTEGER NOT NULL,
    UNIQUE (bundle, reporter)
) STRICT;
",
    // Unpublished listings were stored as 'withdrawn'. SQLite can't change a CHECK constraint,
    // so the table is rebuilt; `migrate` turns foreign keys off around it.
    "
CREATE TABLE listings_new (
    bundle BLOB PRIMARY KEY NOT NULL,
    publisher BLOB NOT NULL REFERENCES accounts (key),
    status TEXT NOT NULL CHECK (status IN ('pending', 'listed', 'unpublished')),
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    category TEXT,
    content_name TEXT,
    content_size INTEGER,
    content_files INTEGER,
    created INTEGER NOT NULL,
    updated INTEGER NOT NULL,
    hidden INTEGER NOT NULL DEFAULT 0
) STRICT;
INSERT INTO listings_new (bundle, publisher, status, title, description, category,
    content_name, content_size, content_files, created, updated, hidden)
SELECT bundle, publisher, CASE status WHEN 'withdrawn' THEN 'unpublished' ELSE status END,
    title, description, category, content_name, content_size, content_files, created, updated,
    hidden
FROM listings;
DROP TABLE listings;
ALTER TABLE listings_new RENAME TO listings;
CREATE INDEX listings_by_publisher ON listings (publisher, updated);
CREATE INDEX listings_by_status ON listings (status, created);
",
    // Account names are unique by their `name_key` (D-261). The key can't be computed in SQL, so
    // `migrate` fills it in between this migration and the next, which enforces it.
    "ALTER TABLE accounts ADD COLUMN name_key TEXT;",
    "CREATE UNIQUE INDEX accounts_by_name_key ON accounts (name_key) WHERE name_key IS NOT NULL;",
];

/// The migration that enforces unique name keys, which the keys are filled in before.
const ENFORCE_NAME_KEYS: usize = 5;

/// SQL over `listings l` that holds when the operator has taken the listing down: hidden, or
/// its publisher banned.
const BLOCKED: &str = "(l.hidden = 1
    OR EXISTS (SELECT 1 FROM accounts b WHERE b.key = l.publisher AND b.banned = 1))";

/// A report of a listing, as the operator reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Its identifier.
    pub id: u64,
    /// The listing reported.
    pub bundle: BundleId,
    /// The listing's publisher.
    pub publisher: PeerId,
    /// Whether the operator has hidden the listing, or banned its publisher.
    pub blocked: bool,
    /// Who reported it.
    pub reporter: PeerId,
    /// Why.
    pub reason: String,
    /// When, in seconds since the Unix epoch.
    pub created: u64,
}

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

/// Gives every account name its key, for a database from before they were unique. Where
/// several accounts have names with one key, the earliest to join keeps its name and the others
/// show their keys instead.
fn fill_name_keys(conn: &Connection) -> Result<(), StoreError> {
    let mut named =
        conn.prepare("SELECT key, name FROM accounts WHERE name IS NOT NULL ORDER BY joined, key")?;
    let rows = named
        .query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut taken = HashSet::new();
    for (key, name) in rows {
        match name_key(&name).filter(|name_key| taken.insert(name_key.clone())) {
            Some(name_key) => conn.execute(
                "UPDATE accounts SET name_key = ?2 WHERE key = ?1",
                params![key, name_key],
            )?,
            None => {
                tracing::warn!("cleared the name {name:?} of an account: another has one like it");
                conn.execute("UPDATE accounts SET name = NULL WHERE key = ?1", [key])?
            }
        };
    }
    Ok(())
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
        ListingStatus::Unpublished => "unpublished",
    }
}

fn parse_status(text: &str) -> Result<ListingStatus, StoreError> {
    match text {
        "pending" => Ok(ListingStatus::Pending),
        "listed" => Ok(ListingStatus::Listed),
        "unpublished" => Ok(ListingStatus::Unpublished),
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
        // The file holds sessions and invite codes; SQLite gives its WAL files the same mode.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(path);
        }
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
        // The operator's commands write while a tracker runs on the same file.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// Brings the schema up to date. Foreign keys are off meanwhile, so a migration can rebuild
    /// a table that others reference; SQLite ignores the setting inside a transaction.
    fn migrate(&mut self) -> Result<(), StoreError> {
        self.conn.pragma_update(None, "foreign_keys", "OFF")?;
        let migrated = self.run_migrations();
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        migrated
    }

    fn run_migrations(&mut self) -> Result<(), StoreError> {
        let known = MIGRATIONS.len() as u32;
        let tx = self.conn.transaction()?;
        let found: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if found > known {
            return Err(StoreError::TooNew { found, known });
        }
        for (index, migration) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            if index == ENFORCE_NAME_KEYS {
                fill_name_keys(&tx)?;
            }
            tx.execute_batch(migration)?;
        }
        tx.pragma_update(None, "user_version", known)?;
        let broken: Option<String> = tx
            .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
            .optional()?;
        if let Some(table) = broken {
            return Err(StoreError::Corrupt(format!(
                "a migration left a broken reference in {table}"
            )));
        }
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

    /// Sets the name `key`'s account shows, with its [`name_key`], or clears it. Returns `false`
    /// and changes nothing if another account has a name with that key.
    pub fn set_name(&self, key: &PeerId, name: Option<(&str, &str)>) -> Result<bool, StoreError> {
        let (name, name_key) = name.unzip();
        match self.conn.execute(
            "UPDATE accounts SET name = ?2, name_key = ?3 WHERE key = ?1",
            params![key.as_bytes(), name, name_key],
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
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

    /// Lists `bundle` again after it was unpublished: pending until its content is known.
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

    /// Deletes the pending listings made or last edited before `cutoff`, and returns how many.
    pub fn delete_pending_before(&self, cutoff: u64) -> Result<usize, StoreError> {
        Ok(self.conn.execute(
            "DELETE FROM listings WHERE status = 'pending' AND updated < ?1",
            [to_i64(cutoff)],
        )?)
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

    /// A page of the listings of `publisher` that show: listed and not taken down, newest first.
    pub fn shown_from(
        &self,
        publisher: &PeerId,
        offset: u64,
        limit: u32,
    ) -> Result<(Vec<Listing>, u64), StoreError> {
        let key = publisher.as_bytes().as_slice();
        self.listing_page(
            None,
            &format!("l.publisher = ?2 AND l.status = 'listed' AND NOT {BLOCKED}"),
            &[&key],
            "l.created DESC, l.bundle",
            offset,
            limit,
        )
    }

    /// What `key` has done over its listings that show.
    pub fn profile_stats(&self, key: &PeerId) -> Result<ProfileStats, StoreError> {
        let key = key.as_bytes().as_slice();
        let shown = format!("l.publisher = ?1 AND l.status = 'listed' AND NOT {BLOCKED}");
        let count = |sql: String| -> Result<u64, StoreError> {
            Ok(to_u64(self.conn.query_row(&sql, [key], |row| row.get(0))?))
        };
        let (up, down): (i64, i64) = self.conn.query_row(
            &format!(
                "SELECT coalesce(sum(v.value = 1), 0), coalesce(sum(v.value = -1), 0)
                 FROM votes v JOIN listings l ON l.bundle = v.bundle WHERE {shown}"
            ),
            [key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(ProfileStats {
            listings: count(format!("SELECT count(*) FROM listings l WHERE {shown}"))?,
            downloads: count(format!(
                "SELECT coalesce(sum(d.count), 0) FROM downloads d
                 JOIN listings l ON l.bundle = d.bundle WHERE {shown}"
            ))?,
            votes_up: to_u64(up),
            votes_down: to_u64(down),
            comments: count(format!(
                "SELECT count(*) FROM comments c JOIN listings l ON l.bundle = c.bundle
                 WHERE c.author = ?1 AND l.status = 'listed' AND NOT {BLOCKED}"
            ))?,
        })
    }

    /// The categories and the tags (at most `max_tags`) of the listings of `key` that show, each
    /// with how many listings, the most first.
    pub fn profile_counts(
        &self,
        key: &PeerId,
        max_tags: u32,
    ) -> Result<(Vec<Count>, Vec<Count>), StoreError> {
        let key = key.as_bytes().as_slice();
        let counts = |sql: String| -> Result<Vec<Count>, StoreError> {
            let mut statement = self.conn.prepare_cached(&sql)?;
            let rows = statement.query_map([key], |row| {
                Ok(Count {
                    name: row.get(0)?,
                    count: to_u64(row.get(1)?),
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        };
        let shown = format!("l.publisher = ?1 AND l.status = 'listed' AND NOT {BLOCKED}");
        let categories = counts(format!(
            "SELECT l.category, count(*) AS n FROM listings l
             WHERE {shown} AND l.category IS NOT NULL GROUP BY l.category ORDER BY n DESC, l.category"
        ))?;
        let tags = counts(format!(
            "SELECT t.tag, count(*) AS n FROM listing_tags t JOIN listings l ON l.bundle = t.bundle
             WHERE {shown} GROUP BY t.tag ORDER BY n DESC, t.tag LIMIT {max_tags}"
        ))?;
        Ok((categories, tags))
    }

    /// Up to `limit` of the comments `author` wrote on listings that show, newest first, before
    /// the comment `before` if given.
    pub fn comments_by(
        &self,
        author: &PeerId,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<ProfileComment>, StoreError> {
        let sql = format!(
            "SELECT c.id, c.body, c.created, l.bundle, l.title, l.content_name
             FROM comments c JOIN listings l ON l.bundle = c.bundle
             WHERE c.author = ?1 AND c.id < ?2 AND l.status = 'listed' AND NOT {BLOCKED}
             ORDER BY c.id DESC LIMIT ?3"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let rows = statement.query_map(
            params![author.as_bytes(), before.map_or(i64::MAX, to_i64), limit],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )?;
        let name = self.name_of(author)?;
        let mut comments = Vec::new();
        for row in rows {
            let (id, body, created, bundle, title, content_name) = row?;
            comments.push(ProfileComment {
                comment: Comment {
                    id: to_u64(id),
                    author: Person {
                        key: *author,
                        name: name.clone(),
                    },
                    body,
                    created: to_u64(created),
                },
                listing: ListingRef {
                    bundle: hash_of(bundle)?,
                    title: match title.is_empty() {
                        true => content_name.unwrap_or_default(),
                        false => title,
                    },
                },
            });
        }
        Ok(comments)
    }

    fn name_of(&self, key: &PeerId) -> Result<Option<String>, StoreError> {
        Ok(self.account(key)?.and_then(|(name, _)| name))
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
            .indexed_where("AND l.bundle = ?1", &[&bundle.as_bytes().as_slice()])?
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
            "SELECT l.bundle, l.title, l.description, l.category, l.content_name, l.created
             FROM listings l WHERE l.status = 'listed' AND NOT {BLOCKED} {filter}"
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

    // The operator's tools.

    /// Hides or shows `bundle`'s listing. Returns whether it has one.
    pub fn set_hidden(&self, bundle: &BundleId, hidden: bool) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE listings SET hidden = ?2 WHERE bundle = ?1",
            params![bundle.as_bytes(), hidden],
        )?;
        Ok(changed == 1)
    }

    /// Bans or unbans `key`. Returns whether it has an account.
    pub fn set_banned(&self, key: &PeerId, banned: bool) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "UPDATE accounts SET banned = ?2 WHERE key = ?1",
            params![key.as_bytes(), banned],
        )?;
        Ok(changed == 1)
    }

    /// Whether `key` is banned.
    pub fn is_banned(&self, key: &PeerId) -> Result<bool, StoreError> {
        let banned: Option<bool> = self
            .conn
            .query_row(
                "SELECT banned FROM accounts WHERE key = ?1",
                [key.as_bytes()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(banned.unwrap_or(false))
    }

    /// Whether `bundle`'s listing is hidden, or its publisher banned.
    pub fn is_blocked(&self, bundle: &BundleId) -> Result<bool, StoreError> {
        let sql = format!("SELECT {BLOCKED} FROM listings l WHERE l.bundle = ?1");
        let blocked: Option<bool> = self
            .conn
            .query_row(&sql, [bundle.as_bytes()], |row| row.get(0))
            .optional()?;
        Ok(blocked.unwrap_or(false))
    }

    /// The bundles whose listings are hidden, or whose publishers are banned.
    pub fn blocked(&self) -> Result<HashSet<BundleId>, StoreError> {
        let sql = format!("SELECT l.bundle FROM listings l WHERE {BLOCKED}");
        let mut statement = self.conn.prepare_cached(&sql)?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|row| hash_of(row?)).collect()
    }

    /// The bundles `key` published.
    pub fn bundles_of(&self, key: &PeerId) -> Result<Vec<BundleId>, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT bundle FROM listings WHERE publisher = ?1")?;
        let rows = statement.query_map([key.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|row| hash_of(row?)).collect()
    }

    /// Records `reporter`'s report of `bundle`, replacing their earlier one.
    pub fn add_report(
        &self,
        bundle: &BundleId,
        reporter: &PeerId,
        reason: &str,
        now: u64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO reports (bundle, reporter, reason, created) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (bundle, reporter) DO UPDATE SET reason = ?3, created = ?4",
            params![bundle.as_bytes(), reporter.as_bytes(), reason, to_i64(now)],
        )?;
        Ok(())
    }

    /// The reports, newest first, of `bundle` only if given.
    pub fn reports(&self, bundle: Option<&BundleId>) -> Result<Vec<Report>, StoreError> {
        let sql = format!(
            "SELECT r.id, r.bundle, l.publisher, {BLOCKED}, r.reporter, r.reason, r.created
             FROM reports r JOIN listings l ON l.bundle = r.bundle
             WHERE ?1 IS NULL OR r.bundle = ?1 ORDER BY r.created DESC, r.id DESC"
        );
        let mut statement = self.conn.prepare_cached(&sql)?;
        let rows = statement.query_map([bundle.map(|b| b.as_bytes().as_slice())], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;
        let mut reports = Vec::new();
        for row in rows {
            let (id, bundle, publisher, blocked, reporter, reason, created) = row?;
            reports.push(Report {
                id: to_u64(id),
                bundle: hash_of(bundle)?,
                publisher: hash_of(publisher)?,
                blocked,
                reporter: hash_of(reporter)?,
                reason,
                created: to_u64(created),
            });
        }
        Ok(reports)
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

    /// Up to `limit` of the comments on `bundle` after the comment `after`, oldest first.
    /// Comment IDs grow, so they order comments by when they were written.
    pub fn comments(
        &self,
        bundle: &BundleId,
        after: u64,
        limit: u32,
    ) -> Result<Vec<Comment>, StoreError> {
        self.comments_where(
            "c.bundle = ?1 AND c.id > ?2 ORDER BY c.id LIMIT ?3",
            &[&bundle.as_bytes().as_slice(), &to_i64(after), &limit],
        )
    }

    /// The comments matching `filter`, an SQL condition on `comments c` that may end with an
    /// order and a limit.
    fn comments_where(
        &self,
        filter: &str,
        filter_params: &[&dyn rusqlite::ToSql],
    ) -> Result<Vec<Comment>, StoreError> {
        let sql = format!(
            "SELECT c.id, c.author, a.name, c.body, c.created
             FROM comments c JOIN accounts a ON a.key = c.author
             WHERE {filter}"
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before migration 3 keeps its listings, and the rows that reference
    /// them, when its unpublished listings are renamed.
    #[test]
    fn migrating_renames_withdrawn_listings_and_keeps_their_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        for migration in &MIGRATIONS[..3] {
            conn.execute_batch(migration).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        let (key, bundle) = ([1u8; 32].to_vec(), [2u8; 32].to_vec());
        conn.execute(
            "INSERT INTO accounts (key, joined) VALUES (?1, 0)",
            params![key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO listings (bundle, publisher, status, created, updated, hidden)
             VALUES (?1, ?2, 'withdrawn', 0, 0, 1)",
            params![bundle, key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO votes (bundle, key, value, at) VALUES (?1, ?2, 1, 0)",
            params![bundle, key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO listing_tags (bundle, position, tag) VALUES (?1, 0, 'csv')",
            params![bundle],
        )
        .unwrap();

        let store = Store::init(conn).unwrap();
        let (status, hidden): (String, i64) = store
            .conn
            .query_row("SELECT status, hidden FROM listings", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!((status.as_str(), hidden), ("unpublished", 1));
        let count = |table: &str| -> i64 {
            store
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        assert_eq!((count("votes"), count("listing_tags")), (1, 1));
        // Foreign keys are back on: a listing's rows still go with it.
        store.conn.execute("DELETE FROM listings", []).unwrap();
        assert_eq!((count("votes"), count("listing_tags")), (0, 0));
    }

    /// A database from before names were unique keeps the earliest of several like names and
    /// clears the rest, and then refuses new duplicates.
    #[test]
    fn migrating_makes_names_unique_by_keeping_the_earliest() {
        let conn = Connection::open_in_memory().unwrap();
        for migration in &MIGRATIONS[..4] {
            conn.execute_batch(migration).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        for (n, name, joined) in [
            (1u8, Some("Zed"), 30),
            (2, Some("zed"), 10),
            (3, Some("z-e-d"), 20),
            (4, Some("★"), 5),
            (5, None, 1),
            (6, Some("Amy"), 2),
        ] {
            conn.execute(
                "INSERT INTO accounts (key, name, joined) VALUES (?1, ?2, ?3)",
                params![[n; 32].to_vec(), name, joined],
            )
            .unwrap();
        }
        let store = Store::init(conn).unwrap();
        let name = |n: u8| store.account(&PeerId::new([n; 32])).unwrap().unwrap().0;
        assert_eq!(name(2).as_deref(), Some("zed"));
        assert_eq!(
            [name(1), name(3), name(4), name(5)],
            [None, None, None, None]
        );
        assert_eq!(name(6).as_deref(), Some("Amy"));
        assert!(
            !store
                .set_name(&PeerId::new([1; 32]), Some(("ZED", "zed")))
                .unwrap()
        );
    }
}
