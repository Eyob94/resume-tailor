use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

mod errors;
mod reader;
mod scanner;

use axum::{
    extract::{DefaultBodyLimit, FromRef, Multipart, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
    Json, Router,
};
use errors::{AppError, StartError};
use reader::{AiStatus, ResumeReader};
use scanner::{Scanner, MAX_INTERVAL_MINUTES, MIN_INTERVAL_MINUTES};
use serde::Deserialize;
use serde_json::json;
use tailor_core::{
    ai::DEFAULT_OLLAMA_URL,
    db::Db,
    errors::PdfError,
    linkedin::LinkedInClient,
    pdf,
    types::{
        JobListing, JobSearch, JobStatus, MasterResume, ProfileRecord, ResumeProfile, ScanStatus,
        SearchInput,
    },
};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

pub(crate) const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    store: Arc<Db>,
    scanner: Arc<Scanner>,
    reader: Arc<ResumeReader>,
}

impl FromRef<AppState> for Arc<ResumeReader> {
    fn from_ref(state: &AppState) -> Self {
        state.reader.clone()
    }
}

impl FromRef<AppState> for Arc<Db> {
    fn from_ref(state: &AppState) -> Self {
        state.store.clone()
    }
}

impl FromRef<AppState> for Arc<Scanner> {
    fn from_ref(state: &AppState) -> Self {
        state.scanner.clone()
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
    let scanner = Arc::new(Scanner::new(store.clone(), LinkedInClient::new()?));
    tokio::spawn(scanner.clone().run_forever());
    let ollama_url = env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_OLLAMA_URL.into());
    let reader = Arc::new(ResumeReader::new(store.clone(), ollama_url));

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
            scanner,
            reader,
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
    Ok(())
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
    let store = state.store.clone();
    while let Some(field) = multipart.next_field().await.map_err(AppError::multipart)? {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = sanitize_file_name(field.file_name().unwrap_or_default());
        let bytes = field.bytes().await.map_err(AppError::multipart)?;
        if bytes.is_empty() {
            return Err(AppError::bad_request("That file is empty."));
        }
        if bytes.len() > MAX_UPLOAD_BYTES {
            return Err(AppError::too_large());
        }

        let info = {
            let bytes = bytes.clone();
            tokio::task::spawn_blocking(move || pdf::inspect(&bytes))
                .await
                .map_err(AppError::internal)?
        }
        .map_err(|e| match e {
            PdfError::NotPdf => AppError::bad_request("That file isn't a PDF."),
            PdfError::Unreadable(detail) => {
                tracing::warn!("unreadable PDF: {detail}");
                AppError::bad_request(
                    "Couldn't read that PDF. It may be corrupted or password-protected.",
                )
            }
        })?;

        let saved =
            tokio::task::spawn_blocking(move || store.save_resume(&file_name, &bytes, &info))
                .await
                .map_err(AppError::internal)??;
        if let Err(StartError::Store(e)) = state.reader.start() {
            tracing::error!("couldn't start reading the resume: {e}");
        }
        return Ok(Json(saved));
    }
    Err(AppError::bad_request("No file was uploaded."))
}

async fn delete_resume(State(store): State<Arc<Db>>) -> Result<StatusCode, AppError> {
    if store.delete_resume()? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found())
    }
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

async fn read_profile(State(reader): State<Arc<ResumeReader>>) -> Result<StatusCode, AppError> {
    match reader.start() {
        Ok(()) => Ok(StatusCode::ACCEPTED),
        Err(StartError::NoResume) => Err(AppError::not_found()),
        Err(StartError::AlreadyReading) => Err(AppError {
            status: StatusCode::CONFLICT,
            message: "Your resume is already being read.".into(),
        }),
        Err(StartError::Store(e)) => Err(e.into()),
    }
}

