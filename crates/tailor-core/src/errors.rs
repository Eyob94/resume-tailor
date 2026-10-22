use thiserror::Error;

#[derive(Debug, Error)]
pub enum PdfError {
    #[error("file is not a PDF")]
    NotPdf,
    #[error("could not read PDF: {0}")]
    Unreadable(String),
}

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

#[derive(Debug, Error)]
pub enum LinkedInError {
    #[error("LinkedIn is rate-limiting requests (HTTP {0}); will retry next scan")]
    Blocked(u16),
    #[error("LinkedIn returned HTTP {0}")]
    Status(u16),
    #[error("couldn't reach LinkedIn: {0}")]
    Network(#[from] reqwest::Error),
}

#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
#[error("{0}")]
pub struct InvalidSearch(pub String);

#[derive(Debug, Error)]
pub(crate) enum ScanError {
    #[error(transparent)]
    Source(#[from] LinkedInError),
    #[error(transparent)]
    Db(#[from] DbError),
}
