//! The engine's saved state, in SQLite (D-082): transfers, the chunks each has verified, the
//! trackers each is listed on, categories, settings and known peers.
//!
//! The schema is created and upgraded by numbered migrations, tracked in SQLite's
//! `user_version`. Every call blocks briefly; the engine owns the only connection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use tempest_protocol::message::PeerRecord;
use tempest_protocol::wire::{Decode, Encode};
use tempest_protocol::{BundleId, Hash};

/// The migrations, in order. Migration `i` takes the schema from version `i` to `i + 1`.
const MIGRATIONS: &[&str] = &[
    "
CREATE TABLE transfers (
    id BLOB PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('download', 'share')),
    link TEXT NOT NULL,
    manifest BLOB,
    name TEXT,
    save_folder TEXT NOT NULL,
    content_dir TEXT,
    wanted INTEGER NOT NULL,
    forced INTEGER NOT NULL,
    done INTEGER NOT NULL,
    error TEXT,
    added INTEGER NOT NULL,
    completed INTEGER,
    queue_position INTEGER NOT NULL,
    category TEXT,
    settings TEXT NOT NULL,
    uploaded INTEGER NOT NULL,
    downloaded INTEGER NOT NULL,
    have INTEGER NOT NULL,
    chunks INTEGER NOT NULL
) STRICT;
CREATE TABLE tags (
    transfer BLOB NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    PRIMARY KEY (transfer, tag)
) STRICT;
CREATE TABLE verified_chunks (
    transfer BLOB PRIMARY KEY NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    checkpoint BLOB NOT NULL
) STRICT;
CREATE TABLE known_peers (
    transfer BLOB NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    peer BLOB NOT NULL,
    record BLOB NOT NULL,
    PRIMARY KEY (transfer, peer)
) STRICT;
CREATE TABLE categories (
    name TEXT PRIMARY KEY NOT NULL,
    save_folder TEXT NOT NULL
) STRICT;
CREATE TABLE settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
) STRICT;
",
    "
ALTER TABLE transfers ADD COLUMN seeding_time INTEGER NOT NULL DEFAULT 0;
",
    "
CREATE TABLE publications (
    transfer BLOB NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    tracker TEXT NOT NULL,
    PRIMARY KEY (transfer, tracker)
) STRICT;
",
];

/// Why the state store failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite failed.
    #[error("state database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database was written by a newer version of Tempest.
    #[error("the state database has schema version {found}, newer than this version's {known}")]
    TooNew {
        /// The database's version.
        found: u32,
        /// The newest version this build knows.
        known: u32,
    },
    /// A row holds something this build can't read.
    #[error("state database: {0}")]
    Corrupt(String),
}

pub use tempest_api::Kind;

/// The kind named `text` in the state database.
fn parse_kind(text: &str) -> Result<Kind, StoreError> {
    match text {
        "download" => Ok(Kind::Download),
        "share" => Ok(Kind::Share),
        other => Err(StoreError::Corrupt(format!(
            "unknown transfer kind {other:?}"
        ))),
    }
}

/// A transfer as saved.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRow {
    /// The bundle ID, which identifies the transfer.
    pub id: BundleId,
    /// Download or share.
    pub kind: Kind,
    /// The link it was added from, or the link made for it.
    pub link: String,
    /// The encoded manifest, once known.
    pub manifest: Option<Vec<u8>>,
    /// A display name the user chose, instead of the bundle's.
    pub name: Option<String>,
    /// The folder a download's bundle folder is made in; a share's base folder.
    pub save_folder: PathBuf,
    /// The folder the bundle's entry paths are relative to, once known.
    pub content_dir: Option<PathBuf>,
    /// The user wants it running.
    pub wanted: bool,
    /// It runs regardless of the queue.
    pub forced: bool,
    /// It completed and then stopped sharing on its own.
    pub done: bool,
    /// Why it stopped, if it failed.
    pub error: Option<String>,
    /// When it was added, in milliseconds since the Unix epoch.
    pub added: i64,
    /// When it completed, in milliseconds since the Unix epoch; when it was added for a share.
    pub completed: Option<i64>,
    /// Its place in the queue, from 0.
    pub queue_position: u32,
    /// Its tags.
    pub tags: BTreeSet<String>,
    /// Its category.
    pub category: Option<String>,
    /// Session settings of its own, by dotted key, applied over the config's.
    pub settings: BTreeMap<String, Value>,
    /// Payload bytes sent, over every run.
    pub uploaded: u64,
    /// Payload bytes received, over every run.
    pub downloaded: u64,
    /// Chunks stored, as of the last checkpoint.
    pub have: u64,
    /// The bundle's chunk count, once the manifest is known; 0 before.
    pub chunks: u64,
    /// How long it has shared while complete, over every run, in milliseconds.
    pub seeding_time: u64,
    /// The trackers whose catalogs the user chose to list it on (D-129).
    pub publish: BTreeSet<String>,
}

