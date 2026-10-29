use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

mod auth;
mod errors;
mod settings;

use axum::{
    extract::{DefaultBodyLimit, Extension, Multipart, Path, Query},
    http::{header, StatusCode},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
    Json, Router,
};
use errors::AppError;

use serde::Deserialize;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tailor_core::{
    db::Db,
    errors::{ProfileError, ResumeError, ScoreError},
    parse_stage::{ParseMessage, DEFAULT_OLLAMA_URL},
    pipeline::{Pipeline, PipelineConfig, PipelineSenders},
    resume_stage::{ResumeMessage, MAX_UPLOAD_BYTES},
    score_stage::ScoreMessage,
    scrape_stage::{ScrapeMessage, DEFAULT_COOLDOWN_SECONDS},
    types::{
        JobListing, JobSearch, JobStatus, MasterResume, ProfileRecord, ResumeProfile, ScanStatus,
        SearchInput, Site,
    },
};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    auth: auth::AuthStore,
    pool: PgPool,
    ollama_url: String,
    cooldown_seconds: i64,
    users: Arc<AsyncMutex<std::collections::HashMap<i64, Arc<UserServices>>>>,
}

struct UserServices {
    user_id: i64,
    store: Arc<Db>,
    senders: PipelineSenders,
    ollama_url: String,
    _pipeline: Pipeline,
}

impl AppState {
    async fn user_services(&self, user_id: i64) -> Result<Arc<UserServices>, String> {
        let mut users = self.users.lock().await;
        if let Some(user) = users.get(&user_id) {
            return Ok(user.clone());
        }
        let store = Arc::new(Db::for_user(self.pool.clone(), user_id));
        let (pipeline, senders) = Pipeline::new(
            store.clone(),
            PipelineConfig {
                ollama_url: self.ollama_url.clone(),
                scan_cooldown_seconds: self.cooldown_seconds,
            },
        )
        .map_err(|error| error.to_string())?;
        let user = Arc::new(UserServices {
            user_id,
            store,
            senders,
            ollama_url: self.ollama_url.clone(),
            _pipeline: pipeline,
        });
        users.insert(user_id, user.clone());
        Ok(user)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let web_dist = PathBuf::from(env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".into()));
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);

    let ollama_url = env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_OLLAMA_URL.into());
    let cooldown_seconds = env::var("SCAN_COOLDOWN_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_COOLDOWN_SECONDS);
    if cooldown_seconds != DEFAULT_COOLDOWN_SECONDS {
        tracing::warn!(cooldown_seconds, "manual scan cooldown overridden");
    }
    let database_url =
        env::var("DATABASE_URL").map_err(|_| anyhow::anyhow!("DATABASE_URL is not set."))?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    sqlx::migrate!("../tailor-core/migrations")
        .run(&pool)
        .await?;
    let state = AppState {
        auth: auth::AuthStore::new(pool.clone()),
        pool,
        ollama_url,
        cooldown_seconds,
        users: Arc::new(AsyncMutex::new(std::collections::HashMap::new())),
    };
    let protected = Router::new()
        .merge(auth::protected_routes())
        .route(
            "/resume",
            get(get_resume).post(upload_resume).delete(delete_resume),
        )
        .route("/resume/file", get(resume_file))
        .route("/resume/profile", get(get_profile).put(update_profile))
        .route("/resume/profile/read", post(read_profile))
        .route("/resume/profile/confirm", post(confirm_profile))
        .merge(settings::routes())
        .route("/searches", get(list_searches).post(create_search))
        .route(
            "/searches/{id}",
            put(update_search)
                .patch(update_search_enabled)
                .delete(delete_search),
        )
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", patch(update_job))
        .route("/jobs/status", post(update_jobs_status))
        .route("/jobs/{id}/score", post(rescore_job).delete(skip_job_score))
        .route("/scan", get(scan_status))
        .route("/scan/{site}/run", post(run_scan_now))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 64 * 1024))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));
    let api = Router::new()
        .route("/health", get(health))
        .merge(auth::public_routes())
        .merge(protected)
        .with_state(state);

    let mut app = Router::new().nest("/api", api);
    if web_dist.join("index.html").exists() {
        let spa = ServeDir::new(&web_dist).fallback(ServeFile::new(web_dist.join("index.html")));
        app = app.fallback_service(spa);
    }
    let app = app.layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn ask<M, T>(
    sender: &UnboundedSender<M>,
    make: impl FnOnce(oneshot::Sender<T>) -> M,
) -> Result<T, AppError> {
    let (tx, rx) = oneshot::channel();
    sender
        .send(make(tx))
        .map_err(|_| AppError::internal("pipeline stopped"))?;
    rx.await.map_err(|_| AppError::internal("pipeline stopped"))
}

