use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

mod errors;

use axum::{
    extract::{DefaultBodyLimit, FromRef, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
    Json, Router,
};
use errors::AppError;

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use tailor_core::{
    db::Db,
    errors::{ProfileError, ResumeError},
    parse_stage::{ollama_status, ParseMessage, DEFAULT_MODEL, DEFAULT_OLLAMA_URL, MODEL_KEY},
    pipeline::{Pipeline, PipelineConfig, PipelineSenders},
    resume_stage::{ResumeMessage, MAX_UPLOAD_BYTES},
    scrape_stage::{
        interval_minutes, ScrapeMessage, DEFAULT_COOLDOWN_SECONDS, MAX_INTERVAL_MINUTES,
        MIN_INTERVAL_MINUTES,
    },
    types::{
        JobListing, JobSearch, JobStatus, MasterResume, OllamaStatus, ProfileRecord, ResumeProfile,
        ScanStatus, SearchInput,
    },
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    store: Arc<Db>,
    senders: PipelineSenders,
    ollama_url: String,
}

impl FromRef<AppState> for Arc<Db> {
    fn from_ref(state: &AppState) -> Self {
        state.store.clone()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let data_dir = env::var("DATA_DIR").unwrap_or_else(|_| "data".into());
    let web_dist = PathBuf::from(env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".into()));
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8787);

    let store = Arc::new(Db::open(&data_dir)?);
    let ollama_url = env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_OLLAMA_URL.into());
    let cooldown_seconds = env::var("SCAN_COOLDOWN_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_COOLDOWN_SECONDS);
    if cooldown_seconds != DEFAULT_COOLDOWN_SECONDS {
        tracing::warn!(cooldown_seconds, "manual scan cooldown overridden");
    }
    let (pipeline, senders) = Pipeline::new(
        store.clone(),
        PipelineConfig {
            ollama_url: ollama_url.clone(),
            scan_cooldown_seconds: cooldown_seconds,
        },
    )?;

    let api = Router::new()
        .route("/health", get(health))
        .route(
            "/resume",
            get(get_resume).post(upload_resume).delete(delete_resume),
        )
        .route("/resume/file", get(resume_file))
        .route("/resume/profile", get(get_profile).put(update_profile))
        .route("/resume/profile/read", post(read_profile))
        .route("/resume/profile/confirm", post(confirm_profile))
        .route("/ai", get(ai_status).put(update_ai_settings))
        .route("/searches", get(list_searches).post(create_search))
        .route("/searches/{id}", put(update_search).delete(delete_search))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", patch(update_job))
        .route("/jobs/status", post(update_jobs_status))
        .route("/scan", get(scan_status))
        .route("/scan/run", post(run_scan_now))
        .route("/settings", get(get_settings).put(update_settings))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 64 * 1024))
        .with_state(AppState {
            store,
            senders: senders.clone(),
            ollama_url,
        });

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
    drop(senders);
    pipeline.join().await;
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

async fn get_resume(State(store): State<Arc<Db>>) -> Result<Json<Option<MasterResume>>, AppError> {
    Ok(Json(store.get_resume()?))
}

async fn upload_resume(
    State(state): State<AppState>,
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

async fn delete_resume(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    ask(&state.senders.resume, |reply| ResumeMessage::Remove {
        reply,
    })
    .await?
    .map_err(map_resume_error)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn resume_file(State(store): State<Arc<Db>>) -> Result<Response, AppError> {
    let Some(resume) = store.get_resume()? else {
        return Err(AppError::not_found());
    };
    let bytes = match tokio::fs::read(store.resume_pdf_path()).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::not_found()),
        Err(e) => return Err(AppError::internal(e)),
    };
    let header_name: String = resume
        .file_name
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
    State(store): State<Arc<Db>>,
) -> Result<Json<Option<ProfileRecord>>, AppError> {
    let Some(resume) = store.get_resume()? else {
        return Ok(Json(None));
    };
    Ok(Json(store.latest_profile(&resume.uploaded_at)?))
}

async fn read_profile(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    ask(&state.senders.parse, |reply| ParseMessage::ReadAgain {
        reply,
    })
    .await?
    .map_err(map_profile_error)?;
    Ok(StatusCode::ACCEPTED)
}

async fn update_profile(
    State(state): State<AppState>,
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

async fn confirm_profile(State(state): State<AppState>) -> Result<Json<ProfileRecord>, AppError> {
    let saved = ask(&state.senders.parse, |reply| ParseMessage::Confirm {
        reply,
    })
    .await?
    .map_err(map_profile_error)?;
    Ok(Json(saved))
}

#[derive(Debug, Serialize)]
struct AiStatus {
    ollama: OllamaStatus,
    resume_reader_model: String,
}

async fn ai_status(State(state): State<AppState>) -> Result<Json<AiStatus>, AppError> {
    let model = state
        .store
        .get_setting(MODEL_KEY)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    Ok(Json(AiStatus {
        ollama: ollama_status(&state.ollama_url).await,
        resume_reader_model: model,
    }))
}

#[derive(Deserialize)]
struct AiSettings {
    resume_reader_model: String,
}

async fn update_ai_settings(
    State(state): State<AppState>,
    Json(settings): Json<AiSettings>,
) -> Result<Json<AiStatus>, AppError> {
    let model = settings.resume_reader_model.trim();
    if model.is_empty() || model.len() > 200 {
        return Err(AppError::bad_request("Pick a model."));
    }
    state.store.set_setting(MODEL_KEY, model)?;
    ai_status(State(state)).await
}

async fn list_searches(State(store): State<Arc<Db>>) -> Result<Json<Vec<JobSearch>>, AppError> {
    Ok(Json(store.list_searches()?))
}

async fn create_search(
    State(state): State<AppState>,
    Json(input): Json<SearchInput>,
) -> Result<(StatusCode, Json<JobSearch>), AppError> {
    let criteria = input.validate().map_err(|e| AppError::bad_request(e.0))?;
    let search = state.store.create_search(&criteria)?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok((StatusCode::CREATED, Json(search)))
}

async fn update_search(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<SearchInput>,
) -> Result<Json<JobSearch>, AppError> {
    let criteria = input.validate().map_err(|e| AppError::bad_request(e.0))?;
    let search = state
        .store
        .update_search(id, &criteria)?
        .ok_or_else(|| AppError::not_found_msg("Search not found."))?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(search))
}

async fn delete_search(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if !state.store.delete_search(id)? {
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
    State(store): State<Arc<Db>>,
    Query(query): Query<JobsQuery>,
) -> Result<Json<Vec<JobListing>>, AppError> {
    Ok(Json(store.list_jobs(query.status)?))
}

#[derive(Deserialize)]
struct JobPatch {
    status: JobStatus,
}

async fn update_job(
    State(store): State<Arc<Db>>,
    Path(id): Path<i64>,
    Json(patch): Json<JobPatch>,
) -> Result<StatusCode, AppError> {
    if store.set_job_status(id, patch.status)? {
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
    State(store): State<Arc<Db>>,
    Json(body): Json<BulkJobStatus>,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.ids.len() > MAX_BULK_IDS {
        return Err(AppError::bad_request(format!(
            "Update at most {MAX_BULK_IDS} jobs at once."
        )));
    }
    let updated = store.set_jobs_status(&body.ids, body.status)?;
    Ok(Json(json!({ "updated": updated })))
}

async fn scan_status(State(state): State<AppState>) -> Result<Json<ScanStatus>, AppError> {
    let status = ask(&state.senders.scrape, |reply| ScrapeMessage::Status {
        reply,
    })
    .await??;
    Ok(Json(status))
}

async fn run_scan_now(State(state): State<AppState>) -> Result<StatusCode, AppError> {
    ask(&state.senders.scrape, |reply| ScrapeMessage::RunNow {
        reply,
    })
    .await?
    .map(|()| StatusCode::ACCEPTED)
    .map_err(|message| AppError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message,
    })
}

#[derive(serde::Serialize, Deserialize)]
struct Settings {
    scan_interval_minutes: u32,
}

async fn get_settings(State(store): State<Arc<Db>>) -> Json<Settings> {
    Json(Settings {
        scan_interval_minutes: interval_minutes(&store),
    })
}

async fn update_settings(
    State(state): State<AppState>,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, AppError> {
    let minutes = settings.scan_interval_minutes;
    if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&minutes) {
        return Err(AppError::bad_request(format!(
            "Scan interval must be between {MIN_INTERVAL_MINUTES} and {MAX_INTERVAL_MINUTES} minutes."
        )));
    }
    state
        .store
        .set_setting("scan_interval_minutes", &minutes.to_string())?;
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(settings))
}
