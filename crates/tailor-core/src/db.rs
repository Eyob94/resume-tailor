use std::collections::HashMap;

use chrono::Utc;
use sqlx::{postgres::PgRow, PgPool, Row};

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

    pub async fn resume_pdf(&self) -> Result<Option<(String, Vec<u8>)>, DbError> {
        Ok(
            sqlx::query_as("SELECT file_name, pdf FROM master_resume WHERE user_id = $1")
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
        let inserted = sqlx::query_scalar::<_, bool>("INSERT INTO search_matches (search_id, job_id, matched_keywords, matched_at) SELECT $1, $2, $3, $4 WHERE EXISTS (SELECT 1 FROM jobs WHERE id = $2 AND user_id = $5) AND EXISTS (SELECT 1 FROM job_searches WHERE id = $1 AND user_id = $5) ON CONFLICT (search_id, job_id) DO UPDATE SET matched_keywords = EXCLUDED.matched_keywords RETURNING (xmax = 0) AS inserted")
            .bind(search_id).bind(job_id).bind(serde_json::to_string(keywords)?).bind(now()).bind(self.user_id).fetch_optional(&self.pool).await?;
        Ok(inserted.unwrap_or(false))
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
            sqlx::query("UPDATE jobs SET status=$3,updated_at=$4 WHERE id=ANY($1) AND user_id=$2 AND status<>$3")
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
    ) -> Result<Option<i64>, DbError> {
        Ok(sqlx::query_scalar("INSERT INTO resume_profiles (user_id,resume_uploaded_at,status,model,created_at,updated_at) SELECT $1,$2,'reading',$3,$4,$4 WHERE EXISTS(SELECT 1 FROM master_resume WHERE user_id=$1) RETURNING id").bind(self.user_id).bind(resume_uploaded_at).bind(model).bind(now()).fetch_optional(&self.pool).await?)
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

    fn profile_from_row(row: &PgRow) -> Result<ProfileRecord, DbError> {
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
        row.as_ref().map(Self::profile_from_row).transpose()
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
        row.as_ref().map(Self::profile_from_row).transpose()
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
    use crate::types::{Contact, Importance, SearchInput, Verdict};
    use sqlx::PgPool;

    async fn user_id(pool: &PgPool) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users (created_at) VALUES ('2026-01-01T00:00:00Z') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap()
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
    async fn save_replace_and_delete(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        assert!(store.get_resume().await.unwrap().is_none());

        let info = PdfInfo {
            page_count: 1,
            text: "first".into(),
        };
        store.save_resume("a.pdf", b"%PDF-1", &info).await.unwrap();
        let info = PdfInfo {
            page_count: 2,
            text: "second".into(),
        };
        store.save_resume("b.pdf", b"%PDF-22", &info).await.unwrap();

        let resume = store.get_resume().await.unwrap().unwrap();
        assert_eq!(resume.file_name, "b.pdf");
        assert_eq!(resume.page_count, 2);
        assert_eq!(
            store.resume_pdf().await.unwrap(),
            Some(("b.pdf".to_string(), b"%PDF-22".to_vec()))
        );

        assert!(store.delete_resume().await.unwrap());
        assert!(store.get_resume().await.unwrap().is_none());
        assert_eq!(store.resume_pdf().await.unwrap(), None);
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
            store.resume_pdf().await.unwrap(),
            Some(("resume.pdf".to_string(), b"%PDF-content".to_vec()))
        );
        assert!(store.delete_resume().await.unwrap());
        assert_eq!(store.resume_pdf().await.unwrap(), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn search_crud(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let search = store
            .create_search(&criteria("Backend Engineer"))
            .await
            .unwrap();
        assert_eq!(search.criteria.titles, vec!["Backend Engineer"]);

        let updated = store
            .update_search(search.id, &criteria("Platform Engineer"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.criteria.titles, vec!["Platform Engineer"]);
        assert!(store
            .update_search(999, &criteria("x"))
            .await
            .unwrap()
            .is_none());

        assert!(store.delete_search(search.id).await.unwrap());
        assert!(store.list_searches().await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn scoring_queue_picks_saved_jobs_for_the_confirmed_profile(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        assert!(store.current_confirmed_profile().await.unwrap().is_none());

        let resume = store
            .save_resume(
                "cv.pdf",
                b"%PDF-1",
                &PdfInfo {
                    page_count: 1,
                    text: "cv".into(),
                },
            )
            .await
            .unwrap();
        let profile_id = store
            .start_profile(&resume.uploaded_at, "fake")
            .await
            .unwrap()
            .unwrap();
        let profile = ResumeProfile::default();
        store
            .finish_profile(profile_id, &profile, &[])
            .await
            .unwrap();
        assert!(store.current_confirmed_profile().await.unwrap().is_none());
        store.confirm_profile(profile_id).await.unwrap();
        assert_eq!(
            store.current_confirmed_profile().await.unwrap().unwrap().id,
            profile_id
        );

        let detail = JobDetail {
            description: "Rust".into(),
            seniority: None,
            employment_type: None,
            industries: None,
        };
        let job = store
            .upsert_job(&card("1"), Some(&detail), None)
            .await
            .unwrap();
        let undescribed = store.upsert_job(&card("2"), None, None).await.unwrap();
        store
            .set_jobs_status(&[job, undescribed], JobStatus::Saved)
            .await
            .unwrap();
        assert_eq!(
            store
                .next_job_to_score(profile_id)
                .await
                .unwrap()
                .unwrap()
                .job_id,
            job
        );

        store.start_score(job, profile_id, "fake").await.unwrap();
        assert!(store.next_job_to_score(profile_id).await.unwrap().is_none());
        assert!(store
            .next_job_to_score(profile_id + 1)
            .await
            .unwrap()
            .is_none());

        let reqs = vec![Requirement {
            text: "Rust".into(),
            importance: Importance::Must,
            verdict: Verdict::Met,
            evidence: None,
        }];
        store
            .finish_score(job, 80, "Good fit.", &reqs)
            .await
            .unwrap();
        assert!(store.next_job_to_score(profile_id).await.unwrap().is_none());
        assert_eq!(
            store
                .next_job_to_score(profile_id + 1)
                .await
                .unwrap()
                .unwrap()
                .job_id,
            job
        );

        let search = store
            .create_search(&criteria("Backend Engineer"))
            .await
            .unwrap();
        store.add_match(search.id, job, &[]).await.unwrap();
        let listed = store.list_jobs(None).await.unwrap();
        let score = listed[0].score.as_ref().unwrap();
        assert_eq!(
            (score.status.as_str(), score.score, score.requirements.len()),
            ("ready", Some(80), 1)
        );

        store.clear_score(job).await.unwrap();
        assert_eq!(
            store
                .next_job_to_score(profile_id)
                .await
                .unwrap()
                .unwrap()
                .job_id,
            job
        );
        store.set_job_status(job, JobStatus::New).await.unwrap();
        assert!(store.next_job_to_score(profile_id).await.unwrap().is_none());

        store.set_job_status(job, JobStatus::Saved).await.unwrap();
        store.start_score(job, profile_id, "fake").await.unwrap();
        assert!(store.next_job_to_score(profile_id).await.unwrap().is_none());
        store.skip_score(job, profile_id).await.unwrap();
        store
            .finish_score(job, 90, "late result", &[])
            .await
            .unwrap();
        assert_eq!(
            store.score_status(job).await.unwrap().as_deref(),
            Some("skipped")
        );
        assert!(store.next_job_to_score(profile_id).await.unwrap().is_none());
        assert!(store
            .next_job_to_score(profile_id + 1)
            .await
            .unwrap()
            .is_none());
        store.clear_score(job).await.unwrap();
        store.start_score(job, profile_id, "fake").await.unwrap();
        store.requeue_interrupted_scores().await.unwrap();
        assert_eq!(store.score_status(job).await.unwrap(), None);
        assert_eq!(
            store
                .next_job_to_score(profile_id)
                .await
                .unwrap()
                .unwrap()
                .job_id,
            job
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn scoring_queue_puts_enabled_preferences_first(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let detail = JobDetail {
            description: "Rust".into(),
            seniority: None,
            employment_type: None,
            industries: None,
        };
        let paused_job = store
            .upsert_job(&card("1"), Some(&detail), None)
            .await
            .unwrap();
        let enabled_job = store
            .upsert_job(&card("2"), Some(&detail), None)
            .await
            .unwrap();
        store
            .set_jobs_status(&[enabled_job], JobStatus::Saved)
            .await
            .unwrap();
        store
            .set_jobs_status(&[paused_job], JobStatus::Saved)
            .await
            .unwrap();

        let paused = store
            .create_search(&criteria("Backend Engineer"))
            .await
            .unwrap();
        let enabled = store
            .create_search(&criteria("Platform Engineer"))
            .await
            .unwrap();
        store.set_search_enabled(paused.id, false).await.unwrap();
        store.add_match(paused.id, paused_job, &[]).await.unwrap();
        store.add_match(enabled.id, enabled_job, &[]).await.unwrap();

        assert_eq!(
            store.next_job_to_score(1).await.unwrap().unwrap().job_id,
            enabled_job
        );
        store.set_search_enabled(enabled.id, false).await.unwrap();
        assert_eq!(
            store.next_job_to_score(1).await.unwrap().unwrap().job_id,
            paused_job
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_keeps_description_and_matches_fold(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let a = store
            .create_search(&criteria("Backend Engineer"))
            .await
            .unwrap();
        let b = store.create_search(&criteria("Engineer")).await.unwrap();

        let detail = JobDetail {
            description: "Rust and Kafka".into(),
            seniority: Some("Mid-Senior level".into()),
            employment_type: None,
            industries: None,
        };
        let id = store
            .upsert_job(&card("1"), Some(&detail), Some(WorkMode::Hybrid))
            .await
            .unwrap();
        assert_eq!(
            store
                .upsert_job(&card("1"), None, Some(WorkMode::Hybrid))
                .await
                .unwrap(),
            id
        );
        assert_eq!(
            store
                .known_job("1")
                .await
                .unwrap()
                .unwrap()
                .description
                .as_deref(),
            Some("Rust and Kafka")
        );

        assert!(store.add_match(a.id, id, &["Rust".into()]).await.unwrap());
        assert!(!store.add_match(a.id, id, &["Rust".into()]).await.unwrap());
        assert!(store.add_match(b.id, id, &["Kafka".into()]).await.unwrap());

        let jobs = store.list_jobs(None).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].search_ids, vec![a.id, b.id]);
        assert_eq!(jobs[0].matched_keywords, vec!["Rust", "Kafka"]);
        assert_eq!(jobs[0].work_mode, Some(WorkMode::Hybrid));

        assert_eq!(jobs[0].tailored_at, None);
        assert_eq!(
            store
                .set_jobs_status(&[id, 999], JobStatus::Saved)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .set_jobs_status(&[id], JobStatus::Saved)
                .await
                .unwrap(),
            0,
            "already saved"
        );
        assert!(store
            .set_job_status(id, JobStatus::Dismissed)
            .await
            .unwrap());
        assert!(store
            .list_jobs(Some(JobStatus::New))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            store
                .list_jobs(Some(JobStatus::Dismissed))
                .await
                .unwrap()
                .len(),
            1
        );

        store
            .update_search(a.id, &criteria("Backend Engineer"))
            .await
            .unwrap();
        assert_eq!(
            store.list_jobs(None).await.unwrap()[0].search_ids,
            vec![a.id, b.id]
        );

        store.set_job_status(id, JobStatus::New).await.unwrap();
        store
            .update_search(a.id, &criteria("Backend Engineer"))
            .await
            .unwrap();
        assert_eq!(
            store.list_jobs(None).await.unwrap()[0].search_ids,
            vec![b.id]
        );
        store.delete_search(b.id).await.unwrap();
        assert!(store.list_jobs(None).await.unwrap().is_empty());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn settings_and_scan_runs(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        assert_eq!(store.get_setting("k").await.unwrap(), None);
        store.set_setting("k", "30").await.unwrap();
        store.set_setting("k", "45").await.unwrap();
        assert_eq!(store.get_setting("k").await.unwrap().as_deref(), Some("45"));

        let run = store.start_scan_run(Site::LinkedIn).await.unwrap();
        store.mark_interrupted_scan_runs().await.unwrap();
        assert_eq!(
            store
                .last_scan_run(Site::LinkedIn)
                .await
                .unwrap()
                .unwrap()
                .status,
            "interrupted"
        );
        store.finish_scan_run(run, "ok", 3, None).await.unwrap();
        let last = store.last_scan_run(Site::LinkedIn).await.unwrap().unwrap();
        assert_eq!((last.status.as_str(), last.new_matches), ("ok", 3));
        assert!(store.last_scan_run(Site::Indeed).await.unwrap().is_none());
        let indeed = store.start_scan_run(Site::Indeed).await.unwrap();
        assert_eq!(
            store.last_scan_run(Site::Indeed).await.unwrap().unwrap().id,
            indeed
        );
        assert_eq!(
            store
                .last_scan_run(Site::LinkedIn)
                .await
                .unwrap()
                .unwrap()
                .id,
            run
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn delete_setting_removes_saved_value(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        store.set_setting("k", "value").await.unwrap();
        store.delete_setting("k").await.unwrap();
        assert_eq!(store.get_setting("k").await.unwrap(), None);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn reading_lifecycle(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let resume = store
            .save_resume(
                "cv.pdf",
                b"%PDF-1",
                &PdfInfo {
                    page_count: 1,
                    text: "cv".into(),
                },
            )
            .await
            .unwrap();
        let uploaded_at = resume.uploaded_at.clone();

        let id = store
            .start_profile(&uploaded_at, "fake")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store
                .latest_profile(&uploaded_at)
                .await
                .unwrap()
                .unwrap()
                .status,
            "reading"
        );
        assert!(store.latest_profile("t2").await.unwrap().is_none());
        assert!(!store.confirm_profile(id).await.unwrap());

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
        store.finish_profile(id, &profile, &warnings).await.unwrap();
        let record = store.latest_profile(&uploaded_at).await.unwrap().unwrap();
        assert_eq!(
            (record.status.as_str(), record.warnings.len()),
            ("ready", 1)
        );
        assert_eq!(record.profile.unwrap().contact.name, "Jordan");

        assert!(store.confirm_profile(id).await.unwrap());
        assert!(store
            .get_profile(id)
            .await
            .unwrap()
            .unwrap()
            .confirmed_at
            .is_some());

        assert!(store.save_profile_edits(id, &profile, &[]).await.unwrap());
        let record = store.get_profile(id).await.unwrap().unwrap();
        assert_eq!(
            (record.status.as_str(), record.confirmed_at),
            ("ready", None)
        );

        let other = store
            .start_profile(&uploaded_at, "fake")
            .await
            .unwrap()
            .unwrap();
        store.mark_interrupted_profiles().await.unwrap();
        let record = store.latest_profile(&uploaded_at).await.unwrap().unwrap();
        assert_eq!((record.id, record.status.as_str()), (other, "failed"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn company_sites_are_fetched_once_and_listed(pool: PgPool) {
        let store = Db::for_user(pool.clone(), user_id(&pool).await);
        let company = "https://ae.linkedin.com/company/acme";
        let mut with_company = card("1");
        with_company.company_url = Some(company.into());
        let job = store.upsert_job(&with_company, None, None).await.unwrap();
        store.upsert_job(&card("2"), None, None).await.unwrap();
        let search = store
            .create_search(&criteria("Backend Engineer"))
            .await
            .unwrap();
        store.add_match(search.id, job, &[]).await.unwrap();

        assert_eq!(
            store.companies_missing_site(8).await.unwrap(),
            vec![company.to_string()]
        );
        store
            .save_company_site(company, Some("https://acme.com/"))
            .await
            .unwrap();
        assert!(store.companies_missing_site(8).await.unwrap().is_empty());
        let listed = store.list_jobs(None).await.unwrap();
        assert_eq!(
            listed[0].company_website.as_deref(),
            Some("https://acme.com/")
        );
    }
}
