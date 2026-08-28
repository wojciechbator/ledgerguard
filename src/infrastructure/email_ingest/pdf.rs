use std::process::Command;
use thiserror::Error;
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum PdfExtractError {
    #[error("pdftotext failed: {0}")]
    TextExtraction(String),
    #[error("pdftoppm failed: {0}")]
    RenderPages(String),
    #[error("tesseract failed: {0}")]
    Ocr(String),
    #[error("temporary file I/O error: {0}")]
    TempFile(String),
}

/// Extracts text from a PDF. Digital invoices with embedded text are read
/// directly via `pdftotext`. Scanned invoices (no embedded text) fall back
/// to rendering pages at 300 DPI and running Tesseract OCR with Polish +
/// English language data.
pub fn extract_text(pdf_bytes: &[u8]) -> Result<String, PdfExtractError> {
    // Stage 1: try pdftotext — fast, preserves layout, works for digital invoices.
    let text = extract_with_pdftotext(pdf_bytes)?;
    let trimmed = text.trim();
    if trimmed.len() > 50 {
        debug!("pdftotext extracted {} chars", trimmed.len());
        return Ok(text);
    }

    // Stage 2: OCR fallback for scanned invoices.
    debug!(
        "pdftotext returned {len} chars, falling back to OCR",
        len = trimmed.len()
    );
    let ocr_text = extract_with_tesseract(pdf_bytes)?;
    Ok(ocr_text)
}

fn extract_with_pdftotext(pdf_bytes: &[u8]) -> Result<String, PdfExtractError> {
    let mut child = Command::new("pdftotext")
        .args(["-layout", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| PdfExtractError::TextExtraction(format!("spawn pdftotext: {e}")))?;

    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(pdf_bytes)
            .map_err(|e| PdfExtractError::TextExtraction(format!("write stdin: {e}")))?;
    }
    // Drop stdin to signal EOF.
    drop(child.stdin.take());

    let output = child
        .wait_with_output()
        .map_err(|e| PdfExtractError::TextExtraction(format!("wait: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // pdftotext returns non-zero on some valid PDFs; treat empty output as
        // the real signal rather than the exit code.
        if output.stdout.is_empty() {
            return Err(PdfExtractError::TextExtraction(stderr.to_string()));
        }
        warn!("pdftotext exited non-zero but produced output: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn extract_with_tesseract(pdf_bytes: &[u8]) -> Result<String, PdfExtractError> {
    let temp_dir = tempfile::tempdir().map_err(|e| PdfExtractError::TempFile(e.to_string()))?;
    let pdf_path = temp_dir.path().join("input.pdf");
    let ppm_prefix = temp_dir.path().join("page");

    std::fs::write(&pdf_path, pdf_bytes).map_err(|e| PdfExtractError::TempFile(e.to_string()))?;

    // Render each page to a PNG at 400 DPI. 300 DPI was too low for
    // thermal-printer gas station receipts — 400 gives Tesseract enough
    // pixel density to distinguish digits on small print.
    let render = Command::new("pdftoppm")
        .args(["-r", "400", "-png"])
        .arg(&pdf_path)
        .arg(&ppm_prefix)
        .output()
        .map_err(|e| PdfExtractError::RenderPages(format!("spawn pdftoppm: {e}")))?;

    if !render.status.success() {
        return Err(PdfExtractError::RenderPages(
            String::from_utf8_lossy(&render.stderr).to_string(),
        ));
    }

    // Find all rendered page images, sorted by page number.
    let mut pages: Vec<_> = std::fs::read_dir(temp_dir.path())
        .map_err(|e| PdfExtractError::TempFile(e.to_string()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect();
    pages.sort();

    if pages.is_empty() {
        return Err(PdfExtractError::RenderPages(
            "pdftoppm produced no page images".to_owned(),
        ));
    }

    let mut full_text = String::new();

    for page in &pages {
        // PSM 6 (Assume a single uniform block of text) works best for
        // receipts and invoices with simple column layouts. The default
        // PSM 3 (fully automatic) often missegments narrow receipts.
        let output = Command::new("tesseract")
            .arg(page)
            .arg("stdout")
            .args(["-l", "pol+eng", "--psm", "6"])
            .output()
            .map_err(|e| PdfExtractError::Ocr(format!("spawn tesseract: {e}")))?;

        if !output.status.success() {
            warn!(
                "tesseract exited non-zero on {:?}: {}",
                page,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        full_text.push_str(&String::from_utf8_lossy(&output.stdout));
        full_text.push('\n');
    }

    Ok(full_text)
}
