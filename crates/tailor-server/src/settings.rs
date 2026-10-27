use axum::{extract::Extension, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use tailor_core::{
    parse_stage::{ollama_status, DEFAULT_MODEL, MODEL_KEY, OLLAMA_URL_KEY},
    scrape_stage::{
        interval_key, interval_minutes, ScrapeMessage, MAX_INTERVAL_MINUTES, MIN_INTERVAL_MINUTES,
    },
    types::{OllamaStatus, Site},
};

use super::{errors::AppError, AppState, UserServices};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/ai", get(ai_status).put(update_ai_settings))
        .route("/settings", get(get_settings).put(update_settings))
}

#[derive(Debug, Serialize)]
struct AiStatus {
    ollama: OllamaStatus,
    resume_reader_model: String,
    ollama_url: String,
}

async fn ai_status(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
) -> Result<Json<AiStatus>, AppError> {
    let model = state
        .store
        .get_setting(MODEL_KEY)?
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    let ollama_url = state
        .store
        .get_setting(OLLAMA_URL_KEY)?
        .unwrap_or_else(|| state.ollama_url.clone());
    Ok(Json(AiStatus {
        ollama: ollama_status(&ollama_url).await,
        resume_reader_model: model,
        ollama_url,
    }))
}

#[derive(Deserialize)]
struct AiSettings {
    resume_reader_model: Option<String>,
    ollama_url: Option<String>,
}

async fn update_ai_settings(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
    Json(settings): Json<AiSettings>,
) -> Result<Json<AiStatus>, AppError> {
    let url = settings
        .ollama_url
        .as_deref()
        .map(|url| url.trim().trim_end_matches('/'));
    if let Some(url) = url {
        if url.len() > 2048 || !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(AppError::bad_request(
                "Enter a valid Ollama URL beginning with http:// or https://.",
            ));
        }
    }
    let model = settings.resume_reader_model.as_deref().map(str::trim);
    if let Some(model) = model {
        if model.is_empty() || model.len() > 200 {
            return Err(AppError::bad_request("Pick a model."));
        }
    }
    if let Some(url) = url {
        state.store.set_setting(OLLAMA_URL_KEY, url)?;
    }
    if let Some(model) = model {
        state.store.set_setting(MODEL_KEY, model)?;
    }
    ai_status(Extension(state)).await
}

#[derive(Serialize, Deserialize)]
struct Settings {
    scan_interval_minutes: u32,
    indeed_scan_interval_minutes: u32,
}

async fn get_settings(Extension(user): Extension<std::sync::Arc<UserServices>>) -> Json<Settings> {
    Json(Settings {
        scan_interval_minutes: interval_minutes(&user.store, Site::LinkedIn),
        indeed_scan_interval_minutes: interval_minutes(&user.store, Site::Indeed),
    })
}

async fn update_settings(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
    Json(settings): Json<Settings>,
) -> Result<Json<Settings>, AppError> {
    let intervals = [
        (Site::LinkedIn, settings.scan_interval_minutes),
        (Site::Indeed, settings.indeed_scan_interval_minutes),
    ];
    if intervals
        .iter()
        .any(|(_, minutes)| !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(minutes))
    {
        return Err(AppError::bad_request(format!(
            "Scan interval must be between {MIN_INTERVAL_MINUTES} and {MAX_INTERVAL_MINUTES} minutes."
        )));
    }
    for (site, minutes) in intervals {
        state
            .store
            .set_setting(interval_key(site), &minutes.to_string())?;
    }
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(settings))
}
