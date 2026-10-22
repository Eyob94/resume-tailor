use crate::errors::PdfError;

#[derive(Debug, Clone)]
pub struct PdfInfo {
    pub page_count: usize,
    pub text: String,
}

pub fn inspect(bytes: &[u8]) -> Result<PdfInfo, PdfError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pdf() {
        assert!(matches!(inspect(b"hello world"), Err(PdfError::NotPdf)));
    }

    #[test]
    fn normalize_collapses_blank_lines() {
        assert_eq!(normalize("a  \n\n\n\nb\n"), "a\n\nb");
    }
}
