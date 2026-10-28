use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::{
    errors::DbError,
    types::{
        JobCard, JobDetail, JobListing, JobScore, JobSearch, JobStatus, JobToScore, MasterResume,
        PdfInfo, PostedWithin, ProfileRecord, Requirement, ResumeProfile, ScanRun, SearchCriteria,
        Site, Warning, WorkMode,
    },
};
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    data_dir: Arc<PathBuf>,
}

pub(crate) struct KnownJob {
    pub(crate) id: i64,
    pub(crate) description: Option<String>,
}

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
        titles        TEXT NOT NULL,
        keywords      TEXT NOT NULL,
        location      TEXT,
        work_modes    TEXT NOT NULL,
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
        matched_keywords TEXT NOT NULL,
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
        status      TEXT NOT NULL,
        new_matches INTEGER NOT NULL DEFAULT 0,
        message     TEXT
    );",
    "ALTER TABLE jobs ADD COLUMN tailored_at TEXT;",
    "CREATE TABLE resume_profiles (
        id                 INTEGER PRIMARY KEY,
        resume_uploaded_at TEXT NOT NULL,
        status             TEXT NOT NULL,
        model              TEXT NOT NULL,
        data               TEXT,
        warnings           TEXT NOT NULL DEFAULT '[]',
        error              TEXT,
        created_at         TEXT NOT NULL,
        updated_at         TEXT NOT NULL,
        confirmed_at       TEXT
    );",
    "ALTER TABLE job_searches ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1;",
    "CREATE TABLE job_scores (
        job_id       INTEGER PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
        profile_id   INTEGER NOT NULL,
        status       TEXT NOT NULL,
        score        INTEGER,
        summary      TEXT,
        requirements TEXT NOT NULL DEFAULT '[]',
        model        TEXT NOT NULL,
        error        TEXT,
        updated_at   TEXT NOT NULL
    );",
    "CREATE TABLE companies (
        url        TEXT PRIMARY KEY,
        website    TEXT,
        fetched_at TEXT NOT NULL
    );",
    "ALTER TABLE jobs RENAME COLUMN linkedin_id TO source_id;
     ALTER TABLE jobs ADD COLUMN source TEXT NOT NULL DEFAULT 'linkedin';",
    "ALTER TABLE scan_runs ADD COLUMN site TEXT NOT NULL DEFAULT 'linkedin';",
];

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

fn now() -> String {
    Utc::now().to_rfc3339()
}

const COLUMNS: &str = "id, resume_uploaded_at, status, model, data, warnings, error, created_at, updated_at, confirmed_at";

fn profile_from_row(row: &Row) -> rusqlite::Result<(ProfileRecord, Option<String>, String)> {
    Ok((
        ProfileRecord {
            id: row.get(0)?,
            resume_uploaded_at: row.get(1)?,
            status: row.get(2)?,
            model: row.get(3)?,
            profile: None,
            warnings: Vec::new(),
            error: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            confirmed_at: row.get(9)?,
        },
        row.get(4)?,
        row.get(5)?,
    ))
}

fn profile_finish(
    parts: (ProfileRecord, Option<String>, String),
) -> Result<ProfileRecord, DbError> {
    let (mut record, data, warnings) = parts;
    record.profile = data.map(|d| serde_json::from_str(&d)).transpose()?;
    record.warnings = serde_json::from_str(&warnings)?;
    Ok(record)
}