fn map_resume_error(error: ResumeError) -> AppError {
    match error {
        ResumeError::TooLarge(_) => AppError::too_large(),
        ResumeError::NoResume => AppError::not_found(),
        ResumeError::Db(error) => error.into(),
        other => AppError::bad_request(other.to_string()),
    }
}

fn map_profile_error(error: ProfileError) -> AppError {
    match error {
        ProfileError::NoResume => AppError::not_found(),
        ProfileError::NotRead => AppError::not_found_msg("Your resume hasn't been read yet."),
        ProfileError::MissingName => AppError::bad_request(error.to_string()),
        ProfileError::AlreadyReading | ProfileError::NotEditable | ProfileError::NotConfirmable => {
            AppError {
                status: StatusCode::CONFLICT,
                message: error.to_string(),
            }
        }
        ProfileError::Db(error) => error.into(),
    }
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn get_resume(
    Extension(user): Extension<Arc<UserServices>>,
) -> Result<Json<Option<MasterResume>>, AppError> {
    Ok(Json(user.store.get_resume().await?))
}

async fn upload_resume(
    Extension(state): Extension<Arc<UserServices>>,
    mut multipart: Multipart,
) -> Result<Json<MasterResume>, AppError> {
    while let Some(field) = multipart.next_field().await.map_err(AppError::multipart)? {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field.file_name().unwrap_or_default().to_string();
        let bytes = field.bytes().await.map_err(AppError::multipart)?.to_vec();
        let resume = ask(&state.senders.resume, |reply| ResumeMessage::Upload {
            file_name,
            bytes,
            reply,
        })
        .await?
        .map_err(map_resume_error)?;
        return Ok(Json(resume));
    }
    Err(AppError::bad_request("No file was uploaded."))
}

async fn delete_resume(
    Extension(state): Extension<Arc<UserServices>>,
) -> Result<StatusCode, AppError> {
    ask(&state.senders.resume, |reply| ResumeMessage::Remove {
        reply,
    })
    .await?
    .map_err(map_resume_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resume_file(Extension(user): Extension<Arc<UserServices>>) -> Result<Response, AppError> {
    let Some((file_name, bytes)) = user.store.resume_pdf().await? else {
        return Err(AppError::not_found());
    };
    let header_name: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let headers = [
        (header::CONTENT_TYPE, "application/pdf".to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{header_name}\""),
        ),
        (header::CACHE_CONTROL, "no-cache".to_string()),
    ];
    Ok((headers, bytes).into_response())
}

async fn get_profile(
    Extension(user): Extension<Arc<UserServices>>,
) -> Result<Json<Option<ProfileRecord>>, AppError> {
    let Some(resume) = user.store.get_resume().await? else {
        return Ok(Json(None));
    };
    Ok(Json(user.store.latest_profile(&resume.uploaded_at).await?))
}

async fn read_profile(
    Extension(state): Extension<Arc<UserServices>>,
) -> Result<StatusCode, AppError> {
    ask(&state.senders.parse, |reply| ParseMessage::ReadAgain {
        reply,
    })
    .await?
    .map_err(map_profile_error)?;
    Ok(StatusCode::ACCEPTED)
}

async fn update_profile(
    Extension(state): Extension<Arc<UserServices>>,
    Json(profile): Json<ResumeProfile>,
) -> Result<Json<ProfileRecord>, AppError> {
    let saved = ask(&state.senders.parse, |reply| ParseMessage::SaveEdits {
        profile,
        reply,
    })
    .await?
    .map_err(map_profile_error)?;
    Ok(Json(saved))
}

async fn confirm_profile(
    Extension(state): Extension<Arc<UserServices>>,
) -> Result<Json<ProfileRecord>, AppError> {
    let saved = ask(&state.senders.parse, |reply| ParseMessage::Confirm {
        reply,
    })
    .await?
    .map_err(map_profile_error)?;
    let _ = state.senders.score.send(ScoreMessage::Kick);
    Ok(Json(saved))
}

async fn list_searches(
    Extension(user): Extension<Arc<UserServices>>,
) -> Result<Json<Vec<JobSearch>>, AppError> {
    Ok(Json(user.store.list_searches().await?))
}

async fn create_search(
    Extension(state): Extension<Arc<UserServices>>,
    Json(input): Json<SearchInput>,
) -> Result<(StatusCode, Json<JobSearch>), AppError> {
    let criteria = input.validate().map_err(|e| AppError::bad_request(e.0))?;
    let search = state.store.create_search(&criteria).await?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok((StatusCode::CREATED, Json(search)))
}

async fn update_search(
    Extension(state): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
    Json(input): Json<SearchInput>,
) -> Result<Json<JobSearch>, AppError> {
    let criteria = input.validate().map_err(|e| AppError::bad_request(e.0))?;
    let search = state
        .store
        .update_search(id, &criteria)
        .await?
        .ok_or_else(|| AppError::not_found_msg("Search not found."))?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(search))
}

#[derive(Deserialize)]
struct SearchEnabledInput {
    enabled: bool,
}

async fn update_search_enabled(
    Extension(state): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
    Json(input): Json<SearchEnabledInput>,
) -> Result<Json<JobSearch>, AppError> {
    let search = state
        .store
        .set_search_enabled(id, input.enabled)
        .await?
        .ok_or_else(|| AppError::not_found_msg("Preference not found."))?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(search))
}

