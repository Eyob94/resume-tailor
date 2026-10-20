use std::{env, fmt::Display, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    extract::{DefaultBodyLimit, Multipart, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;
use tailor_core::{
    pdf::{self, PdfError},
    store::{MasterResume, Store, StoreError},
};
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

type AppState = Arc<Store>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let data_dir = env::var("DATA_DIR").unwrap_or_else(|_| "data".into());
    let web_dist = PathBuf::from(env::var("WEB_DIST").unwrap_or_else(|_| "web/dist".into()));
    let port: u16 = env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);

    let store = Arc::new(Store::open(&data_dir)?);

    let api = Router::new()
        .route("/health", get(health))
        .route("/resume", get(get_resume).post(upload_resume).delete(delete_resume))
        .route("/resume/file", get(resume_file))
        // Leave headroom over the file limit for multipart framing.
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES + 64 * 1024))
        .with_state(store);

    let mut app = Router::new().nest("/api", api);
    if web_dist.join("index.html").exists() {
        // Serve the built frontend; unknown paths fall back to the SPA entry.
        let spa = ServeDir::new(&web_dist).fallback(ServeFile::new(web_dist.join("index.html")));
        app = app.fallback_service(spa);
    }
    let app = app.layer(TraceLayer::new_for_http());

    // Loopback only: there's no auth, so never expose this on the network.
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn get_resume(State(store): State<AppState>) -> Result<Json<Option<MasterResume>>, AppError> {
    Ok(Json(store.get_resume()?))
}

async fn upload_resume(
    State(store): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<MasterResume>, AppError> {
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
                AppError::bad_request("Couldn't read that PDF. It may be corrupted or password-protected.")
            }
        })?;

        let saved = tokio::task::spawn_blocking(move || store.save_resume(&file_name, &bytes, &info))
            .await
            .map_err(AppError::internal)??;
        return Ok(Json(saved));
    }
    Err(AppError::bad_request("No file was uploaded."))
}

async fn delete_resume(State(store): State<AppState>) -> Result<StatusCode, AppError> {
    if store.delete_resume()? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::not_found())
    }
}

async fn resume_file(State(store): State<AppState>) -> Result<Response, AppError> {
    let Some(resume) = store.get_resume()? else {
        return Err(AppError::not_found());
    };
    let bytes = match tokio::fs::read(store.resume_pdf_path()).await {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AppError::not_found()),
        Err(e) => return Err(AppError::internal(e)),
    };
    // Header values must be ASCII; the real name is still shown in the UI.
    let header_name: String = resume
        .file_name
        .chars()
        .map(|c| if c.is_ascii_graphic() && c != '"' || c == ' ' { c } else { '_' })
        .collect();
    let headers = [
        (header::CONTENT_TYPE, "application/pdf".to_string()),
        (header::CONTENT_DISPOSITION, format!("inline; filename=\"{header_name}\"")),
        (header::CACHE_CONTROL, "no-cache".to_string()),
    ];
    Ok((headers, bytes).into_response())
}

/// Keeps only the base name, trimmed and capped, defaulting to `resume.pdf`.
fn sanitize_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    let name: String = base.chars().filter(|c| !c.is_control()).take(200).collect();
    if name.is_empty() {
        "resume.pdf".into()
    } else {
        name
    }
}

struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }

    fn not_found() -> Self {
        Self { status: StatusCode::NOT_FOUND, message: "No master resume has been uploaded.".into() }
    }

    fn too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: format!("That file is too large. The limit is {} MB.", MAX_UPLOAD_BYTES / 1024 / 1024),
        }
    }

    fn multipart(e: axum::extract::multipart::MultipartError) -> Self {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::too_large()
        } else {
            Self { status: e.status(), message: e.body_text() }
        }
    }

    fn internal(e: impl Display) -> Self {
        tracing::error!("internal error: {e}");
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: "Something went wrong on the server.".into() }
    }
}

impl From<StoreError> for AppError {
    fn from(e: StoreError) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize_file_name;

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("../../etc/My Resume.pdf"), "My Resume.pdf");
        assert_eq!(sanitize_file_name("C:\\Users\\me\\cv.pdf"), "cv.pdf");
        assert_eq!(sanitize_file_name("   "), "resume.pdf");
    }
}
