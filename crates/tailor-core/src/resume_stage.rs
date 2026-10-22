use std::sync::Arc;

use tokio::{
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender},
        oneshot,
    },
    task::JoinHandle,
};

use crate::{
    db::Db,
    errors::{DbError, PdfError, ResumeError},
    parse_stage::ParseMessage,
    types::{MasterResume, PdfInfo},
};

pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

pub enum ResumeMessage {
    Upload {
        file_name: String,
        bytes: Vec<u8>,
        reply: oneshot::Sender<Result<MasterResume, ResumeError>>,
    },
    Remove {
        reply: oneshot::Sender<Result<(), ResumeError>>,
    },
}

pub struct ResumeStage {
    handle: JoinHandle<()>,
}

impl ResumeStage {
    pub fn new(
        db: Arc<Db>,
        rx: UnboundedReceiver<ResumeMessage>,
        parse: UnboundedSender<ParseMessage>,
    ) -> Self {
        let handle = tokio::spawn(run(db, rx, parse));
        Self { handle }
    }

    pub async fn join(self) {
        let _ = self.handle.await;
    }
}

async fn run(
    db: Arc<Db>,
    mut rx: UnboundedReceiver<ResumeMessage>,
    parse: UnboundedSender<ParseMessage>,
) {
    while let Some(message) = rx.recv().await {
        match message {
            ResumeMessage::Upload {
                file_name,
                bytes,
                reply,
            } => {
                let result = upload(&db, &file_name, bytes).await;
                if let Ok(resume) = &result {
                    let _ = reply.send(Ok(resume.clone()));
                    let _ = parse.send(ParseMessage::Parse {
                        resume: resume.clone(),
                    });
                } else {
                    let _ = reply.send(result);
                }
            }
            ResumeMessage::Remove { reply } => {
                let db = db.clone();
                let result = tokio::task::spawn_blocking(move || db.delete_resume())
                    .await
                    .map_err(|e| ResumeError::Db(DbError::Io(std::io::Error::other(e.to_string()))))
                    .and_then(|result| result.map_err(ResumeError::Db))
                    .and_then(|deleted| {
                        if deleted {
                            Ok(())
                        } else {
                            Err(ResumeError::NoResume)
                        }
                    });
                let _ = reply.send(result);
            }
        }
    }
}

async fn upload(
    db: &Arc<Db>,
    file_name: &str,
    bytes: Vec<u8>,
) -> Result<MasterResume, ResumeError> {
    if bytes.is_empty() {
        return Err(ResumeError::Empty);
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ResumeError::TooLarge(MAX_UPLOAD_BYTES / 1024 / 1024));
    }

    let bytes_for_inspection = bytes.clone();
    let info = tokio::task::spawn_blocking(move || inspect_pdf(&bytes_for_inspection))
        .await
        .map_err(|e| ResumeError::Db(DbError::Io(std::io::Error::other(e.to_string()))))?
        .map_err(|error| match error {
            PdfError::NotPdf => ResumeError::NotPdf,
            PdfError::Unreadable(detail) => {
                tracing::warn!("unreadable PDF: {detail}");
                ResumeError::Unreadable
            }
        })?;
    let file_name = sanitize_file_name(file_name);
    let db = db.clone();
    tokio::task::spawn_blocking(move || db.save_resume(&file_name, &bytes, &info))
        .await
        .map_err(|e| ResumeError::Db(DbError::Io(std::io::Error::other(e.to_string()))))?
        .map_err(ResumeError::Db)
}

fn inspect_pdf(bytes: &[u8]) -> Result<PdfInfo, PdfError> {
    let header_window = &bytes[..bytes.len().min(1024)];
    if !header_window.windows(5).any(|w| w == b"%PDF-") {
        return Err(PdfError::NotPdf);
    }
    let doc = lopdf::Document::load_mem(bytes).map_err(|e| PdfError::Unreadable(e.to_string()))?;
    let page_count = doc.get_pages().len();
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .ok()
        .and_then(Result::ok)
        .map(|raw| normalize(&raw))
        .unwrap_or_default();
    Ok(PdfInfo { page_count, text })
}

fn normalize(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut previous_blank = false;
    for line in raw.lines() {
        let line = line.trim_end();
        let blank = line.trim().is_empty();
        if blank && previous_blank {
            continue;
        }
        previous_blank = blank;
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
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
    use super::*;

    #[test]
    fn rejects_non_pdf() {
        assert!(matches!(inspect_pdf(b"hello world"), Err(PdfError::NotPdf)));
    }

    #[test]
    fn normalize_collapses_blank_lines() {
        assert_eq!(normalize("a  \n\n\n\nb\n"), "a\n\nb");
    }

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
