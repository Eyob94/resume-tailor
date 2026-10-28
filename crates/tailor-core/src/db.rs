use std::collections::HashMap;

use chrono::Utc;
use sqlx::{PgPool, Row};

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
    pool: PgPool,
    user_id: i64,
}

pub(crate) struct KnownJob {
    pub(crate) id: i64,
    pub(crate) description: Option<String>,
}

const PROFILE_COLUMNS: &str = "id, resume_uploaded_at, status, model, data, warnings, error, created_at, updated_at, confirmed_at";

fn now() -> String {
    Utc::now().to_rfc3339()
}

impl Db {
    pub fn for_user(pool: PgPool, user_id: i64) -> Self {
        Self { pool, user_id }
    }

    pub async fn get_resume(&self) -> Result<Option<MasterResume>, DbError> {
        let row = sqlx::query("SELECT file_name, size_bytes, page_count, extracted_text, uploaded_at FROM master_resume WHERE user_id = $1")
            .bind(self.user_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(MasterResume {
                file_name: row.try_get("file_name")?,
                size_bytes: row.try_get::<i64, _>("size_bytes")?.max(0) as u64,
                page_count: row.try_get::<i64, _>("page_count")?.max(0) as usize,
                extracted_text: row.try_get("extracted_text")?,
                uploaded_at: row.try_get("uploaded_at")?,
            })
        })
        .transpose()
    }

    pub async fn save_resume(
        &self,
        file_name: &str,
        bytes: &[u8],
        info: &PdfInfo,
    ) -> Result<MasterResume, DbError> {
        let resume = MasterResume {
            file_name: file_name.to_owned(),
            size_bytes: bytes.len() as u64,
            page_count: info.page_count,
            extracted_text: info.text.clone(),
            uploaded_at: now(),
        };
        sqlx::query("INSERT INTO master_resume (user_id, file_name, size_bytes, page_count, extracted_text, pdf, uploaded_at) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (user_id) DO UPDATE SET file_name = EXCLUDED.file_name, size_bytes = EXCLUDED.size_bytes, page_count = EXCLUDED.page_count, extracted_text = EXCLUDED.extracted_text, pdf = EXCLUDED.pdf, uploaded_at = EXCLUDED.uploaded_at")
            .bind(self.user_id).bind(&resume.file_name).bind(resume.size_bytes as i64).bind(resume.page_count as i64).bind(&resume.extracted_text).bind(bytes).bind(&resume.uploaded_at).execute(&self.pool).await?;
        Ok(resume)
    }

    pub async fn resume_pdf(&self) -> Result<Option<Vec<u8>>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT pdf FROM master_resume WHERE user_id = $1")
                .bind(self.user_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn delete_resume(&self) -> Result<bool, DbError> {
        Ok(sqlx::query("DELETE FROM master_resume WHERE user_id = $1")
            .bind(self.user_id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    pub async fn list_searches(&self) -> Result<Vec<JobSearch>, DbError> {
        let rows = sqlx::query("SELECT id, name, titles, keywords, location, work_modes, posted_within, created_at, updated_at, enabled FROM job_searches WHERE user_id = $1 ORDER BY id")
            .bind(self.user_id).fetch_all(&self.pool).await?;
        rows.into_iter()
            .map(|row| {
                Ok(JobSearch {
                    id: row.try_get("id")?,
                    enabled: row.try_get("enabled")?,
                    criteria: SearchCriteria {
                        name: row.try_get("name")?,
                        titles: serde_json::from_str(&row.try_get::<String, _>("titles")?)?,
                        keywords: serde_json::from_str(&row.try_get::<String, _>("keywords")?)?,
                        location: row.try_get("location")?,
                        work_modes: serde_json::from_str(&row.try_get::<String, _>("work_modes")?)?,
                        posted_within: PostedWithin::parse(
                            &row.try_get::<String, _>("posted_within")?,
                        )
                        .unwrap_or_default(),
                    },
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect()
    }

    pub async fn create_search(&self, criteria: &SearchCriteria) -> Result<JobSearch, DbError> {
        let ts = now();
        let id: i64 = sqlx::query_scalar("INSERT INTO job_searches (user_id, name, titles, keywords, location, work_modes, posted_within, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8) RETURNING id")
            .bind(self.user_id).bind(&criteria.name).bind(serde_json::to_string(&criteria.titles)?).bind(serde_json::to_string(&criteria.keywords)?).bind(&criteria.location).bind(serde_json::to_string(&criteria.work_modes)?).bind(criteria.posted_within.as_str()).bind(ts).fetch_one(&self.pool).await?;
        Ok(self
            .list_searches()
            .await?
            .into_iter()
            .find(|search| search.id == id)
            .expect("search was just inserted"))
    }

    pub async fn update_search(
        &self,
        id: i64,
        criteria: &SearchCriteria,
    ) -> Result<Option<JobSearch>, DbError> {
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query("UPDATE job_searches SET name=$3, titles=$4, keywords=$5, location=$6, work_modes=$7, posted_within=$8, updated_at=$9 WHERE id=$1 AND user_id=$2")
            .bind(id).bind(self.user_id).bind(&criteria.name).bind(serde_json::to_string(&criteria.titles)?).bind(serde_json::to_string(&criteria.keywords)?).bind(&criteria.location).bind(serde_json::to_string(&criteria.work_modes)?).bind(criteria.posted_within.as_str()).bind(now()).execute(&mut *tx).await?.rows_affected();
        if changed == 0 {
            tx.rollback().await?;
            return Ok(None);
        }
        sqlx::query("DELETE FROM search_matches m USING jobs j WHERE m.search_id=$1 AND m.job_id=j.id AND j.user_id=$2 AND j.status='new'")
            .bind(id).bind(self.user_id).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(self
            .list_searches()
            .await?
            .into_iter()
            .find(|search| search.id == id))
    }

    pub async fn delete_search(&self, id: i64) -> Result<bool, DbError> {
        Ok(
            sqlx::query("DELETE FROM job_searches WHERE id=$1 AND user_id=$2")
                .bind(id)
                .bind(self.user_id)
                .execute(&self.pool)
                .await?
                .rows_affected()
                > 0,
        )
    }

    pub async fn set_search_enabled(
        &self,
        id: i64,
        enabled: bool,
    ) -> Result<Option<JobSearch>, DbError> {
        let changed = sqlx::query("UPDATE job_searches SET enabled=$3 WHERE id=$1 AND user_id=$2")
            .bind(id)
            .bind(self.user_id)
            .bind(enabled)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if changed == 0 {
            return Ok(None);
        }
        Ok(self
            .list_searches()
            .await?
            .into_iter()
            .find(|search| search.id == id))
    }

    pub(crate) async fn known_job(&self, source_id: &str) -> Result<Option<KnownJob>, DbError> {
        let row = sqlx::query("SELECT id, description FROM jobs WHERE source_id=$1 AND user_id=$2")
            .bind(source_id)
            .bind(self.user_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(KnownJob {
                id: row.try_get("id")?,
                description: row.try_get("description")?,
            })
        })
        .transpose()
    }

    pub async fn upsert_job(
        &self,
        card: &JobCard,
        detail: Option<&JobDetail>,
        work_mode: Option<WorkMode>,
    ) -> Result<i64, DbError> {
        let ts = now();
        Ok(sqlx::query_scalar("INSERT INTO jobs (user_id, source, source_id, title, company, company_url, location, posted_on, url, description, seniority, employment_type, industries, work_mode, first_seen_at, updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$15) ON CONFLICT (user_id, source_id) DO UPDATE SET title=EXCLUDED.title, company=EXCLUDED.company, company_url=EXCLUDED.company_url, location=EXCLUDED.location, posted_on=COALESCE(EXCLUDED.posted_on,jobs.posted_on), url=EXCLUDED.url, description=COALESCE(EXCLUDED.description,jobs.description), seniority=COALESCE(EXCLUDED.seniority,jobs.seniority), employment_type=COALESCE(EXCLUDED.employment_type,jobs.employment_type), industries=COALESCE(EXCLUDED.industries,jobs.industries), work_mode=EXCLUDED.work_mode, updated_at=EXCLUDED.updated_at RETURNING id")
            .bind(self.user_id).bind(card.source.as_str()).bind(&card.source_id).bind(&card.title).bind(&card.company).bind(&card.company_url).bind(&card.location).bind(&card.posted_on).bind(&card.url).bind(detail.map(|d| &d.description)).bind(detail.and_then(|d| d.seniority.as_ref())).bind(detail.and_then(|d| d.employment_type.as_ref())).bind(detail.and_then(|d| d.industries.as_ref())).bind(work_mode.map(WorkMode::as_str)).bind(ts).fetch_one(&self.pool).await?)
    }

    pub async fn add_match(
        &self,
        search_id: i64,
        job_id: i64,
        keywords: &[String],
    ) -> Result<bool, DbError> {
        let mut tx = self.pool.begin().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM search_matches m JOIN jobs j ON j.id=m.job_id JOIN job_searches q ON q.id=m.search_id WHERE m.search_id=$1 AND m.job_id=$2 AND j.user_id=$3 AND q.user_id=$3)")
            .bind(search_id).bind(job_id).bind(self.user_id).fetch_one(&mut *tx).await?;
        let inserted = sqlx::query("INSERT INTO search_matches (search_id, job_id, matched_keywords, matched_at) SELECT $1,$2,$3,$4 WHERE EXISTS (SELECT 1 FROM jobs WHERE id=$2 AND user_id=$5) AND EXISTS (SELECT 1 FROM job_searches WHERE id=$1 AND user_id=$5) ON CONFLICT (search_id, job_id) DO UPDATE SET matched_keywords=EXCLUDED.matched_keywords")
            .bind(search_id).bind(job_id).bind(serde_json::to_string(keywords)?).bind(now()).bind(self.user_id).execute(&mut *tx).await?.rows_affected();
        tx.commit().await?;
        Ok(!exists && inserted > 0)
    }

    pub async fn remove_match(&self, search_id: i64, job_id: i64) -> Result<bool, DbError> {
        Ok(sqlx::query("DELETE FROM search_matches m USING jobs j, job_searches q WHERE m.job_id=j.id AND m.search_id=q.id AND m.search_id=$1 AND m.job_id=$2 AND j.user_id=$3 AND q.user_id=$3")
            .bind(search_id).bind(job_id).bind(self.user_id).execute(&self.pool).await?.rows_affected() > 0)
    }

    pub async fn current_confirmed_profile(&self) -> Result<Option<ProfileRecord>, DbError> {
        let Some(resume) = self.get_resume().await? else {
            return Ok(None);
        };
        Ok(self
            .latest_profile(&resume.uploaded_at)
            .await?
            .filter(|record| record.status == "confirmed" && record.profile.is_some()))
    }

    pub async fn companies_missing_site(&self, limit: usize) -> Result<Vec<String>, DbError> {
        Ok(sqlx::query_scalar("SELECT j.company_url FROM jobs j JOIN search_matches m ON m.job_id=j.id JOIN job_searches q ON q.id=m.search_id LEFT JOIN companies c ON c.user_id=j.user_id AND c.url=j.company_url WHERE j.user_id=$1 AND q.user_id=$1 AND j.source='linkedin' AND j.company_url IS NOT NULL AND c.url IS NULL GROUP BY j.company_url ORDER BY BOOL_OR(j.status='saved') DESC, MAX(j.first_seen_at) DESC LIMIT $2")
            .bind(self.user_id).bind(limit as i64).fetch_all(&self.pool).await?)
    }

    pub async fn save_company_site(&self, url: &str, website: Option<&str>) -> Result<(), DbError> {
        sqlx::query("INSERT INTO companies (user_id,url,website,fetched_at) VALUES ($1,$2,$3,$4) ON CONFLICT (user_id,url) DO UPDATE SET website=EXCLUDED.website,fetched_at=EXCLUDED.fetched_at")
            .bind(self.user_id).bind(url).bind(website).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn next_job_to_score(&self, profile_id: i64) -> Result<Option<JobToScore>, DbError> {
        let row=sqlx::query("SELECT j.id,j.title,j.company,j.description FROM jobs j LEFT JOIN job_scores s ON s.job_id=j.id WHERE j.user_id=$1 AND j.status='saved' AND j.description IS NOT NULL AND (s.job_id IS NULL OR (s.profile_id<>$2 AND s.status NOT IN ('scoring','skipped'))) ORDER BY EXISTS (SELECT 1 FROM search_matches m JOIN job_searches q ON q.id=m.search_id WHERE m.job_id=j.id AND q.enabled=TRUE AND q.user_id=$1) DESC,j.updated_at DESC,j.id DESC LIMIT 1")
            .bind(self.user_id).bind(profile_id).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(JobToScore {
                job_id: row.try_get("id")?,
                title: row.try_get("title")?,
                company: row.try_get("company")?,
                description: row.try_get("description")?,
            })
        })
        .transpose()
    }

    pub async fn start_score(
        &self,
        job_id: i64,
        profile_id: i64,
        model: &str,
    ) -> Result<(), DbError> {
        sqlx::query("INSERT INTO job_scores (job_id,profile_id,status,model,updated_at) SELECT $1,$2,'scoring',$3,$4 WHERE EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND user_id=$5) ON CONFLICT(job_id) DO UPDATE SET profile_id=EXCLUDED.profile_id,status='scoring',score=NULL,summary=NULL,requirements='[]',model=EXCLUDED.model,error=NULL,updated_at=EXCLUDED.updated_at")
            .bind(job_id).bind(profile_id).bind(model).bind(now()).bind(self.user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn finish_score(
        &self,
        job_id: i64,
        score: u8,
        summary: &str,
        requirements: &[Requirement],
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE job_scores s SET status='ready',score=$3,summary=$4,requirements=$5,error=NULL,updated_at=$6 FROM jobs j WHERE s.job_id=j.id AND j.user_id=$1 AND s.job_id=$2 AND s.status='scoring'")
            .bind(self.user_id).bind(job_id).bind(score as i64).bind(summary).bind(serde_json::to_string(requirements)?).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn fail_score(&self, job_id: i64, error: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE job_scores s SET status='failed',error=$3,updated_at=$4 FROM jobs j WHERE s.job_id=j.id AND j.user_id=$1 AND s.job_id=$2 AND s.status='scoring'")
            .bind(self.user_id).bind(job_id).bind(error).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn skip_score(&self, job_id: i64, profile_id: i64) -> Result<(), DbError> {
        sqlx::query("INSERT INTO job_scores (job_id,profile_id,status,model,updated_at) SELECT $1,$2,'skipped','',$3 WHERE EXISTS(SELECT 1 FROM jobs WHERE id=$1 AND user_id=$4) ON CONFLICT(job_id) DO UPDATE SET status='skipped',score=NULL,summary=NULL,requirements='[]',error=NULL,updated_at=EXCLUDED.updated_at")
            .bind(job_id).bind(profile_id).bind(now()).bind(self.user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn score_status(&self, job_id: i64) -> Result<Option<String>, DbError> {
        Ok(sqlx::query_scalar("SELECT s.status FROM job_scores s JOIN jobs j ON j.id=s.job_id WHERE s.job_id=$1 AND j.user_id=$2").bind(job_id).bind(self.user_id).fetch_optional(&self.pool).await?)
    }

    pub async fn clear_score(&self, job_id: i64) -> Result<(), DbError> {
        sqlx::query("DELETE FROM job_scores s USING jobs j WHERE s.job_id=j.id AND s.job_id=$1 AND j.user_id=$2").bind(job_id).bind(self.user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn job_status(&self, job_id: i64) -> Result<Option<JobStatus>, DbError> {
        Ok(
            sqlx::query_scalar::<_, String>("SELECT status FROM jobs WHERE id=$1 AND user_id=$2")
                .bind(job_id)
                .bind(self.user_id)
                .fetch_optional(&self.pool)
                .await?
                .as_deref()
                .map(JobStatus::parse),
        )
    }

    pub async fn requeue_interrupted_scores(&self) -> Result<(), DbError> {
        sqlx::query("DELETE FROM job_scores s USING jobs j WHERE s.job_id=j.id AND j.user_id=$1 AND s.status='scoring'").bind(self.user_id).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn list_jobs(&self, status: Option<JobStatus>) -> Result<Vec<JobListing>, DbError> {
        let rows=sqlx::query("SELECT j.id,j.source_id,j.title,j.company,j.company_url,c.website AS company_website,j.location,j.posted_on,j.url,j.description,j.seniority,j.employment_type,j.industries,j.work_mode,j.status,j.first_seen_at,m.search_id,m.matched_keywords,j.tailored_at,s.status AS score_status,s.score,s.summary,s.requirements,s.model,s.error,s.profile_id,s.updated_at AS score_updated_at,j.source FROM jobs j JOIN search_matches m ON m.job_id=j.id JOIN job_searches q ON q.id=m.search_id LEFT JOIN job_scores s ON s.job_id=j.id LEFT JOIN companies c ON c.user_id=j.user_id AND c.url=j.company_url WHERE j.user_id=$1 AND q.user_id=$1 AND ($2::text IS NULL OR j.status=$2) ORDER BY COALESCE(j.posted_on,'') DESC,j.first_seen_at DESC,j.id DESC,m.search_id")
            .bind(self.user_id).bind(status.map(JobStatus::as_str)).fetch_all(&self.pool).await?;
        let mut jobs: Vec<JobListing> = Vec::new();
        let mut indexes: HashMap<i64, usize> = HashMap::new();
        for row in rows {
            let id: i64 = row.try_get("id")?;
            let keywords: Vec<String> =
                serde_json::from_str(&row.try_get::<String, _>("matched_keywords")?)?;
            let score_status: Option<String> = row.try_get("score_status")?;
            let score = if let Some(status) = score_status {
                Some(JobScore {
                    status,
                    score: row
                        .try_get::<Option<i64>, _>("score")?
                        .map(|v| v.clamp(0, 100) as u8),
                    summary: row.try_get("summary")?,
                    requirements: serde_json::from_str(&row.try_get::<String, _>("requirements")?)?,
                    model: row.try_get("model")?,
                    error: row.try_get("error")?,
                    profile_id: row.try_get("profile_id")?,
                    updated_at: row.try_get("score_updated_at")?,
                })
            } else {
                None
            };
            let listing = JobListing {
                id,
                source: Site::parse(&row.try_get::<String, _>("source")?),
                source_id: row.try_get("source_id")?,
                title: row.try_get("title")?,
                company: row.try_get("company")?,
                company_url: row.try_get("company_url")?,
                company_website: row.try_get("company_website")?,
                location: row.try_get("location")?,
                posted_on: row.try_get("posted_on")?,
                url: row.try_get("url")?,
                description: row.try_get("description")?,
                seniority: row.try_get("seniority")?,
                employment_type: row.try_get("employment_type")?,
                industries: row.try_get("industries")?,
                work_mode: row
                    .try_get::<Option<String>, _>("work_mode")?
                    .as_deref()
                    .and_then(WorkMode::parse),
                status: JobStatus::parse(&row.try_get::<String, _>("status")?),
                first_seen_at: row.try_get("first_seen_at")?,
                tailored_at: row.try_get("tailored_at")?,
                search_ids: vec![row.try_get("search_id")?],
                matched_keywords: Vec::new(),
                score,
            };
            if let Some(&i) = indexes.get(&id) {
                let job = &mut jobs[i];
                job.search_ids.extend(listing.search_ids);
                for keyword in keywords {
                    if !job
                        .matched_keywords
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(&keyword))
                    {
                        job.matched_keywords.push(keyword);
                    }
                }
            } else {
                let mut listing = listing;
                listing.matched_keywords = keywords;
                indexes.insert(id, jobs.len());
                jobs.push(listing);
            }
        }
        Ok(jobs)
    }

    pub async fn set_job_status(&self, id: i64, status: JobStatus) -> Result<bool, DbError> {
        Ok(
            sqlx::query("UPDATE jobs SET status=$3,updated_at=$4 WHERE id=$1 AND user_id=$2")
                .bind(id)
                .bind(self.user_id)
                .bind(status.as_str())
                .bind(now())
                .execute(&self.pool)
                .await?
                .rows_affected()
                > 0,
        )
    }

    pub async fn set_jobs_status(&self, ids: &[i64], status: JobStatus) -> Result<usize, DbError> {
        Ok(
            sqlx::query("UPDATE jobs SET status=$3,updated_at=$4 WHERE id=ANY($1) AND user_id=$2")
                .bind(ids)
                .bind(self.user_id)
                .bind(status.as_str())
                .bind(now())
                .execute(&self.pool)
                .await?
                .rows_affected() as usize,
        )
    }

    pub async fn get_setting(&self, key: &str) -> Result<Option<String>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT value FROM settings WHERE user_id=$1 AND key=$2")
                .bind(self.user_id)
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<(), DbError> {
        sqlx::query("INSERT INTO settings (user_id,key,value) VALUES ($1,$2,$3) ON CONFLICT(user_id,key) DO UPDATE SET value=EXCLUDED.value").bind(self.user_id).bind(key).bind(value).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn delete_setting(&self, key: &str) -> Result<(), DbError> {
        sqlx::query("DELETE FROM settings WHERE user_id=$1 AND key=$2")
            .bind(self.user_id)
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn start_scan_run(&self, site: Site) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar("INSERT INTO scan_runs (user_id,started_at,status,site) VALUES ($1,$2,'running',$3) RETURNING id").bind(self.user_id).bind(now()).bind(site.as_str()).fetch_one(&self.pool).await?)
    }

    pub async fn finish_scan_run(
        &self,
        id: i64,
        status: &str,
        new_matches: usize,
        message: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE scan_runs SET finished_at=$3,status=$4,new_matches=$5,message=$6 WHERE id=$1 AND user_id=$2").bind(id).bind(self.user_id).bind(now()).bind(status).bind(new_matches as i64).bind(message).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn last_scan_run(&self, site: Site) -> Result<Option<ScanRun>, DbError> {
        let row=sqlx::query("SELECT id,started_at,finished_at,status,new_matches,message FROM scan_runs WHERE user_id=$1 AND site=$2 ORDER BY id DESC LIMIT 1").bind(self.user_id).bind(site.as_str()).fetch_optional(&self.pool).await?;
        row.map(|row| {
            Ok(ScanRun {
                id: row.try_get("id")?,
                started_at: row.try_get("started_at")?,
                finished_at: row.try_get("finished_at")?,
                status: row.try_get("status")?,
                new_matches: row.try_get("new_matches")?,
                message: row.try_get("message")?,
            })
        })
        .transpose()
    }

    pub async fn mark_interrupted_scan_runs(&self) -> Result<(), DbError> {
        sqlx::query("UPDATE scan_runs SET status='interrupted',finished_at=$2 WHERE user_id=$1 AND status='running'").bind(self.user_id).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn start_profile(
        &self,
        resume_uploaded_at: &str,
        model: &str,
    ) -> Result<i64, DbError> {
        Ok(sqlx::query_scalar("INSERT INTO resume_profiles (user_id,resume_uploaded_at,status,model,created_at,updated_at) SELECT $1,$2,'reading',$3,$4,$4 WHERE EXISTS(SELECT 1 FROM master_resume WHERE user_id=$1) RETURNING id").bind(self.user_id).bind(resume_uploaded_at).bind(model).bind(now()).fetch_one(&self.pool).await?)
    }

    pub async fn finish_profile(
        &self,
        id: i64,
        profile: &ResumeProfile,
        warnings: &[Warning],
    ) -> Result<(), DbError> {
        sqlx::query("UPDATE resume_profiles SET status='ready',data=$3,warnings=$4,error=NULL,updated_at=$5 WHERE id=$1 AND user_id=$2").bind(id).bind(self.user_id).bind(serde_json::to_string(profile)?).bind(serde_json::to_string(warnings)?).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    pub async fn fail_profile(&self, id: i64, error: &str) -> Result<(), DbError> {
        sqlx::query("UPDATE resume_profiles SET status='failed',error=$3,updated_at=$4 WHERE id=$1 AND user_id=$2").bind(id).bind(self.user_id).bind(error).bind(now()).execute(&self.pool).await?;
        Ok(())
    }

    async fn profile_by_query(
        &self,
        query: &str,
        bind_value: &str,
    ) -> Result<Option<ProfileRecord>, DbError> {
        let row = sqlx::query(query)
            .bind(bind_value)
            .bind(self.user_id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            let data: Option<String> = row.try_get("data")?;
            let warnings: String = row.try_get("warnings")?;
            Ok(ProfileRecord {
                id: row.try_get("id")?,
                resume_uploaded_at: row.try_get("resume_uploaded_at")?,
                status: row.try_get("status")?,
                model: row.try_get("model")?,
                profile: data.map(|value| serde_json::from_str(&value)).transpose()?,
                warnings: serde_json::from_str(&warnings)?,
                error: row.try_get("error")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                confirmed_at: row.try_get("confirmed_at")?,
            })
        })
        .transpose()
    }

    pub async fn latest_profile(
        &self,
        resume_uploaded_at: &str,
    ) -> Result<Option<ProfileRecord>, DbError> {
        self.profile_by_query(&format!("SELECT {PROFILE_COLUMNS} FROM resume_profiles WHERE resume_uploaded_at=$1 AND user_id=$2 ORDER BY id DESC LIMIT 1"),resume_uploaded_at).await
    }

    pub async fn get_profile(&self, id: i64) -> Result<Option<ProfileRecord>, DbError> {
        let row = sqlx::query(&format!(
            "SELECT {PROFILE_COLUMNS} FROM resume_profiles WHERE id=$1 AND user_id=$2"
        ))
        .bind(id)
        .bind(self.user_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let data: Option<String> = row.try_get("data")?;
            let warnings: String = row.try_get("warnings")?;
            Ok(ProfileRecord {
                id: row.try_get("id")?,
                resume_uploaded_at: row.try_get("resume_uploaded_at")?,
                status: row.try_get("status")?,
                model: row.try_get("model")?,
                profile: data.map(|value| serde_json::from_str(&value)).transpose()?,
                warnings: serde_json::from_str(&warnings)?,
                error: row.try_get("error")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                confirmed_at: row.try_get("confirmed_at")?,
            })
        })
        .transpose()
    }

    pub async fn save_profile_edits(
        &self,
        id: i64,
        profile: &ResumeProfile,
        warnings: &[Warning],
    ) -> Result<bool, DbError> {
        Ok(sqlx::query("UPDATE resume_profiles SET status='ready',data=$3,warnings=$4,confirmed_at=NULL,updated_at=$5 WHERE id=$1 AND user_id=$2 AND status IN ('ready','confirmed')").bind(id).bind(self.user_id).bind(serde_json::to_string(profile)?).bind(serde_json::to_string(warnings)?).bind(now()).execute(&self.pool).await?.rows_affected()>0)
    }

    pub async fn confirm_profile(&self, id: i64) -> Result<bool, DbError> {
        Ok(sqlx::query("UPDATE resume_profiles SET status='confirmed',confirmed_at=$3,updated_at=$3 WHERE id=$1 AND user_id=$2 AND status='ready'").bind(id).bind(self.user_id).bind(now()).execute(&self.pool).await?.rows_affected()>0)
    }

    pub async fn mark_interrupted_profiles(&self) -> Result<(), DbError> {
        sqlx::query("UPDATE resume_profiles SET status='failed',error='Interrupted by a restart. Try again.',updated_at=$2 WHERE user_id=$1 AND status='reading'").bind(self.user_id).bind(now()).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    async fn user_id(pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users (created_at) VALUES ('2026-01-01T00:00:00Z') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn users_do_not_share_jobs_searches_or_settings(pool: PgPool) {
        let first = Db::for_user(pool.clone(), user_id(&pool).await);
        let second = Db::for_user(pool.clone(), user_id(&pool).await);
        let criteria = SearchCriteria {
            name: "Backend Engineer · Dubai".into(),
            titles: vec!["Backend Engineer".into()],
            keywords: vec!["Rust".into()],
            location: Some("Dubai".into()),
            work_modes: vec![WorkMode::Onsite],
            posted_within: PostedWithin::Week,
        };
        let search = first.create_search(&criteria).await.unwrap();
        let card = JobCard {
            source: Site::LinkedIn,
            source_id: "job-1".into(),
            title: "Backend Engineer".into(),
            company: "Acme".into(),
            company_url: None,
            location: "Dubai".into(),
            posted_on: None,
            url: "https://example.com/job-1".into(),
        };
        let job_id = first
            .upsert_job(&card, None, Some(WorkMode::Onsite))
            .await
            .unwrap();
        first
            .add_match(search.id, job_id, &["Rust".into()])
            .await
            .unwrap();
        first.set_setting("private", "first-account").await.unwrap();

        assert_eq!(second.list_searches().await.unwrap().len(), 0);
        assert_eq!(second.list_jobs(None).await.unwrap().len(), 0);
        assert_eq!(second.get_setting("private").await.unwrap(), None);
        assert!(!second.add_match(search.id, job_id, &[]).await.unwrap());
        assert!(!second.remove_match(search.id, job_id).await.unwrap());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn resume_pdf_is_stored_and_deleted_with_its_record(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let info = PdfInfo {
            page_count: 1,
            text: "Resume text".into(),
        };
        store
            .save_resume("resume.pdf", b"%PDF-content", &info)
            .await
            .unwrap();
        assert_eq!(
            store.resume_pdf().await.unwrap().as_deref(),
            Some(&b"%PDF-content"[..])
        );
        assert!(store.delete_resume().await.unwrap());
        assert_eq!(store.resume_pdf().await.unwrap(), None);
    }
}
