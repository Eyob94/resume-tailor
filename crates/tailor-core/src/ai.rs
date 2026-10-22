//! Language-model backends that return JSON matching a schema.
//!
//! Only Ollama (local, free) for now; Claude Code / Codex CLIs can implement
//! the same trait later.

use std::{future::Future, time::Duration};

use serde::Serialize;
use serde_json::{json, Value};
use thiserror::Error;

pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

#[derive(Debug, Error)]
pub enum AiError {
    #[error("Ollama isn't running at {0}. Start it with `ollama serve` or the Ollama app.")]
    Unreachable(String),
    #[error("The model \"{0}\" isn't installed. Run `ollama pull {0}`.")]
    ModelMissing(String),
    #[error("The model took too long to answer.")]
    Timeout,
    #[error("The model returned an error (HTTP {0}): {1}")]
    Status(u16, String),
    #[error("The model's answer didn't fit the expected format: {0}")]
    BadOutput(String),
}

/// A model that answers with JSON shaped by a JSON schema.
pub trait JsonModel {
    /// Short name shown to the user, e.g. "ollama · qwen2.5:7b".
    fn label(&self) -> String;

    fn generate_json(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> impl Future<Output = Result<Value, AiError>> + Send;
}

pub struct Ollama {
    http: reqwest::Client,
    base_url: String,
    model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OllamaStatus {
    pub url: String,
    pub reachable: bool,
    pub models: Vec<String>,
}

impl Ollama {
    pub fn new(base_url: &str, model: &str) -> Self {
        let http = reqwest::Client::builder()
            // A 7B model on a laptop can take a few minutes for a long resume.
            .timeout(Duration::from_secs(600))
            .build()
            .expect("static client config is valid");
        Self { http, base_url: base_url.trim_end_matches('/').to_string(), model: model.to_string() }
    }

    /// Whether Ollama answers, and which models are installed.
    pub async fn status(base_url: &str) -> OllamaStatus {
        let url = base_url.trim_end_matches('/').to_string();
        let models = async {
            let res = reqwest::Client::new()
                .get(format!("{url}/api/tags"))
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .ok()?;
            let body: Value = res.json().await.ok()?;
            let mut names: Vec<String> = body["models"]
                .as_array()?
                .iter()
                .filter_map(|m| m["name"].as_str().map(str::to_string))
                .collect();
            names.sort();
            Some(names)
        }
        .await;
        OllamaStatus { url, reachable: models.is_some(), models: models.unwrap_or_default() }
    }

    fn map_send_error(&self, e: reqwest::Error) -> AiError {
        if e.is_timeout() {
            AiError::Timeout
        } else {
            AiError::Unreachable(self.base_url.clone())
        }
    }
}

impl JsonModel for Ollama {
    fn label(&self) -> String {
        format!("ollama · {}", self.model)
    }

    async fn generate_json(&self, system: &str, user: &str, schema: &Value) -> Result<Value, AiError> {
        let body = json!({
            "model": self.model,
            "stream": false,
            "format": schema,
            "options": {
                "temperature": 0,
                // Ollama's default 2048-token window cuts off a full resume.
                "num_ctx": 16384,
            },
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user },
            ],
        });
        let res = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| self.map_send_error(e))?;

        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| self.map_send_error(e))?;
        if status == 404 && text.contains("not found") {
            return Err(AiError::ModelMissing(self.model.clone()));
        }
        if !(200..300).contains(&status) {
            return Err(AiError::Status(status, text.chars().take(300).collect()));
        }

        let reply: Value = serde_json::from_str(&text).map_err(|e| AiError::BadOutput(e.to_string()))?;
        let content = reply["message"]["content"]
            .as_str()
            .ok_or_else(|| AiError::BadOutput("no message content".into()))?;
        serde_json::from_str(content).map_err(|e| AiError::BadOutput(e.to_string()))
    }
}