impl TransferRow {
    /// Every chunk is stored and synced.
    pub fn is_complete(&self) -> bool {
        self.completed.is_some()
    }
}

/// The engine's state database.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

fn path_text(path: &Path) -> Result<&str, StoreError> {
    path.to_str()
        .ok_or_else(|| StoreError::Corrupt(format!("{} is not valid UTF-8", path.display())))
}

fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

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
        // WAL keeps writes cheap; NORMAL sync is durable across application crashes, which is
        // what resume needs. Verified chunks are only recorded after the data is synced.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    /// The schema version this build writes.
    pub fn schema_version() -> u32 {
        MIGRATIONS.len() as u32
    }

    fn migrate(&mut self) -> Result<(), StoreError> {
        let known = Self::schema_version();
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

    /// Every transfer, in queue order.
    pub fn transfers(&self) -> Result<Vec<TransferRow>, StoreError> {
        let mut statement = self.conn.prepare(
            "SELECT id, kind, link, manifest, name, save_folder, content_dir, wanted, forced, done,
                    error, added, completed, queue_position, category, settings, uploaded,
                    downloaded, have, chunks, seeding_time
             FROM transfers ORDER BY queue_position",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, String>(15)?,
                TransferRow {
                    id: Hash::new([0; 32]),
                    kind: Kind::Download,
                    link: row.get(2)?,
                    manifest: row.get(3)?,
                    name: row.get(4)?,
                    save_folder: PathBuf::new(),
                    content_dir: None,
                    wanted: row.get(7)?,
                    forced: row.get(8)?,
                    done: row.get(9)?,
                    error: row.get(10)?,
                    added: row.get(11)?,
                    completed: row.get(12)?,
                    queue_position: row.get(13)?,
                    tags: BTreeSet::new(),
                    category: row.get(14)?,
                    settings: BTreeMap::new(),
                    uploaded: row.get::<_, i64>(16)?.max(0) as u64,
                    downloaded: row.get::<_, i64>(17)?.max(0) as u64,
                    have: row.get::<_, i64>(18)?.max(0) as u64,
                    chunks: row.get::<_, i64>(19)?.max(0) as u64,
                    seeding_time: row.get::<_, i64>(20)?.max(0) as u64,
                    publish: BTreeSet::new(),
                },
            ))
        })?;
        let mut transfers = Vec::new();
        for row in rows {
            let (id, kind, save_folder, content_dir, settings, mut transfer) = row?;
            transfer.id = bundle_id(&id)?;
            transfer.kind = parse_kind(&kind)?;
            transfer.save_folder = save_folder.into();
            transfer.content_dir = content_dir.map(PathBuf::from);
            transfer.settings = serde_json::from_str(&settings)
                .map_err(|e| StoreError::Corrupt(format!("transfer settings: {e}")))?;
            transfer.tags = self.tags(&transfer.id)?;
            transfer.publish = self.publications(&transfer.id)?;
            transfers.push(transfer);
        }
        Ok(transfers)
    }

    fn tags(&self, id: &BundleId) -> Result<BTreeSet<String>, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT tag FROM tags WHERE transfer = ?1")?;
        let tags = statement
            .query_map([id.as_bytes()], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(tags)
    }

    fn publications(&self, id: &BundleId) -> Result<BTreeSet<String>, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT tracker FROM publications WHERE transfer = ?1")?;
        let trackers = statement
            .query_map([id.as_bytes()], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(trackers)
    }

    /// Saves a transfer, adding or replacing it.
    pub fn save_transfer(&mut self, transfer: &TransferRow) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        save_transfer(&tx, transfer)?;
        tx.commit()?;
        Ok(())
    }

    /// Saves several transfers at once, such as after the queue is reordered.
    pub fn save_transfers<'a>(
        &mut self,
        transfers: impl IntoIterator<Item = &'a TransferRow>,
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        for transfer in transfers {
            save_transfer(&tx, transfer)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes a transfer and everything saved for it.
    pub fn remove_transfer(&mut self, id: &BundleId) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM transfers WHERE id = ?1", [id.as_bytes()])?;
        Ok(())
    }

    /// Records a transfer's verified chunks: an encoded storage checkpoint.
    pub fn save_checkpoint(&mut self, id: &BundleId, checkpoint: &[u8]) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO verified_chunks (transfer, checkpoint) VALUES (?1, ?2)
             ON CONFLICT (transfer) DO UPDATE SET checkpoint = excluded.checkpoint",
            params![id.as_bytes(), checkpoint],
        )?;
        Ok(())
    }

    /// A transfer's last recorded checkpoint, if any.
    pub fn checkpoint(&self, id: &BundleId) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT checkpoint FROM verified_chunks WHERE transfer = ?1",
                [id.as_bytes()],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Forgets a transfer's checkpoint, so its data is checked from scratch next time.
    pub fn clear_checkpoint(&mut self, id: &BundleId) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM verified_chunks WHERE transfer = ?1",
            [id.as_bytes()],
        )?;
        Ok(())
    }

    /// Remembers peers known to have or want a transfer's bundle, replacing what was known of
    /// each.
    pub fn save_peers(&mut self, id: &BundleId, peers: &[PeerRecord]) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        {
            let mut statement = tx.prepare_cached(
                "INSERT INTO known_peers (transfer, peer, record) VALUES (?1, ?2, ?3)
                 ON CONFLICT (transfer, peer) DO UPDATE SET record = excluded.record",
            )?;
            for peer in peers {
                statement.execute(params![
                    id.as_bytes(),
                    peer.peer_id.as_bytes(),
                    peer.to_bytes()
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The peers remembered for a transfer. Records that no longer decode are skipped.
    pub fn peers(&self, id: &BundleId) -> Result<Vec<PeerRecord>, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT record FROM known_peers WHERE transfer = ?1")?;
        let records = statement
            .query_map([id.as_bytes()], |row| row.get::<_, Vec<u8>>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records
            .iter()
            .filter_map(|bytes| PeerRecord::from_bytes(bytes).ok())
            .collect())
    }

    /// Every category and its save folder.
    pub fn categories(&self) -> Result<BTreeMap<String, PathBuf>, StoreError> {
        let mut statement = self
            .conn
            .prepare("SELECT name, save_folder FROM categories")?;
        let categories = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    PathBuf::from(row.get::<_, String>(1)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(categories)
    }

    /// Adds a category or changes its save folder.
    pub fn set_category(&mut self, name: &str, save_folder: &Path) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO categories (name, save_folder) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET save_folder = excluded.save_folder",
            params![name, path_text(save_folder)?],
        )?;
        Ok(())
    }

    /// Removes a category, taking it off every transfer.
    pub fn remove_category(&mut self, name: &str) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE transfers SET category = NULL WHERE category = ?1",
            [name],
        )?;
        tx.execute("DELETE FROM categories WHERE name = ?1", [name])?;
        tx.commit()?;
        Ok(())
    }

    /// The settings changed at run time, by dotted key.
    pub fn settings(&self) -> Result<BTreeMap<String, Value>, StoreError> {
        let mut statement = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (key, text) = row?;
            let value = serde_json::from_str(&text)
                .map_err(|e| StoreError::Corrupt(format!("setting {key}: {e}")))?;
            Ok((key, value))
        })
        .collect()
    }

    /// Saves a setting changed at run time.
    pub fn set_setting(&mut self, key: &str, value: &Value) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    /// Forgets a setting changed at run time.
    pub fn remove_setting(&mut self, key: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }
}

