use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use thiserror::Error;

use crate::pdf::PdfInfo;

mod jobs;
pub use jobs::*;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct MasterResume {
    pub file_name: String,
    pub size_bytes: u64,
    pub page_count: usize,
    pub extracted_text: String,
    pub uploaded_at: String,
}

/// Each entry runs once, in order; progress is tracked in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    "CREATE TABLE master_resume (
        id             INTEGER PRIMARY KEY CHECK (id = 1),
        file_name      TEXT    NOT NULL,
        size_bytes     INTEGER NOT NULL,
        page_count     INTEGER NOT NULL,
        extracted_text TEXT    NOT NULL,
        uploaded_at    TEXT    NOT NULL
    );",
    "CREATE TABLE job_searches (
        id            INTEGER PRIMARY KEY,
        name          TEXT NOT NULL,
        titles        TEXT NOT NULL,  -- JSON array
        keywords      TEXT NOT NULL,  -- JSON array
        location      TEXT,
        work_modes    TEXT NOT NULL,  -- JSON array
        posted_within TEXT NOT NULL,
        created_at    TEXT NOT NULL,
        updated_at    TEXT NOT NULL
    );
    CREATE TABLE jobs (
        id              INTEGER PRIMARY KEY,
        linkedin_id     TEXT NOT NULL UNIQUE,
        title           TEXT NOT NULL,
        company         TEXT NOT NULL,
        company_url     TEXT,
        location        TEXT NOT NULL,
        posted_on       TEXT,
        url             TEXT NOT NULL,
        description     TEXT,
        seniority       TEXT,
        employment_type TEXT,
        industries      TEXT,
        work_mode       TEXT,
        status          TEXT NOT NULL DEFAULT 'new',
        first_seen_at   TEXT NOT NULL,
        updated_at      TEXT NOT NULL
    );
    CREATE TABLE search_matches (
        search_id        INTEGER NOT NULL REFERENCES job_searches(id) ON DELETE CASCADE,
        job_id           INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
        matched_keywords TEXT NOT NULL,  -- JSON array
        matched_at       TEXT NOT NULL,
        PRIMARY KEY (search_id, job_id)
    );
    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE scan_runs (
        id          INTEGER PRIMARY KEY,
        started_at  TEXT NOT NULL,
        finished_at TEXT,
        status      TEXT NOT NULL,  -- running | ok | partial | failed | interrupted
        new_matches INTEGER NOT NULL DEFAULT 0,
        message     TEXT
    );",
];

pub struct Store {
    conn: Mutex<Connection>,
    data_dir: PathBuf,
}

impl Store {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        let conn = Connection::open(data_dir.join("db.sqlite"))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn), data_dir })
    }

    pub fn resume_pdf_path(&self) -> PathBuf {
        self.data_dir.join("resume.pdf")
    }

    pub fn get_resume(&self) -> Result<Option<MasterResume>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let resume = conn
            .query_row(
                "SELECT file_name, size_bytes, page_count, extracted_text, uploaded_at
                 FROM master_resume WHERE id = 1",
                [],
                |row| {
                    Ok(MasterResume {
                        file_name: row.get(0)?,
                        size_bytes: row.get::<_, i64>(1)? as u64,
                        page_count: row.get::<_, i64>(2)? as usize,
                        extracted_text: row.get(3)?,
                        uploaded_at: row.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(resume)
    }

    /// Stores `bytes` as the master resume, replacing any existing one.
    pub fn save_resume(
        &self,
        file_name: &str,
        bytes: &[u8],
        info: &PdfInfo,
    ) -> Result<MasterResume, StoreError> {
        let conn = self.conn.lock().unwrap();

        // Write to a temp file first so a crash never leaves a half-written PDF.
        let tmp = self.data_dir.join("resume.pdf.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, self.resume_pdf_path())?;

        let resume = MasterResume {
            file_name: file_name.to_string(),
            size_bytes: bytes.len() as u64,
            page_count: info.page_count,
            extracted_text: info.text.clone(),
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        };
        conn.execute(
            "INSERT INTO master_resume (id, file_name, size_bytes, page_count, extracted_text, uploaded_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
                file_name = excluded.file_name,
                size_bytes = excluded.size_bytes,
                page_count = excluded.page_count,
                extracted_text = excluded.extracted_text,
                uploaded_at = excluded.uploaded_at",
            params![
                resume.file_name,
                resume.size_bytes as i64,
                resume.page_count as i64,
                resume.extracted_text,
                resume.uploaded_at,
            ],
        )?;
        Ok(resume)
    }

    /// Removes the master resume. Returns `false` if there was none.
    pub fn delete_resume(&self) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM master_resume WHERE id = 1", [])?;
        match fs::remove_file(self.resume_pdf_path()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(deleted > 0)
    }
}

/// A fresh, unique temp directory for a test database. A counter makes it
/// unique even when parallel tests start within the same clock tick.
#[cfg(test)]
pub(crate) fn test_dir(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()))
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let applied = conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))? as usize;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied) {
        conn.execute_batch(sql)?;
        conn.pragma_update(None, "user_version", (index + 1) as i64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (Store, PathBuf) {
        let dir = test_dir("tailor-store-test");
        (Store::open(&dir).unwrap(), dir)
    }

    #[test]
    fn save_replace_and_delete() {
        let (store, dir) = temp_store();
        assert!(store.get_resume().unwrap().is_none());

        let info = PdfInfo { page_count: 1, text: "first".into() };
        store.save_resume("a.pdf", b"%PDF-1", &info).unwrap();
        let info = PdfInfo { page_count: 2, text: "second".into() };
        store.save_resume("b.pdf", b"%PDF-22", &info).unwrap();

        let resume = store.get_resume().unwrap().unwrap();
        assert_eq!(resume.file_name, "b.pdf");
        assert_eq!(resume.page_count, 2);
        assert_eq!(fs::read(store.resume_pdf_path()).unwrap(), b"%PDF-22");

        assert!(store.delete_resume().unwrap());
        assert!(store.get_resume().unwrap().is_none());
        assert!(!store.resume_pdf_path().exists());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reopening_keeps_data() {
        let (store, dir) = temp_store();
        let info = PdfInfo { page_count: 1, text: String::new() };
        store.save_resume("a.pdf", b"%PDF-1", &info).unwrap();
        drop(store);

        let store = Store::open(&dir).unwrap();
        assert_eq!(store.get_resume().unwrap().unwrap().file_name, "a.pdf");
        fs::remove_dir_all(dir).unwrap();
    }
}
