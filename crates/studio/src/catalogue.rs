//! The SQLite catalogue: what is in the store, what was imported, what ran, and
//! the history of every subject. Schema version 1. No secret is ever written here:
//! a job records the names of the environment variables it got, not their values.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StudioError};
use crate::store::{hash_file, ObjectInfo};

/// The catalogue schema this binary writes.
pub const SCHEMA_VERSION: i64 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE schema_meta (version INTEGER NOT NULL);
CREATE TABLE objects (
    hash TEXT PRIMARY KEY,
    size INTEGER NOT NULL,
    kind TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE assets (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    source_path TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    hash TEXT NOT NULL,
    copied INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE TABLE subjects (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    parent_subject INTEGER REFERENCES subjects(id),
    head_checkpoint INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    subject INTEGER NOT NULL REFERENCES subjects(id),
    command TEXT NOT NULL,
    args TEXT NOT NULL,
    inputs TEXT NOT NULL,
    class TEXT NOT NULL,
    priority INTEGER NOT NULL,
    status TEXT NOT NULL,
    after TEXT NOT NULL,
    cache_key TEXT NOT NULL,
    from_checkpoint INTEGER,
    exit_code INTEGER,
    result TEXT,
    error TEXT,
    env_names TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER
);
CREATE INDEX jobs_by_status ON jobs(status, class, priority, id);
CREATE INDEX jobs_by_cache ON jobs(cache_key, status);
CREATE TABLE job_events (
    job INTEGER NOT NULL REFERENCES jobs(id),
    seq INTEGER NOT NULL,
    ts INTEGER NOT NULL,
    event TEXT NOT NULL,
    PRIMARY KEY (job, seq)
);
CREATE TABLE checkpoints (
    id INTEGER PRIMARY KEY,
    subject INTEGER NOT NULL REFERENCES subjects(id),
    parent INTEGER REFERENCES checkpoints(id),
    job INTEGER REFERENCES jobs(id),
    label TEXT NOT NULL,
    outputs TEXT NOT NULL,
    cached INTEGER NOT NULL,
    note TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX checkpoints_by_subject ON checkpoints(subject, id);
";

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The kinds of imported input.
pub const ASSET_KINDS: [&str; 8] = [
    "video",
    "edit_dna",
    "understanding",
    "otio",
    "chart_spec",
    "cube",
    "jsonl",
    "other",
];

/// An imported input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub source_path: String,
    pub size: u64,
    pub mtime: i64,
    pub hash: String,
    /// The bytes are also in the store (`assets.import` with `copy`).
    pub copied: bool,
    pub created_at: i64,
}

/// The catalogue, open.
pub struct Catalogue {
    pub(crate) db: Connection,
}

impl Catalogue {
    /// Open the catalogue at `path`, making schema version 1 in an empty file. A
    /// catalogue from a newer `reelctl` is refused, never changed.
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let catalogue = Self { db };
        catalogue.migrate()?;
        Ok(catalogue)
    }

    /// An in-memory catalogue, for tests.
    pub fn in_memory() -> Result<Self> {
        let db = Connection::open_in_memory()?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        let catalogue = Self { db };
        catalogue.migrate()?;
        Ok(catalogue)
    }

    fn migrate(&self) -> Result<()> {
        let has_meta: bool = self.db.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'schema_meta'",
            [],
            |row| row.get(0),
        )?;
        if !has_meta {
            self.db.execute_batch(&format!(
                "BEGIN; {SCHEMA_V1} INSERT INTO schema_meta (version) VALUES ({SCHEMA_VERSION}); COMMIT;"
            ))?;
            return Ok(());
        }
        let version = self.version()?;
        if version > SCHEMA_VERSION {
            return Err(StudioError::invalid(format!(
                "the catalogue is schema version {version}, newer than this reelctl's {SCHEMA_VERSION}; use a newer reelctl"
            )));
        }
        // Version 1 is the first; later versions add numbered steps here.
        Ok(())
    }

    pub fn version(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("SELECT version FROM schema_meta", [], |row| row.get(0))?)
    }

    /// Record a stored object (once).
    pub fn record_object(&self, info: &ObjectInfo) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO objects (hash, size, kind, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![info.hash, info.size as i64, info.kind, now_ms()],
        )?;
        Ok(())
    }

    pub fn object(&self, hash: &str) -> Result<Option<ObjectInfo>> {
        Ok(self
            .db
            .query_row(
                "SELECT hash, size, kind FROM objects WHERE hash = ?1",
                [hash],
                |row| {
                    Ok(ObjectInfo {
                        hash: row.get(0)?,
                        size: row.get::<_, i64>(1)? as u64,
                        kind: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Import the file at the absolute `path` by reference: its path, size, mtime
    /// and hash are recorded, the bytes stay where they are. `copy` (the caller
    /// has also stored the file) is recorded. The same file imported twice is the
    /// same asset.
    pub fn import_asset(&self, kind: &str, name: &str, path: &Path, copied: bool) -> Result<Asset> {
        if !ASSET_KINDS.contains(&kind) {
            return Err(StudioError::invalid(format!(
                "asset kind \"{kind}\" is not one of {}",
                ASSET_KINDS.join(", ")
            )));
        }
        if !path.is_absolute() {
            return Err(StudioError::invalid(format!(
                "{} is not an absolute path",
                path.display()
            )));
        }
        let meta = std::fs::metadata(path).map_err(|error| StudioError::io(path, error))?;
        if !meta.is_file() {
            return Err(StudioError::invalid(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let (hash, size) = hash_file(path)?;
        let source = path.to_string_lossy().into_owned();
        let existing = self
            .db
            .query_row(
                "SELECT id FROM assets WHERE source_path = ?1 AND hash = ?2 AND kind = ?3",
                params![source, hash, kind],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => {
                if copied {
                    self.db
                        .execute("UPDATE assets SET copied = 1 WHERE id = ?1", [id])?;
                }
                id
            }
            None => {
                self.db.execute(
                    "INSERT INTO assets (kind, name, source_path, size, mtime, hash, copied, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![kind, name, source, size as i64, mtime, hash, copied, now_ms()],
                )?;
                self.db.last_insert_rowid()
            }
        };
        self.asset(id)?
            .ok_or_else(|| StudioError::corrupt("an asset vanished as it was written"))
    }

    pub fn asset(&self, id: i64) -> Result<Option<Asset>> {
        Ok(self
            .db
            .query_row(
                "SELECT id, kind, name, source_path, size, mtime, hash, copied, created_at FROM assets WHERE id = ?1",
                [id],
                asset_row,
            )
            .optional()?)
    }

    /// Imported assets, newest first, of one kind or all.
    pub fn assets(&self, kind: Option<&str>) -> Result<Vec<Asset>> {
        let mut statement = self.db.prepare(
            "SELECT id, kind, name, source_path, size, mtime, hash, copied, created_at FROM assets
             WHERE ?1 IS NULL OR kind = ?1 ORDER BY id DESC",
        )?;
        let rows = statement.query_map([kind], asset_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Whether an asset's file is still the one that was imported (same size and
    /// hash; the mtime alone is not trusted).
    pub fn asset_unchanged(&self, asset: &Asset) -> Result<bool> {
        let path = Path::new(&asset.source_path);
        match std::fs::metadata(path) {
            Ok(meta) if meta.len() == asset.size => Ok(hash_file(path)?.0 == asset.hash),
            _ => Ok(false),
        }
    }
}

fn asset_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
    Ok(Asset {
        id: row.get(0)?,
        kind: row.get(1)?,
        name: row.get(2)?,
        source_path: row.get(3)?,
        size: row.get::<_, i64>(4)? as u64,
        mtime: row.get(5)?,
        hash: row.get(6)?,
        copied: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-cat-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_empty_file_gets_schema_version_1_and_opens_again() {
        let dir = dir("open");
        let path = dir.join("studio.db");
        let catalogue = Catalogue::open(&path).unwrap();
        assert_eq!(catalogue.version().unwrap(), 1);
        drop(catalogue);
        assert_eq!(Catalogue::open(&path).unwrap().version().unwrap(), 1);
    }

    #[test]
    fn a_newer_catalogue_is_refused_and_left_alone() {
        let dir = dir("newer");
        let path = dir.join("studio.db");
        {
            let catalogue = Catalogue::open(&path).unwrap();
            catalogue
                .db
                .execute("UPDATE schema_meta SET version = 9", [])
                .unwrap();
        }
        let error = Catalogue::open(&path).err().unwrap().to_string();
        assert!(error.contains("schema version 9"), "{error}");
        let still: i64 = Connection::open(&path)
            .unwrap()
            .query_row("SELECT version FROM schema_meta", [], |row| row.get(0))
            .unwrap();
        assert_eq!(still, 9);
    }

    #[test]
    fn an_asset_is_imported_by_reference_once() {
        let dir = dir("asset");
        let catalogue = Catalogue::in_memory().unwrap();
        let file = dir.join("take.mp4");
        fs::write(&file, b"not really a video").unwrap();
        let asset = catalogue
            .import_asset("video", "take", &file, false)
            .unwrap();
        assert_eq!(asset.size, 18);
        assert_eq!(asset.source_path, file.to_string_lossy());
        assert!(!asset.copied);
        assert_eq!(
            catalogue
                .import_asset("video", "take", &file, true)
                .unwrap()
                .id,
            asset.id
        );
        assert!(catalogue.asset(asset.id).unwrap().unwrap().copied);
        assert_eq!(catalogue.assets(Some("video")).unwrap().len(), 1);
        assert!(catalogue.assets(Some("otio")).unwrap().is_empty());
        assert!(catalogue.asset_unchanged(&asset).unwrap());
        fs::write(&file, b"not really a vide0").unwrap();
        assert!(!catalogue.asset_unchanged(&asset).unwrap());
    }

    #[test]
    fn bad_imports_are_refused() {
        let catalogue = Catalogue::in_memory().unwrap();
        assert!(catalogue
            .import_asset("video", "x", Path::new("relative.mp4"), false)
            .is_err());
        assert!(catalogue
            .import_asset("movie", "x", Path::new("/tmp"), false)
            .is_err());
        assert!(catalogue
            .import_asset("video", "x", &std::env::temp_dir(), false)
            .is_err());
    }

    #[test]
    fn objects_are_recorded_once() {
        let catalogue = Catalogue::in_memory().unwrap();
        let info = ObjectInfo {
            hash: "a".repeat(64),
            size: 3,
            kind: "file".to_string(),
        };
        catalogue.record_object(&info).unwrap();
        catalogue.record_object(&info).unwrap();
        assert_eq!(catalogue.object(&info.hash).unwrap(), Some(info));
        assert_eq!(catalogue.object(&"b".repeat(64)).unwrap(), None);
    }

    #[test]
    fn many_writers_share_one_file() {
        let dir = dir("writers");
        let path = dir.join("studio.db");
        Catalogue::open(&path).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|k| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let catalogue = Catalogue::open(&path).unwrap();
                    for n in 0..25 {
                        let info = ObjectInfo {
                            hash: format!("{:064x}", k * 1000 + n),
                            size: 1,
                            kind: "file".to_string(),
                        };
                        catalogue.record_object(&info).unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let count: i64 = Catalogue::open(&path)
            .unwrap()
            .db
            .query_row("SELECT count(*) FROM objects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 200);
    }
}
