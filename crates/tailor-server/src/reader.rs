
use std::sync::Arc;

use serde::Serialize;
use tailor_core::{
    ai::{Ollama, OllamaStatus},
    profile::read_resume,
    store::{Store, StoreError},
};

const MODEL_KEY: &str = "resume_reader_model";
pub const DEFAULT_MODEL: &str = "qwen2.5:7b";

#[derive(Debug, Serialize)]
pub struct AiStatus {
    pub ollama: OllamaStatus,
    pub resume_reader_model: String,
}

pub enum StartError {
    NoResume,
    AlreadyReading,
    Store(StoreError),
}

impl From<StoreError> for StartError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

pub struct ResumeReader {
    store: Arc<Store>,
    ollama_url: String,
}

impl ResumeReader {
    pub fn new(store: Arc<Store>, ollama_url: String) -> Self {
        if let Err(e) = store.mark_interrupted_profiles() {
            tracing::error!("couldn't tidy interrupted resume readings: {e}");
        }
        Self { store, ollama_url }
    }

    pub fn model(&self) -> String {
        self.store.get_setting(MODEL_KEY).ok().flatten().unwrap_or_else(|| DEFAULT_MODEL.into())
    }

    pub fn set_model(&self, model: &str) -> Result<(), StoreError> {
        self.store.set_setting(MODEL_KEY, model)
    }

    pub async fn status(&self) -> AiStatus {
        AiStatus { ollama: Ollama::status(&self.ollama_url).await, resume_reader_model: self.model() }
    }

    pub fn start(self: &Arc<Self>) -> Result<(), StartError> {
        let resume = self.store.get_resume()?.ok_or(StartError::NoResume)?;
        if let Some(latest) = self.store.latest_profile(&resume.uploaded_at)? {
            if latest.status == "reading" {
                return Err(StartError::AlreadyReading);
            }
        }

        let model = Ollama::new(&self.ollama_url, &self.model());
        let label = tailor_core::ai::JsonModel::label(&model);
        let id = self.store.start_profile(&resume.uploaded_at, &label)?;
        let reader = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!(model = %label, "reading resume");
            let started = std::time::Instant::now();
            let outcome = match read_resume(&model, &resume.extracted_text).await {
                Ok((profile, warnings)) => reader.store.finish_profile(id, &profile, &warnings),
                Err(e) => {
                    tracing::warn!("resume reading failed: {e}");
                    reader.store.fail_profile(id, &e.to_string())
                }
            };
            if let Err(e) = outcome {
                tracing::error!("couldn't save resume reading: {e}");
            }
            tracing::info!(seconds = started.elapsed().as_secs(), "resume reading finished");
        });
        Ok(())
    }
}