fn current_profile(store: &Db) -> Result<(MasterResume, ProfileRecord), AppError> {
    let resume = store.get_resume()?.ok_or_else(AppError::not_found)?;
    let record = store
        .latest_profile(&resume.uploaded_at)?
        .ok_or_else(|| AppError::not_found_msg("Your resume hasn't been read yet."))?;
    Ok((resume, record))
}

async fn update_profile(
    State(store): State<Arc<Db>>,
    Json(mut profile): Json<ResumeProfile>,
) -> Result<Json<ProfileRecord>, AppError> {
    let (resume, record) = current_profile(&store)?;
    profile.tidy();
    profile.assign_ids();
    if profile.contact.name.is_empty() {
        return Err(AppError::bad_request("Add your name."));
    }
    let warnings = profile.check_against(&resume.extracted_text);
    if !store.save_profile_edits(record.id, &profile, &warnings)? {
        return Err(AppError {
            status: StatusCode::CONFLICT,
            message: "This reading can't be edited right now. Reload the page.".into(),
        });
    }
    let saved = store
        .get_profile(record.id)?
        .ok_or_else(|| AppError::internal("profile vanished"))?;
    Ok(Json(saved))
}

async fn confirm_profile(State(store): State<Arc<Db>>) -> Result<Json<ProfileRecord>, AppError> {
    let (_, record) = current_profile(&store)?;
    if !store.confirm_profile(record.id)? {
        return Err(AppError {
            status: StatusCode::CONFLICT,
            message: "Only a finished reading can be confirmed. Reload the page.".into(),
        });
    }
    let saved = store
        .get_profile(record.id)?
        .ok_or_else(|| AppError::internal("profile vanished"))?;
    Ok(Json(saved))
}

async fn ai_status(State(reader): State<Arc<ResumeReader>>) -> Json<AiStatus> {
    Json(reader.status().await)
}

#[derive(Deserialize)]
struct AiSettings {
    resume_reader_model: String,
}

async fn update_ai_settings(
    State(reader): State<Arc<ResumeReader>>,
    Json(settings): Json<AiSettings>,
) -> Result<Json<AiStatus>, AppError> {
    let model = settings.resume_reader_model.trim();
    if model.is_empty() || model.len() > 200 {
        return Err(AppError::bad_request("Pick a model."));
    }
    reader.set_model(model)?;
    Ok(Json(reader.status().await))
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
    state.scanner.reschedule();
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
    state.scanner.reschedule();
    Ok(Json(search))
}

async fn delete_search(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    if !state.store.delete_search(id)? {
        return Err(AppError::not_found_msg("Search not found."));
    }
    state.scanner.reschedule();
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

async fn scan_status(State(scanner): State<Arc<Scanner>>) -> Result<Json<ScanStatus>, AppError> {
    Ok(Json(scanner.status()?))
}

async fn run_scan_now(State(scanner): State<Arc<Scanner>>) -> Result<StatusCode, AppError> {
    scanner
        .request_run()
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

async fn get_settings(State(scanner): State<Arc<Scanner>>) -> Json<Settings> {
    Json(Settings {
        scan_interval_minutes: scanner.interval_minutes(),
    })
}

async fn update_settings(
    State(scanner): State<Arc<Scanner>>,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, AppError> {
    let minutes = settings.scan_interval_minutes;
    if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(&minutes) {
        return Err(AppError::bad_request(format!(
            "Scan interval must be between {MIN_INTERVAL_MINUTES} and {MAX_INTERVAL_MINUTES} minutes."
        )));
    }
    scanner.set_interval_minutes(minutes)?;
    Ok(Json(settings))
}

fn sanitize_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let name: String = base.chars().filter(|c| !c.is_control()).take(200).collect();
    if name.is_empty() {
        "resume.pdf".into()
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(
            sanitize_file_name("../../etc/My Resume.pdf"),
            "My Resume.pdf"
        );
        assert_eq!(sanitize_file_name("C:\\Users\\me\\cv.pdf"), "cv.pdf");
        assert_eq!(sanitize_file_name("   "), "resume.pdf");
    }
}
