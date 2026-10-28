use axum::{
    extract::{Extension, Path},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tailor_core::{
    parse_stage::{ai_step_config, ollama_status, DEFAULT_MODEL, MODEL_KEY, OLLAMA_URL_KEY},
    scrape_stage::{
        interval_key, interval_minutes, ScrapeMessage, MAX_INTERVAL_MINUTES, MIN_INTERVAL_MINUTES,
    },
    types::{AiStep, AiStepConfig, OllamaStatus, Site},
};

use super::{errors::AppError, AppState, UserServices};

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/ai", get(ai_status).put(update_ai_settings))
        .route("/settings", get(get_settings).put(update_settings))
        .route("/workflow/ai-steps", get(get_ai_steps))
        .route(
            "/workflow/ai-steps/{step}",
            axum::routing::put(update_ai_step),
        )
        .route(
            "/workflow/ai-steps/{step}/prompt",
            axum::routing::delete(reset_ai_prompt),
        )
}

async fn get_ai_steps(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
) -> Result<Json<Vec<AiStepConfig>>, AppError> {
    let mut configs = Vec::new();
    for step in AiStep::ALL {
        configs.push(ai_step_config(&state.store, step).await?);
    }
    Ok(Json(configs))
}

#[derive(Deserialize)]
struct AiStepUpdate {
    model: Option<String>,
    prompt: Option<String>,
}

async fn update_ai_step(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
    Path(step): Path<String>,
    Json(update): Json<AiStepUpdate>,
) -> Result<Json<AiStepConfig>, AppError> {
    let step = AiStep::parse(&step).ok_or_else(|| AppError::not_found_msg("Unknown step."))?;
    let model = update.model.as_deref().map(str::trim);
    let prompt = update.prompt.as_deref().map(str::trim);
    if let Some(model) = model {
        if model.is_empty() || model.chars().count() > 200 {
            return Err(AppError::bad_request("Model must be 1 to 200 characters."));
        }
    }
    if let Some(prompt) = prompt {
        if prompt.is_empty() || prompt.chars().count() > 20_000 {
            return Err(AppError::bad_request(
                "Prompt must be 1 to 20,000 characters.",
            ));
        }
    }
    let current = if prompt.is_some() {
        Some(ai_step_config(&state.store, step).await?)
    } else {
        None
    };
    if let Some(model) = model {
        state.store.set_setting(step.model_key(), model).await?;
    }
    if let Some(prompt) = prompt {
        let current = current.unwrap();
        if prompt == current.default_prompt {
            state.store.delete_setting(step.prompt_key()).await?;
        } else {
            state.store.set_setting(step.prompt_key(), prompt).await?;
        }
    }
    Ok(Json(ai_step_config(&state.store, step).await?))
}

async fn reset_ai_prompt(
    Extension(state): Extension<std::sync::Arc<UserServices>>,
    Path(step): Path<String>,
) -> Result<Json<AiStepConfig>, AppError> {
    let step = AiStep::parse(&step).ok_or_else(|| AppError::not_found_msg("Unknown step."))?;
    state.store.delete_setting(step.prompt_key()).await?;
    Ok(Json(ai_step_config(&state.store, step).await?))
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
        .get_setting(MODEL_KEY)
        .await?
        .unwrap_or_else(|| DEFAULT_MODEL.into());
    let ollama_url = state
        .store
        .get_setting(OLLAMA_URL_KEY)
        .await?
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
        state.store.set_setting(OLLAMA_URL_KEY, url).await?;
    }
    if let Some(model) = model {
        state.store.set_setting(MODEL_KEY, model).await?;
    }
    ai_status(Extension(state)).await
}

#[derive(Serialize, Deserialize)]
struct Settings {
    scan_interval_minutes: u32,
    indeed_scan_interval_minutes: u32,
}

async fn get_settings(
    Extension(user): Extension<std::sync::Arc<UserServices>>,
) -> Result<Json<Settings>, AppError> {
    Ok(Json(Settings {
        scan_interval_minutes: interval_minutes(&user.store, Site::LinkedIn).await?,
        indeed_scan_interval_minutes: interval_minutes(&user.store, Site::Indeed).await?,
    }))
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
            .set_setting(interval_key(site), &minutes.to_string())
            .await?;
    }
    let _ = state.senders.scrape.send(ScrapeMessage::Reschedule);
    Ok(Json(settings))
}