impl Db {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, DbError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        fs::create_dir_all(&data_dir)?;
        let conn = Connection::open(data_dir.join("db.sqlite"))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            data_dir: Arc::new(data_dir),
        })
    }

    pub fn resume_pdf_path(&self) -> PathBuf {
        self.data_dir.join("resume.pdf")
    }

    pub fn get_resume(&self) -> Result<Option<MasterResume>, DbError> {
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

    pub fn save_resume(
        &self,
        file_name: &str,
        bytes: &[u8],
        info: &PdfInfo,
    ) -> Result<MasterResume, DbError> {
        let conn = self.conn.lock().unwrap();

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

    pub fn delete_resume(&self) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let deleted = conn.execute("DELETE FROM master_resume WHERE id = 1", [])?;
        match fs::remove_file(self.resume_pdf_path()) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(deleted > 0)
    }

    pub fn list_searches(&self) -> Result<Vec<JobSearch>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
        "SELECT id, name, titles, keywords, location, work_modes, posted_within, created_at, updated_at, enabled
         FROM job_searches ORDER BY id",
    )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, bool>(9)?,
            ))
        })?;
        rows.map(|row| {
            let (
                id,
                name,
                titles,
                keywords,
                location,
                work_modes,
                posted_within,
                created_at,
                updated_at,
                enabled,
            ) = row?;
            Ok(JobSearch {
                id,
                enabled,
                criteria: SearchCriteria {
                    name,
                    titles: serde_json::from_str(&titles)?,
                    keywords: serde_json::from_str(&keywords)?,
                    location,
                    work_modes: serde_json::from_str(&work_modes)?,
                    posted_within: PostedWithin::parse(&posted_within).unwrap_or_default(),
                },
                created_at,
                updated_at,
            })
        })
        .collect()
    }

    fn get_search(&self, id: i64) -> Result<Option<JobSearch>, DbError> {
        Ok(self.list_searches()?.into_iter().find(|s| s.id == id))
    }

    pub fn create_search(&self, criteria: &SearchCriteria) -> Result<JobSearch, DbError> {
        let id = {
            let conn = self.conn.lock().unwrap();
            conn.execute(
            "INSERT INTO job_searches (name, titles, keywords, location, work_modes, posted_within, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![
                criteria.name,
                serde_json::to_string(&criteria.titles)?,
                serde_json::to_string(&criteria.keywords)?,
                criteria.location,
                serde_json::to_string(&criteria.work_modes)?,
                criteria.posted_within.as_str(),
                now(),
            ],
        )?;
            conn.last_insert_rowid()
        };
        Ok(self.get_search(id)?.expect("search was just inserted"))
    }

    pub fn update_search(
        &self,
        id: i64,
        criteria: &SearchCriteria,
    ) -> Result<Option<JobSearch>, DbError> {
        {
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE job_searches SET name = ?2, titles = ?3, keywords = ?4, location = ?5,
                work_modes = ?6, posted_within = ?7, updated_at = ?8
             WHERE id = ?1",
                params![
                    id,
                    criteria.name,
                    serde_json::to_string(&criteria.titles)?,
                    serde_json::to_string(&criteria.keywords)?,
                    criteria.location,
                    serde_json::to_string(&criteria.work_modes)?,
                    criteria.posted_within.as_str(),
                    now(),
                ],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            tx.execute(
                "DELETE FROM search_matches
                 WHERE search_id = ?1 AND job_id IN (SELECT id FROM jobs WHERE status = 'new')",
                params![id],
            )?;
            tx.commit()?;
        }
        self.get_search(id)
    }

    pub fn delete_search(&self, id: i64) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM job_searches WHERE id = ?1", params![id])? > 0)
    }

    pub fn set_search_enabled(&self, id: i64, enabled: bool) -> Result<Option<JobSearch>, DbError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE job_searches SET enabled = ?2 WHERE id = ?1",
            params![id, enabled],
        )?;
        drop(conn);
        if changed == 0 {
            Ok(None)
        } else {
            self.get_search(id)
        }
    }

    pub(crate) fn known_job(&self, source_id: &str) -> Result<Option<KnownJob>, DbError> {
        let conn = self.conn.lock().unwrap();
        let job = conn
            .query_row(
                "SELECT id, description FROM jobs WHERE source_id = ?1",
                params![source_id],
                |row| {
                    Ok(KnownJob {
                        id: row.get(0)?,
                        description: row.get(1)?,
                    })
                },
            )
            .optional()?;
        Ok(job)
    }

    pub fn upsert_job(
        &self,
        card: &JobCard,
        detail: Option<&JobDetail>,
        work_mode: Option<WorkMode>,
    ) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        let id = conn.query_row(
        "INSERT INTO jobs (source, source_id, title, company, company_url, location, posted_on, url, description,
                           seniority, employment_type, industries, work_mode, first_seen_at, updated_at)
         VALUES (?14, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
         ON CONFLICT(source_id) DO UPDATE SET
            title = excluded.title,
            company = excluded.company,
            company_url = excluded.company_url,
            location = excluded.location,
            posted_on = COALESCE(excluded.posted_on, jobs.posted_on),
            url = excluded.url,
            description = COALESCE(excluded.description, jobs.description),
            seniority = COALESCE(excluded.seniority, jobs.seniority),
            employment_type = COALESCE(excluded.employment_type, jobs.employment_type),
            industries = COALESCE(excluded.industries, jobs.industries),
            work_mode = excluded.work_mode,
            updated_at = excluded.updated_at
         RETURNING id",
        params![
            card.source_id,
            card.title,
            card.company,
            card.company_url,
            card.location,
            card.posted_on,
            card.url,
            detail.map(|d| &d.description),
            detail.and_then(|d| d.seniority.as_ref()),
            detail.and_then(|d| d.employment_type.as_ref()),
            detail.and_then(|d| d.industries.as_ref()),
            work_mode.map(WorkMode::as_str),
            now(),
            card.source.as_str(),
        ],
        |row| row.get(0),
    )?;
        Ok(id)
    }

    pub fn add_match(
        &self,
        search_id: i64,
        job_id: i64,
        keywords: &[String],
    ) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let keywords = serde_json::to_string(keywords)?;
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO search_matches (search_id, job_id, matched_keywords, matched_at)
         VALUES (?1, ?2, ?3, ?4)",
            params![search_id, job_id, keywords, now()],
        )?;
        if inserted == 0 {
            conn.execute(
            "UPDATE search_matches SET matched_keywords = ?3 WHERE search_id = ?1 AND job_id = ?2",
            params![search_id, job_id, keywords],
        )?;
        }
        Ok(inserted == 1)
    }

    pub fn remove_match(&self, search_id: i64, job_id: i64) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM search_matches WHERE search_id = ?1 AND job_id = ?2",
            params![search_id, job_id],
        )?;
        Ok(removed > 0)
    }

    pub fn current_confirmed_profile(&self) -> Result<Option<ProfileRecord>, DbError> {
        let Some(resume) = self.get_resume()? else {
            return Ok(None);
        };
        Ok(self
            .latest_profile(&resume.uploaded_at)?
            .filter(|record| record.status == "confirmed" && record.profile.is_some()))
    }

    pub fn companies_missing_site(&self, limit: usize) -> Result<Vec<String>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT j.company_url
             FROM jobs j JOIN search_matches m ON m.job_id = j.id
             LEFT JOIN companies c ON c.url = j.company_url
             WHERE j.source = 'linkedin' AND j.company_url IS NOT NULL AND c.url IS NULL
             GROUP BY j.company_url
             ORDER BY MAX(j.status = 'saved') DESC, MAX(j.first_seen_at) DESC
             LIMIT ?1",
        )?;
        let urls = stmt
            .query_map(params![limit as i64], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(urls)
    }

    pub fn save_company_site(&self, url: &str, website: Option<&str>) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO companies (url, website, fetched_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(url) DO UPDATE SET website = excluded.website, fetched_at = excluded.fetched_at",
            params![url, website, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn next_job_to_score(&self, profile_id: i64) -> Result<Option<JobToScore>, DbError> {
        let conn = self.conn.lock().unwrap();
        let job = conn
            .query_row(
                "SELECT j.id, j.title, j.company, j.description
                 FROM jobs j LEFT JOIN job_scores s ON s.job_id = j.id
                 WHERE j.status = 'saved' AND j.description IS NOT NULL
                   AND (s.job_id IS NULL OR (s.profile_id != ?1 AND s.status NOT IN ('scoring', 'skipped')))
                 ORDER BY EXISTS (
                     SELECT 1 FROM search_matches m JOIN job_searches q ON q.id = m.search_id
                     WHERE m.job_id = j.id AND q.enabled = 1
                 ) DESC, j.updated_at DESC, j.id DESC
                 LIMIT 1",
                params![profile_id],
                |row| {
                    Ok(JobToScore {
                        job_id: row.get(0)?,
                        title: row.get(1)?,
                        company: row.get(2)?,
                        description: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(job)
    }

    pub fn start_score(&self, job_id: i64, profile_id: i64, model: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO job_scores (job_id, profile_id, status, model, updated_at)
             VALUES (?1, ?2, 'scoring', ?3, ?4)
             ON CONFLICT(job_id) DO UPDATE SET
                profile_id = excluded.profile_id,
                status = 'scoring',
                score = NULL,
                summary = NULL,
                requirements = '[]',
                model = excluded.model,
                error = NULL,
                updated_at = excluded.updated_at",
            params![job_id, profile_id, model, now()],
        )?;
        Ok(())
    }

    pub fn finish_score(
        &self,
        job_id: i64,
        score: u8,
        summary: &str,
        requirements: &[Requirement],
    ) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE job_scores SET status = 'ready', score = ?2, summary = ?3, requirements = ?4,
                error = NULL, updated_at = ?5
             WHERE job_id = ?1 AND status = 'scoring'",
            params![
                job_id,
                score,
                summary,
                serde_json::to_string(requirements)?,
                now()
            ],
        )?;
        Ok(())
    }

    pub fn fail_score(&self, job_id: i64, error: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE job_scores SET status = 'failed', error = ?2, updated_at = ?3
             WHERE job_id = ?1 AND status = 'scoring'",
            params![job_id, error, now()],
        )?;
        Ok(())
    }

    pub fn skip_score(&self, job_id: i64, profile_id: i64) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO job_scores (job_id, profile_id, status, model, updated_at)
             VALUES (?1, ?2, 'skipped', '', ?3)
             ON CONFLICT(job_id) DO UPDATE SET
                status = 'skipped',
                score = NULL,
                summary = NULL,
                requirements = '[]',
                error = NULL,
                updated_at = excluded.updated_at",
            params![job_id, profile_id, now()],
        )?;
        Ok(())
    }

    pub fn score_status(&self, job_id: i64) -> Result<Option<String>, DbError> {
        let conn = self.conn.lock().unwrap();
        let status = conn
            .query_row(
                "SELECT status FROM job_scores WHERE job_id = ?1",
                params![job_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(status)
    }

    pub fn clear_score(&self, job_id: i64) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM job_scores WHERE job_id = ?1", params![job_id])?;
        Ok(())
    }

    pub fn job_status(&self, job_id: i64) -> Result<Option<JobStatus>, DbError> {
        let conn = self.conn.lock().unwrap();
        let status = conn
            .query_row(
                "SELECT status FROM jobs WHERE id = ?1",
                params![job_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(status.as_deref().map(JobStatus::parse))
    }

    pub fn requeue_interrupted_scores(&self) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM job_scores WHERE status = 'scoring'", [])?;
        Ok(())
    }

    pub fn list_jobs(&self, status: Option<JobStatus>) -> Result<Vec<JobListing>, DbError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
        "SELECT j.id, j.source_id, j.title, j.company, j.company_url, j.location, j.posted_on, j.url,
                j.description, j.seniority, j.employment_type, j.industries, j.work_mode, j.status,
                j.first_seen_at, m.search_id, m.matched_keywords, j.tailored_at,
                s.status, s.score, s.summary, s.requirements, s.model, s.error, s.profile_id, s.updated_at,
                c.website, j.source
         FROM jobs j JOIN search_matches m ON m.job_id = j.id
         LEFT JOIN job_scores s ON s.job_id = j.id
         LEFT JOIN companies c ON c.url = j.company_url
         WHERE ?1 IS NULL OR j.status = ?1
         ORDER BY COALESCE(j.posted_on, '') DESC, j.first_seen_at DESC, j.id DESC, m.search_id",
    )?;
        let rows = stmt.query_map(params![status.map(JobStatus::as_str)], |row| {
            let listing = JobListing {
                id: row.get(0)?,
                source: Site::parse(&row.get::<_, String>(27)?),
                source_id: row.get(1)?,
                title: row.get(2)?,
                company: row.get(3)?,
                company_url: row.get(4)?,
                company_website: row.get(26)?,
                location: row.get(5)?,
                posted_on: row.get(6)?,
                url: row.get(7)?,
                description: row.get(8)?,
                seniority: row.get(9)?,
                employment_type: row.get(10)?,
                industries: row.get(11)?,
                work_mode: row
                    .get::<_, Option<String>>(12)?
                    .as_deref()
                    .and_then(WorkMode::parse),
                status: JobStatus::parse(&row.get::<_, String>(13)?),
                first_seen_at: row.get(14)?,
                tailored_at: row.get(17)?,
                search_ids: vec![row.get(15)?],
                matched_keywords: Vec::new(),
                score: None,
            };
            let score = match row.get::<_, Option<String>>(18)? {
                Some(status) => Some((
                    status,
                    row.get::<_, Option<i64>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, String>(21)?,
                    row.get::<_, String>(22)?,
                    row.get::<_, Option<String>>(23)?,
                    row.get::<_, i64>(24)?,
                    row.get::<_, String>(25)?,
                )),
                None => None,
            };
            Ok((listing, row.get::<_, String>(16)?, score))
        })?;

        let mut jobs: Vec<JobListing> = Vec::new();
        let mut index: HashMap<i64, usize> = HashMap::new();
        for row in rows {
            let (mut listing, keywords, score) = row?;
            let keywords: Vec<String> = serde_json::from_str(&keywords)?;
            if let Some((
                status,
                score,
                summary,
                requirements,
                model,
                error,
                profile_id,
                updated_at,
            )) = score
            {
                listing.score = Some(JobScore {
                    status,
                    score: score.map(|s| s.clamp(0, 100) as u8),
                    summary,
                    requirements: serde_json::from_str(&requirements)?,
                    model,
                    error,
                    profile_id,
                    updated_at,
                });
            }
            let job = match index.get(&listing.id) {
                Some(&i) => {
                    jobs[i].search_ids.extend(listing.search_ids);
                    &mut jobs[i]
                }
                None => {
                    index.insert(listing.id, jobs.len());
                    jobs.push(listing);
                    jobs.last_mut().unwrap()
                }
            };
            for keyword in keywords {
                if !job
                    .matched_keywords
                    .iter()
                    .any(|k| k.eq_ignore_ascii_case(&keyword))
                {
                    job.matched_keywords.push(keyword);
                }
            }
        }
        Ok(jobs)
    }

    pub fn set_job_status(&self, id: i64, status: JobStatus) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), now()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_jobs_status(&self, ids: &[i64], status: JobStatus) -> Result<usize, DbError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut changed = 0;
        {
            let mut stmt = tx.prepare(
                "UPDATE jobs SET status = ?2, updated_at = ?3 WHERE id = ?1 AND status != ?2",
            )?;
            let ts = now();
            for id in ids {
                changed += stmt.execute(params![id, status.as_str(), ts])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, DbError> {
        let conn = self.conn.lock().unwrap();
        let value = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }

    pub fn start_scan_run(&self, site: Site) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO scan_runs (started_at, status, site) VALUES (?1, 'running', ?2)",
            params![now(), site.as_str()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_scan_run(
        &self,
        id: i64,
        status: &str,
        new_matches: usize,
        message: Option<&str>,
    ) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "UPDATE scan_runs SET finished_at = ?2, status = ?3, new_matches = ?4, message = ?5 WHERE id = ?1",
        params![id, now(), status, new_matches as i64, message],
    )?;
        Ok(())
    }

    pub fn last_scan_run(&self, site: Site) -> Result<Option<ScanRun>, DbError> {
        let conn = self.conn.lock().unwrap();
        let run = conn
            .query_row(
                "SELECT id, started_at, finished_at, status, new_matches, message
             FROM scan_runs WHERE site = ?1 ORDER BY id DESC LIMIT 1",
                params![site.as_str()],
                |row| {
                    Ok(ScanRun {
                        id: row.get(0)?,
                        started_at: row.get(1)?,
                        finished_at: row.get(2)?,
                        status: row.get(3)?,
                        new_matches: row.get(4)?,
                        message: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(run)
    }

    pub fn mark_interrupted_scan_runs(&self) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "UPDATE scan_runs SET status = 'interrupted', finished_at = ?1 WHERE status = 'running'",
        params![now()],
    )?;
        Ok(())
    }

    pub fn start_profile(&self, resume_uploaded_at: &str, model: &str) -> Result<i64, DbError> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        conn.execute(
        "INSERT INTO resume_profiles (resume_uploaded_at, status, model, created_at, updated_at)
         VALUES (?1, 'reading', ?2, ?3, ?3)",
        params![resume_uploaded_at, model, ts],
    )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_profile(
        &self,
        id: i64,
        profile: &ResumeProfile,
        warnings: &[Warning],
    ) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "UPDATE resume_profiles SET status = 'ready', data = ?2, warnings = ?3, error = NULL, updated_at = ?4
         WHERE id = ?1",
        params![id, serde_json::to_string(profile)?, serde_json::to_string(warnings)?, now()],
    )?;
        Ok(())
    }

    pub fn fail_profile(&self, id: i64, error: &str) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "UPDATE resume_profiles SET status = 'failed', error = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, error, now()],
    )?;
        Ok(())
    }

    pub fn latest_profile(
        &self,
        resume_uploaded_at: &str,
    ) -> Result<Option<ProfileRecord>, DbError> {
        let conn = self.conn.lock().unwrap();
        let parts = conn
        .query_row(
            &format!(
                "SELECT {COLUMNS} FROM resume_profiles WHERE resume_uploaded_at = ?1 ORDER BY id DESC LIMIT 1"
            ),
            params![resume_uploaded_at],
            profile_from_row,
        )
        .optional()?;
        parts.map(profile_finish).transpose()
    }

    pub fn get_profile(&self, id: i64) -> Result<Option<ProfileRecord>, DbError> {
        let conn = self.conn.lock().unwrap();
        let parts = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM resume_profiles WHERE id = ?1"),
                params![id],
                profile_from_row,
            )
            .optional()?;
        parts.map(profile_finish).transpose()
    }

    pub fn save_profile_edits(
        &self,
        id: i64,
        profile: &ResumeProfile,
        warnings: &[Warning],
    ) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
        "UPDATE resume_profiles SET status = 'ready', data = ?2, warnings = ?3, confirmed_at = NULL, updated_at = ?4
         WHERE id = ?1 AND status IN ('ready', 'confirmed')",
        params![id, serde_json::to_string(profile)?, serde_json::to_string(warnings)?, now()],
    )?;
        Ok(changed > 0)
    }

    pub fn confirm_profile(&self, id: i64) -> Result<bool, DbError> {
        let conn = self.conn.lock().unwrap();
        let ts = now();
        let changed = conn.execute(
            "UPDATE resume_profiles SET status = 'confirmed', confirmed_at = ?2, updated_at = ?2
         WHERE id = ?1 AND status = 'ready'",
            params![id, ts],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_interrupted_profiles(&self) -> Result<(), DbError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
        "UPDATE resume_profiles SET status = 'failed', error = 'Interrupted by a restart. Try again.', updated_at = ?1
         WHERE status = 'reading'",
        params![now()],
    )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Contact, SearchInput};
    fn temp_store() -> (Db, PathBuf) {
        let dir = test_dir("tailor-store-test");
        (Db::open(&dir).unwrap(), dir)
    }

    #[test]
    fn save_replace_and_delete() {
        let (store, dir) = temp_store();
        assert!(store.get_resume().unwrap().is_none());

        let info = PdfInfo {
            page_count: 1,
            text: "first".into(),
        };
        store.save_resume("a.pdf", b"%PDF-1", &info).unwrap();
        let info = PdfInfo {
            page_count: 2,
            text: "second".into(),
        };
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
        let info = PdfInfo {
            page_count: 1,
            text: String::new(),
        };
        store.save_resume("a.pdf", b"%PDF-1", &info).unwrap();
        drop(store);

        let store = Db::open(&dir).unwrap();
        assert_eq!(store.get_resume().unwrap().unwrap().file_name, "a.pdf");
        fs::remove_dir_all(dir).unwrap();
    }

    fn temp_job_store() -> (Db, std::path::PathBuf) {
        let dir = test_dir("tailor-jobs-test");
        (Db::open(&dir).unwrap(), dir)
    }

    fn criteria(title: &str) -> SearchCriteria {
        SearchInput {
            name: None,
            titles: vec![title.into()],
            keywords: vec!["Rust".into()],
            location: Some("Dubai".into()),
            work_modes: vec![WorkMode::Hybrid],
            posted_within: PostedWithin::Week,
        }
        .validate()
        .unwrap()
    }

    fn card(id: &str) -> JobCard {
        JobCard {
            source: Site::LinkedIn,
            source_id: id.into(),
            title: "Backend Engineer".into(),
            company: "Acme".into(),
            company_url: None,
            location: "Dubai".into(),
            posted_on: Some("2026-10-19".into()),
            url: format!("https://www.linkedin.com/jobs/view/{id}"),
        }
    }

    #[test]
    fn search_crud() {
        let (store, dir) = temp_job_store();
        let search = store.create_search(&criteria("Backend Engineer")).unwrap();
        assert_eq!(search.criteria.titles, vec!["Backend Engineer"]);

        let updated = store
            .update_search(search.id, &criteria("Platform Engineer"))
            .unwrap()
            .unwrap();
        assert_eq!(updated.criteria.titles, vec!["Platform Engineer"]);
        assert!(store.update_search(999, &criteria("x")).unwrap().is_none());

        assert!(store.delete_search(search.id).unwrap());
        assert!(store.list_searches().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scoring_queue_picks_saved_jobs_for_the_confirmed_profile() {
        let (store, dir) = temp_store();
        assert!(store.current_confirmed_profile().unwrap().is_none());

        let resume = store
            .save_resume(
                "cv.pdf",
                b"%PDF-1",
                &PdfInfo {
                    page_count: 1,
                    text: "cv".into(),
                },
            )
            .unwrap();
        let profile_id = store.start_profile(&resume.uploaded_at, "fake").unwrap();
        let profile = ResumeProfile::default();
        store.finish_profile(profile_id, &profile, &[]).unwrap();
        assert!(store.current_confirmed_profile().unwrap().is_none());
        store.confirm_profile(profile_id).unwrap();
        assert_eq!(
            store.current_confirmed_profile().unwrap().unwrap().id,
            profile_id
        );

        let detail = JobDetail {
            description: "Rust".into(),
            seniority: None,
            employment_type: None,
            industries: None,
        };
        let job = store.upsert_job(&card("1"), Some(&detail), None).unwrap();
        let undescribed = store.upsert_job(&card("2"), None, None).unwrap();
        store
            .set_jobs_status(&[job, undescribed], JobStatus::Saved)
            .unwrap();
        assert_eq!(
            store.next_job_to_score(profile_id).unwrap().unwrap().job_id,
            job
        );

        store.start_score(job, profile_id, "fake").unwrap();
        assert!(store.next_job_to_score(profile_id).unwrap().is_none());
        assert!(store.next_job_to_score(profile_id + 1).unwrap().is_none());

        let reqs = vec![Requirement {
            text: "Rust".into(),
            importance: crate::types::Importance::Must,
            verdict: crate::types::Verdict::Met,
            evidence: None,
        }];
        store.finish_score(job, 80, "Good fit.", &reqs).unwrap();
        assert!(store.next_job_to_score(profile_id).unwrap().is_none());
        assert_eq!(
            store
                .next_job_to_score(profile_id + 1)
                .unwrap()
                .unwrap()
                .job_id,
            job
        );

        let search = store.create_search(&criteria("Backend Engineer")).unwrap();
        store.add_match(search.id, job, &[]).unwrap();
        let listed = store.list_jobs(None).unwrap();
        let score = listed[0].score.as_ref().unwrap();
        assert_eq!(
            (score.status.as_str(), score.score, score.requirements.len()),
            ("ready", Some(80), 1)
        );

        store.clear_score(job).unwrap();
        assert_eq!(
            store.next_job_to_score(profile_id).unwrap().unwrap().job_id,
            job
        );
        store.set_job_status(job, JobStatus::New).unwrap();
        assert!(store.next_job_to_score(profile_id).unwrap().is_none());

        store.set_job_status(job, JobStatus::Saved).unwrap();
        store.start_score(job, profile_id, "fake").unwrap();
        assert!(store.next_job_to_score(profile_id).unwrap().is_none());
        store.skip_score(job, profile_id).unwrap();
        store.finish_score(job, 90, "late result", &[]).unwrap();
        assert_eq!(store.score_status(job).unwrap().as_deref(), Some("skipped"));
        assert!(store.next_job_to_score(profile_id).unwrap().is_none());
        assert!(store.next_job_to_score(profile_id + 1).unwrap().is_none());
        store.clear_score(job).unwrap();
        store.start_score(job, profile_id, "fake").unwrap();
        store.requeue_interrupted_scores().unwrap();
        assert_eq!(store.score_status(job).unwrap(), None);
        assert_eq!(
            store.next_job_to_score(profile_id).unwrap().unwrap().job_id,
            job
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn scoring_queue_puts_enabled_preferences_first() {
        let (store, dir) = temp_store();
        let detail = JobDetail {
            description: "Rust".into(),
            seniority: None,
            employment_type: None,
            industries: None,
        };
        let paused_job = store.upsert_job(&card("1"), Some(&detail), None).unwrap();
        let enabled_job = store.upsert_job(&card("2"), Some(&detail), None).unwrap();
        store
            .set_jobs_status(&[enabled_job], JobStatus::Saved)
            .unwrap();
        store
            .set_jobs_status(&[paused_job], JobStatus::Saved)
            .unwrap();

        let paused = store.create_search(&criteria("Backend Engineer")).unwrap();
        let enabled = store.create_search(&criteria("Platform Engineer")).unwrap();
        store.set_search_enabled(paused.id, false).unwrap();
        store.add_match(paused.id, paused_job, &[]).unwrap();
        store.add_match(enabled.id, enabled_job, &[]).unwrap();

        assert_eq!(
            store.next_job_to_score(1).unwrap().unwrap().job_id,
            enabled_job
        );
        store.set_search_enabled(enabled.id, false).unwrap();
        assert_eq!(
            store.next_job_to_score(1).unwrap().unwrap().job_id,
            paused_job
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn upsert_keeps_description_and_matches_fold() {
        let (store, dir) = temp_job_store();
        let a = store.create_search(&criteria("Backend Engineer")).unwrap();
        let b = store.create_search(&criteria("Engineer")).unwrap();

        let detail = JobDetail {
            description: "Rust and Kafka".into(),
            seniority: Some("Mid-Senior level".into()),
            employment_type: None,
            industries: None,
        };
        let id = store
            .upsert_job(&card("1"), Some(&detail), Some(WorkMode::Hybrid))
            .unwrap();
        assert_eq!(
            store
                .upsert_job(&card("1"), None, Some(WorkMode::Hybrid))
                .unwrap(),
            id
        );
        assert_eq!(
            store
                .known_job("1")
                .unwrap()
                .unwrap()
                .description
                .as_deref(),
            Some("Rust and Kafka")
        );

        assert!(store.add_match(a.id, id, &["Rust".into()]).unwrap());
        assert!(!store.add_match(a.id, id, &["Rust".into()]).unwrap());
        assert!(store.add_match(b.id, id, &["Kafka".into()]).unwrap());

        let jobs = store.list_jobs(None).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].search_ids, vec![a.id, b.id]);
        assert_eq!(jobs[0].matched_keywords, vec!["Rust", "Kafka"]);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Hybrid));

        assert_eq!(jobs[0].tailored_at, None);
        assert_eq!(
            store.set_jobs_status(&[id, 999], JobStatus::Saved).unwrap(),
            1
        );
        assert_eq!(
            store.set_jobs_status(&[id], JobStatus::Saved).unwrap(),
            0,
            "already saved"
        );
        assert!(store.set_job_status(id, JobStatus::Dismissed).unwrap());
        assert!(store.list_jobs(Some(JobStatus::New)).unwrap().is_empty());
        assert_eq!(
            store.list_jobs(Some(JobStatus::Dismissed)).unwrap().len(),
            1
        );

        store
            .update_search(a.id, &criteria("Backend Engineer"))
            .unwrap();
        assert_eq!(
            store.list_jobs(None).unwrap()[0].search_ids,
            vec![a.id, b.id]
        );

        store.set_job_status(id, JobStatus::New).unwrap();
        store
            .update_search(a.id, &criteria("Backend Engineer"))
            .unwrap();
        assert_eq!(store.list_jobs(None).unwrap()[0].search_ids, vec![b.id]);
        store.delete_search(b.id).unwrap();
        assert!(store.list_jobs(None).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_and_scan_runs() {
        let (store, dir) = temp_job_store();
        assert_eq!(store.get_setting("k").unwrap(), None);
        store.set_setting("k", "30").unwrap();
        store.set_setting("k", "45").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("45"));

        let run = store.start_scan_run(Site::LinkedIn).unwrap();
        store.mark_interrupted_scan_runs().unwrap();
        assert_eq!(
            store.last_scan_run(Site::LinkedIn).unwrap().unwrap().status,
            "interrupted"
        );
        store.finish_scan_run(run, "ok", 3, None).unwrap();
        let last = store.last_scan_run(Site::LinkedIn).unwrap().unwrap();
        assert_eq!((last.status.as_str(), last.new_matches), ("ok", 3));
        assert!(store.last_scan_run(Site::Indeed).unwrap().is_none());
        let indeed = store.start_scan_run(Site::Indeed).unwrap();
        assert_eq!(
            store.last_scan_run(Site::Indeed).unwrap().unwrap().id,
            indeed
        );
        assert_eq!(
            store.last_scan_run(Site::LinkedIn).unwrap().unwrap().id,
            run
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn delete_setting_removes_saved_value() {
        let (store, dir) = temp_job_store();
        store.set_setting("k", "value").unwrap();
        store.delete_setting("k").unwrap();
        assert_eq!(store.get_setting("k").unwrap(), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reading_lifecycle() {
        let dir = test_dir("tailor-profile-test");
        let store = Db::open(&dir).unwrap();

        let id = store.start_profile("t1", "fake").unwrap();
        assert_eq!(
            store.latest_profile("t1").unwrap().unwrap().status,
            "reading"
        );
        assert!(store.latest_profile("t2").unwrap().is_none());
        assert!(!store.confirm_profile(id).unwrap());

        let profile = ResumeProfile {
            contact: Contact {
                name: "Jordan".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let warnings = vec![Warning {
            item_id: "e1.b1".into(),
            message: "x".into(),
        }];
        store.finish_profile(id, &profile, &warnings).unwrap();
        let record = store.latest_profile("t1").unwrap().unwrap();
        assert_eq!(
            (record.status.as_str(), record.warnings.len()),
            ("ready", 1)
        );
        assert_eq!(record.profile.unwrap().contact.name, "Jordan");

        assert!(store.confirm_profile(id).unwrap());
        assert!(store
            .get_profile(id)
            .unwrap()
            .unwrap()
            .confirmed_at
            .is_some());

        assert!(store.save_profile_edits(id, &profile, &[]).unwrap());
        let record = store.get_profile(id).unwrap().unwrap();
        assert_eq!(
            (record.status.as_str(), record.confirmed_at),
            ("ready", None)
        );

        let other = store.start_profile("t1", "fake").unwrap();
        store.mark_interrupted_profiles().unwrap();
        let record = store.latest_profile("t1").unwrap().unwrap();
        assert_eq!((record.id, record.status.as_str()), (other, "failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn company_sites_are_fetched_once_and_listed() {
        let (store, dir) = temp_store();
        let company = "https://ae.linkedin.com/company/acme";
        let mut with_company = card("1");
        with_company.company_url = Some(company.into());
        let job = store.upsert_job(&with_company, None, None).unwrap();
        store.upsert_job(&card("2"), None, None).unwrap();
        let search = store.create_search(&criteria("Backend Engineer")).unwrap();
        store.add_match(search.id, job, &[]).unwrap();

        assert_eq!(
            store.companies_missing_site(8).unwrap(),
            vec![company.to_string()]
        );
        store
            .save_company_site(company, Some("https://acme.com/"))
            .unwrap();
        assert!(store.companies_missing_site(8).unwrap().is_empty());
        let listed = store.list_jobs(None).unwrap();
        assert_eq!(
            listed[0].company_website.as_deref(),
            Some("https://acme.com/")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