async fn delete_search(
    Extension(state): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if !state.store.delete_search(id).await? {
        return Err(AppError::not_found_msg("Search not found."));
    }
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct JobsQuery {
    status: Option<JobStatus>,
}

async fn list_jobs(
    Extension(user): Extension<Arc<UserServices>>,
    Query(query): Query<JobsQuery>,
) -> Result<Json<Vec<JobListing>>, AppError> {
    Ok(Json(user.store.list_jobs(query.status).await?))
}

#[derive(Deserialize)]
struct JobPatch {
    status: JobStatus,
}

async fn update_job(
    Extension(user): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
    Json(patch): Json<JobPatch>,
) -> Result<StatusCode, AppError> {
    if user.store.set_job_status(id, patch.status).await? {
        if patch.status == JobStatus::Saved {
            let _ = user.senders.score.send(ScoreMessage::Kick);
        }
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found_msg("Job not found."))
    }
}

#[derive(Deserialize)]
struct BulkJobStatus {
    ids: Vec<i64>,
    status: JobStatus,
}

const MAX_BULK_IDS: usize = 2000;

async fn update_jobs_status(
    Extension(user): Extension<Arc<UserServices>>,
    Json(body): Json<BulkJobStatus>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.ids.len() > MAX_BULK_IDS {
        return Err(AppError::bad_request(format!(
            "Update at most {MAX_BULK_IDS} jobs at once."
        )));
    }
    let updated = user.store.set_jobs_status(&body.ids, body.status).await?;
    if body.status == JobStatus::Saved {
        let _ = user.senders.score.send(ScoreMessage::Kick);
    }
    Ok(Json(json!({ "updated": updated })))
}

async fn rescore_job(
    Extension(user): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    ask(&user.senders.score, |reply| ScoreMessage::Rescore {
        job_id: id,
        reply,
    })
    .await?
    .map(|()| StatusCode::ACCEPTED)
    .map_err(map_score_error)
}

async fn skip_job_score(
    Extension(user): Extension<Arc<UserServices>>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    ask(&user.senders.score, |reply| ScoreMessage::Skip {
        job_id: id,
        reply,
    })
    .await?
    .map(|()| StatusCode::NO_CONTENT)
    .map_err(map_score_error)
}

fn map_score_error(error: ScoreError) -> AppError {
    match error {
        ScoreError::NotFound => AppError::not_found_msg("Job not found."),
        ScoreError::NotSaved | ScoreError::NoProfile => AppError::bad_request(error.to_string()),
        ScoreError::AlreadyScoring => AppError {
            status: StatusCode::CONFLICT,
            message: error.to_string(),
        },
        ScoreError::Db(error) => error.into(),
    }
}

async fn scan_status(
    Extension(state): Extension<Arc<UserServices>>,
) -> Result<Json<Vec<ScanStatus>>, AppError> {
    let status = ask(&state.senders.scrape, |reply| ScrapeMessage::Status {
        reply,
    })
    .await??;
    Ok(Json(status))
}

async fn run_scan_now(
    Extension(state): Extension<Arc<UserServices>>,
    Path(site): Path<Site>,
) -> Result<StatusCode, AppError> {
    ask(&state.senders.scrape, |reply| ScrapeMessage::RunNow {
        site,
        reply,
    })
    .await?
    .map(|()| StatusCode::ACCEPTED)
    .map_err(|message| AppError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message,
    })
}
