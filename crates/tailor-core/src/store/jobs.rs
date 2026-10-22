
use std::collections::HashMap;

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::{Store, StoreError};
use crate::{
    linkedin::{JobCard, JobDetail},
    search::{JobSearch, PostedWithin, SearchCriteria, WorkMode},
};

fn now() -> String {
    Utc::now().to_rfc3339()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    New,
    Saved,
    Dismissed,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Saved => "saved",
            Self::Dismissed => "dismissed",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "saved" => Self::Saved,
            "dismissed" => Self::Dismissed,
            _ => Self::New,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JobListing {
    pub id: i64,
    pub linkedin_id: String,
    pub title: String,
    pub company: String,
    pub company_url: Option<String>,
    pub location: String,
    pub posted_on: Option<String>,
    pub url: String,
    pub description: Option<String>,
    pub seniority: Option<String>,
    pub employment_type: Option<String>,
    pub industries: Option<String>,
    pub work_mode: Option<WorkMode>,
    pub status: JobStatus,
    pub first_seen_at: String,
    pub tailored_at: Option<String>,
    pub search_ids: Vec<i64>,
    pub matched_keywords: Vec<String>,
}

pub struct KnownJob {
    pub id: i64,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanRun {
    pub id: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub new_matches: i64,
    pub message: Option<String>,
}

impl Store {
    pub fn list_searches(&self) -> Result<Vec<JobSearch>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, titles, keywords, location, work_modes, posted_within, created_at, updated_at
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
            ))
        })?;
        rows.map(|row| {
            let (id, name, titles, keywords, location, work_modes, posted_within, created_at, updated_at) = row?;
            Ok(JobSearch {
                id,
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

    fn get_search(&self, id: i64) -> Result<Option<JobSearch>, StoreError> {
        Ok(self.list_searches()?.into_iter().find(|s| s.id == id))
    }

    pub fn create_search(&self, criteria: &SearchCriteria) -> Result<JobSearch, StoreError> {
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

    pub fn update_search(&self, id: i64, criteria: &SearchCriteria) -> Result<Option<JobSearch>, StoreError> {
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
            tx.execute("DELETE FROM search_matches WHERE search_id = ?1", params![id])?;
            tx.commit()?;
        }
        self.get_search(id)
    }

    pub fn delete_search(&self, id: i64) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM job_searches WHERE id = ?1", params![id])? > 0)
    }

    pub fn known_job(&self, linkedin_id: &str) -> Result<Option<KnownJob>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let job = conn
            .query_row("SELECT id, description FROM jobs WHERE linkedin_id = ?1", params![linkedin_id], |row| {
                Ok(KnownJob { id: row.get(0)?, description: row.get(1)? })
            })
            .optional()?;
        Ok(job)
    }

    pub fn upsert_job(
        &self,
        card: &JobCard,
        detail: Option<&JobDetail>,
        work_mode: Option<WorkMode>,
    ) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        let id = conn.query_row(
            "INSERT INTO jobs (linkedin_id, title, company, company_url, location, posted_on, url, description,
                               seniority, employment_type, industries, work_mode, first_seen_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
             ON CONFLICT(linkedin_id) DO UPDATE SET
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
                card.linkedin_id,
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
            ],
            |row| row.get(0),
        )?;
        Ok(id)
    }

    pub fn add_match(&self, search_id: i64, job_id: i64, keywords: &[String]) -> Result<bool, StoreError> {
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

    pub fn remove_match(&self, search_id: i64, job_id: i64) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM search_matches WHERE search_id = ?1 AND job_id = ?2",
            params![search_id, job_id],
        )?;
        Ok(removed > 0)
    }

    pub fn list_jobs(&self, status: Option<JobStatus>) -> Result<Vec<JobListing>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT j.id, j.linkedin_id, j.title, j.company, j.company_url, j.location, j.posted_on, j.url,
                    j.description, j.seniority, j.employment_type, j.industries, j.work_mode, j.status,
                    j.first_seen_at, m.search_id, m.matched_keywords, j.tailored_at
             FROM jobs j JOIN search_matches m ON m.job_id = j.id
             WHERE ?1 IS NULL OR j.status = ?1
             ORDER BY COALESCE(j.posted_on, '') DESC, j.first_seen_at DESC, j.id DESC, m.search_id",
        )?;
        let rows = stmt.query_map(params![status.map(JobStatus::as_str)], |row| {
            let listing = JobListing {
                id: row.get(0)?,
                linkedin_id: row.get(1)?,
                title: row.get(2)?,
                company: row.get(3)?,
                company_url: row.get(4)?,
                location: row.get(5)?,
                posted_on: row.get(6)?,
                url: row.get(7)?,
                description: row.get(8)?,
                seniority: row.get(9)?,
                employment_type: row.get(10)?,
                industries: row.get(11)?,
                work_mode: row.get::<_, Option<String>>(12)?.as_deref().and_then(WorkMode::parse),
                status: JobStatus::parse(&row.get::<_, String>(13)?),
                first_seen_at: row.get(14)?,
                tailored_at: row.get(17)?,
                search_ids: vec![row.get(15)?],
                matched_keywords: Vec::new(),
            };
            Ok((listing, row.get::<_, String>(16)?))
        })?;

        let mut jobs: Vec<JobListing> = Vec::new();
        let mut index: HashMap<i64, usize> = HashMap::new();
        for row in rows {
            let (listing, keywords) = row?;
            let keywords: Vec<String> = serde_json::from_str(&keywords)?;
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
                if !job.matched_keywords.iter().any(|k| k.eq_ignore_ascii_case(&keyword)) {
                    job.matched_keywords.push(keyword);
                }
            }
        }
        Ok(jobs)
    }

    pub fn set_job_status(&self, id: i64, status: JobStatus) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE jobs SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), now()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_jobs_status(&self, ids: &[i64], status: JobStatus) -> Result<usize, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut changed = 0;
        {
            let mut stmt = tx.prepare("UPDATE jobs SET status = ?2, updated_at = ?3 WHERE id = ?1 AND status != ?2")?;
            let ts = now();
            for id in ids {
                changed += stmt.execute(params![id, status.as_str(), ts])?;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let value =
            conn.query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0)).optional()?;
        Ok(value)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn start_scan_run(&self) -> Result<i64, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO scan_runs (started_at, status) VALUES (?1, 'running')", params![now()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_scan_run(
        &self,
        id: i64,
        status: &str,
        new_matches: usize,
        message: Option<&str>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE scan_runs SET finished_at = ?2, status = ?3, new_matches = ?4, message = ?5 WHERE id = ?1",
            params![id, now(), status, new_matches as i64, message],
        )?;
        Ok(())
    }

    pub fn last_scan_run(&self) -> Result<Option<ScanRun>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let run = conn
            .query_row(
                "SELECT id, started_at, finished_at, status, new_matches, message
                 FROM scan_runs ORDER BY id DESC LIMIT 1",
                [],
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

    pub fn mark_interrupted_scan_runs(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE scan_runs SET status = 'interrupted', finished_at = ?1 WHERE status = 'running'",
            params![now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchInput;

    fn temp_store() -> (Store, std::path::PathBuf) {
        let dir = crate::store::test_dir("tailor-jobs-test");
        (Store::open(&dir).unwrap(), dir)
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
            linkedin_id: id.into(),
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
        let (store, dir) = temp_store();
        let search = store.create_search(&criteria("Backend Engineer")).unwrap();
        assert_eq!(search.criteria.titles, vec!["Backend Engineer"]);

        let updated = store.update_search(search.id, &criteria("Platform Engineer")).unwrap().unwrap();
        assert_eq!(updated.criteria.titles, vec!["Platform Engineer"]);
        assert!(store.update_search(999, &criteria("x")).unwrap().is_none());

        assert!(store.delete_search(search.id).unwrap());
        assert!(store.list_searches().unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn upsert_keeps_description_and_matches_fold() {
        let (store, dir) = temp_store();
        let a = store.create_search(&criteria("Backend Engineer")).unwrap();
        let b = store.create_search(&criteria("Engineer")).unwrap();

        let detail = JobDetail {
            description: "Rust and Kafka".into(),
            seniority: Some("Mid-Senior level".into()),
            employment_type: None,
            industries: None,
        };
        let id = store.upsert_job(&card("1"), Some(&detail), Some(WorkMode::Hybrid)).unwrap();
        assert_eq!(store.upsert_job(&card("1"), None, Some(WorkMode::Hybrid)).unwrap(), id);
        assert_eq!(store.known_job("1").unwrap().unwrap().description.as_deref(), Some("Rust and Kafka"));

        assert!(store.add_match(a.id, id, &["Rust".into()]).unwrap());
        assert!(!store.add_match(a.id, id, &["Rust".into()]).unwrap());
        assert!(store.add_match(b.id, id, &["Kafka".into()]).unwrap());

        let jobs = store.list_jobs(None).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].search_ids, vec![a.id, b.id]);
        assert_eq!(jobs[0].matched_keywords, vec!["Rust", "Kafka"]);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Hybrid));

        assert_eq!(jobs[0].tailored_at, None);
        assert_eq!(store.set_jobs_status(&[id, 999], JobStatus::Saved).unwrap(), 1);
        assert_eq!(store.set_jobs_status(&[id], JobStatus::Saved).unwrap(), 0, "already saved");
        assert!(store.set_job_status(id, JobStatus::Dismissed).unwrap());
        assert!(store.list_jobs(Some(JobStatus::New)).unwrap().is_empty());
        assert_eq!(store.list_jobs(Some(JobStatus::Dismissed)).unwrap().len(), 1);

        store.update_search(a.id, &criteria("Backend Engineer")).unwrap();
        assert_eq!(store.list_jobs(None).unwrap()[0].search_ids, vec![b.id]);
        store.delete_search(b.id).unwrap();
        assert!(store.list_jobs(None).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_and_scan_runs() {
        let (store, dir) = temp_store();
        assert_eq!(store.get_setting("k").unwrap(), None);
        store.set_setting("k", "30").unwrap();
        store.set_setting("k", "45").unwrap();
        assert_eq!(store.get_setting("k").unwrap().as_deref(), Some("45"));

        let run = store.start_scan_run().unwrap();
        store.mark_interrupted_scan_runs().unwrap();
        assert_eq!(store.last_scan_run().unwrap().unwrap().status, "interrupted");
        store.finish_scan_run(run, "ok", 3, None).unwrap();
        let last = store.last_scan_run().unwrap().unwrap();
        assert_eq!((last.status.as_str(), last.new_matches), ("ok", 3));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