fn bundle_id(bytes: &[u8]) -> Result<BundleId, StoreError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StoreError::Corrupt("a transfer ID is not 32 bytes".to_owned()))?;
    Ok(Hash::new(bytes))
}

fn save_transfer(tx: &Transaction<'_>, t: &TransferRow) -> Result<(), StoreError> {
    let settings = serde_json::to_string(&t.settings).expect("JSON values serialize");
    let content_dir = t.content_dir.as_deref().map(path_text).transpose()?;
    tx.execute(
        "INSERT INTO transfers (id, kind, link, manifest, name, save_folder, content_dir, wanted,
             forced, done, error, added, completed, queue_position, category, settings, uploaded,
             downloaded, have, chunks, seeding_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
             ?18, ?19, ?20, ?21)
         ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, link = excluded.link,
             manifest = excluded.manifest, name = excluded.name,
             save_folder = excluded.save_folder, content_dir = excluded.content_dir,
             wanted = excluded.wanted, forced = excluded.forced, done = excluded.done,
             error = excluded.error, added = excluded.added, completed = excluded.completed,
             queue_position = excluded.queue_position, category = excluded.category,
             settings = excluded.settings, uploaded = excluded.uploaded,
             downloaded = excluded.downloaded, have = excluded.have, chunks = excluded.chunks,
             seeding_time = excluded.seeding_time",
        params![
            t.id.as_bytes(),
            t.kind.as_str(),
            t.link,
            t.manifest,
            t.name,
            path_text(&t.save_folder)?,
            content_dir,
            t.wanted,
            t.forced,
            t.done,
            t.error,
            t.added,
            t.completed,
            t.queue_position,
            t.category,
            settings,
            to_i64(t.uploaded),
            to_i64(t.downloaded),
            to_i64(t.have),
            to_i64(t.chunks),
            to_i64(t.seeding_time),
        ],
    )?;
    tx.execute("DELETE FROM tags WHERE transfer = ?1", [t.id.as_bytes()])?;
    let mut insert = tx.prepare_cached("INSERT INTO tags (transfer, tag) VALUES (?1, ?2)")?;
    for tag in &t.tags {
        insert.execute(params![t.id.as_bytes(), tag])?;
    }
    tx.execute(
        "DELETE FROM publications WHERE transfer = ?1",
        [t.id.as_bytes()],
    )?;
    let mut insert =
        tx.prepare_cached("INSERT INTO publications (transfer, tracker) VALUES (?1, ?2)")?;
    for tracker in &t.publish {
        insert.execute(params![t.id.as_bytes(), tracker])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempest_protocol::Address;

    use super::*;

    fn row(seed: u8, position: u32) -> TransferRow {
        TransferRow {
            id: Hash::new([seed; 32]),
            kind: Kind::Download,
            link: format!("tempest:?bundle={}", Hash::new([seed; 32])),
            manifest: None,
            name: None,
            save_folder: "/downloads".into(),
            content_dir: None,
            wanted: true,
            forced: false,
            done: false,
            error: None,
            added: 1_700_000_000_000,
            completed: None,
            queue_position: position,
            tags: BTreeSet::new(),
            category: None,
            settings: BTreeMap::new(),
            uploaded: 0,
            downloaded: 0,
            have: 0,
            chunks: 0,
            seeding_time: 0,
            publish: BTreeSet::new(),
        }
    }

    #[test]
    fn transfers_are_saved_updated_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut store = Store::open(&path).unwrap();
        let mut first = row(1, 1);
        let second = TransferRow {
            kind: Kind::Share,
            manifest: Some(vec![1, 2, 3]),
            name: Some("renamed".into()),
            content_dir: Some("/data/folder".into()),
            forced: true,
            error: Some("disk full".into()),
            completed: Some(1_700_000_000_500),
            tags: BTreeSet::from(["a".to_owned(), "b".to_owned()]),
            settings: BTreeMap::from([("upload.reciprocal_slots".to_owned(), json!(6))]),
            uploaded: 10,
            downloaded: 20,
            have: 3,
            chunks: 3,
            seeding_time: 7,
            publish: BTreeSet::from(["https://tracker.example.org".to_owned()]),
            ..row(2, 0)
        };
        store.save_transfers([&first, &second]).unwrap();
        assert_eq!(store.transfers().unwrap(), [second.clone(), first.clone()]);

        first.tags.insert("x".into());
        first.wanted = false;
        first.queue_position = 2;
        store.save_transfer(&first).unwrap();
        store.save_checkpoint(&first.id, &[9, 9]).unwrap();
        store.save_checkpoint(&first.id, &[8]).unwrap();
        assert_eq!(store.checkpoint(&first.id).unwrap(), Some(vec![8]));
        drop(store);

        // Everything survives reopening.
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.transfers().unwrap(), [second.clone(), first.clone()]);
        store.remove_transfer(&first.id).unwrap();
        assert_eq!(store.transfers().unwrap(), [second]);
        assert_eq!(store.checkpoint(&first.id).unwrap(), None);
    }

    #[test]
    fn categories_settings_and_peers() {
        let mut store = Store::in_memory().unwrap();
        let mut transfer = row(1, 0);
        transfer.category = Some("music".into());
        store.save_transfer(&transfer).unwrap();
        store.set_category("music", Path::new("/music")).unwrap();
        store
            .set_category("music", Path::new("/media/music"))
            .unwrap();
        store.set_category("films", Path::new("/films")).unwrap();
        assert_eq!(
            store.categories().unwrap(),
            BTreeMap::from([
                ("films".to_owned(), PathBuf::from("/films")),
                ("music".to_owned(), PathBuf::from("/media/music")),
            ])
        );
        store.remove_category("music").unwrap();
        assert_eq!(store.categories().unwrap().len(), 1);
        assert_eq!(store.transfers().unwrap()[0].category, None);

        assert!(store.settings().unwrap().is_empty());
        store
            .set_setting("queue.max_active_uploads", &json!(2))
            .unwrap();
        store.set_setting("lan.enabled", &json!(false)).unwrap();
        store
            .set_setting("queue.max_active_uploads", &json!(3))
            .unwrap();
        store.remove_setting("lan.enabled").unwrap();
        assert_eq!(
            store.settings().unwrap(),
            BTreeMap::from([("queue.max_active_uploads".to_owned(), json!(3))])
        );

        let peer = |seed: u8, port: u16| PeerRecord {
            peer_id: Hash::new([seed; 32]),
            addresses: vec![Address::new([127, 0, 0, 1].into(), port)],
            relay: None,
        };
        store
            .save_peers(&transfer.id, &[peer(5, 1), peer(6, 2)])
            .unwrap();
        store.save_peers(&transfer.id, &[peer(5, 3)]).unwrap();
        let mut peers = store.peers(&transfer.id).unwrap();
        peers.sort_by_key(|p| p.peer_id);
        assert_eq!(peers, [peer(5, 3), peer(6, 2)]);
        store.remove_transfer(&transfer.id).unwrap();
        assert!(store.peers(&transfer.id).unwrap().is_empty());
    }

    #[test]
    fn migrations_run_once_and_newer_databases_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        Store::open(&path).unwrap();
        // Reopening applies nothing twice.
        Store::open(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        let version: u32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, Store::schema_version());
        conn.pragma_update(None, "user_version", version + 1)
            .unwrap();
        drop(conn);
        assert!(matches!(Store::open(&path), Err(StoreError::TooNew { .. })));
    }

    #[test]
    fn a_first_version_database_is_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO transfers VALUES (?1, 'download', 'link', NULL, NULL, '/downloads',
                 NULL, 1, 0, 0, NULL, 0, NULL, 0, NULL, '{}', 0, 0, 0, 0)",
            [[9u8; 32].as_slice()],
        )
        .unwrap();
        drop(conn);
        let store = Store::open(&path).unwrap();
        let transfers = store.transfers().unwrap();
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].seeding_time, 0);
    }
}
