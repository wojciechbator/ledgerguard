pub mod classify;
pub mod imap_client;
pub mod parse;
pub mod pdf;
pub mod store;
pub mod vendor;

use serde::Serialize;
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::domain::{EntryKind, LedgerEntry, Money, SourceSystem};

use classify::DocumentClass;
use imap_client::{EmailAttachment, ImapConfig, ImapFetchError};
use pdf::PdfExtractError;
use store::IngestStore;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error(transparent)]
    Imap(#[from] ImapFetchError),
    #[error(transparent)]
    Pdf(#[from] PdfExtractError),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error("configuration error: {0}")]
    Config(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct IngestReport {
    pub scanned: usize,
    pub invoices_imported: usize,
    pub bank_confirmations_skipped: usize,
    pub unparseable: usize,
    pub errors: usize,
}

/// Runs the full email-OCR ingestion pipeline:
///
/// 1. Connect to IMAP and fetch PDF attachments from sent emails
/// 2. For each PDF: extract text, classify, parse, store
/// 3. Bank confirmations are skipped but recorded for dedup
/// 4. Invoices are upserted into `ledger_entries` with `source = "email-ocr"`
pub async fn run_ingest(
    imap_config: ImapConfig,
    store: &IngestStore,
) -> Result<IngestReport, IngestError> {
    // 1. Load already-seen content hashes from the database for dedup.
    let known_hashes = store.fetch_known_hashes().await?;
    info!(
        "email ingest: {} documents already processed, fetching new attachments",
        known_hashes.len()
    );

    // 2. Fetch PDF attachments from the IMAP sent folder.
    //    The IMAP crate is synchronous, so we run it in spawn_blocking.
    let known_hashes_clone = known_hashes.clone();
    let attachments = tokio::task::spawn_blocking(move || {
        imap_client::fetch_pdf_attachments(&imap_config, &known_hashes_clone)
    })
    .await
    .map_err(|e| IngestError::Imap(ImapFetchError::Connect(format!("join error: {e}"))))??;

    info!(
        "email ingest: {} new PDF attachments to process",
        attachments.len()
    );

    let mut report = IngestReport {
        scanned: attachments.len(),
        invoices_imported: 0,
        bank_confirmations_skipped: 0,
        unparseable: 0,
        errors: 0,
    };

    // Process attachments with bounded concurrency. PDF text extraction
    // (pdftotext) is fast (~50ms), but OCR fallback (Tesseract) takes ~10s
    // per scanned receipt. Processing 4 at a time cuts total wall time
    // significantly without overwhelming the CPU on the home server.
    use tokio::task::JoinSet;

    const CONCURRENCY: usize = 4;
    let mut join_set: JoinSet<(String, Result<DocumentClass, IngestError>)> = JoinSet::new();
    let mut attachments_iter = attachments.into_iter();

    // Prime the queue with up to CONCURRENCY tasks.
    for att in attachments_iter.by_ref().take(CONCURRENCY) {
        let store = store.clone();
        let filename = att.filename.clone();
        join_set.spawn(async move {
            let result = process_attachment(&att, &store).await;
            (filename, result)
        });
    }

    // As each task completes, spawn the next one.
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((filename, result)) => match result {
                Ok(DocumentClass::Invoice) => report.invoices_imported += 1,
                Ok(DocumentClass::BankConfirmation) => report.bank_confirmations_skipped += 1,
                Ok(DocumentClass::Unparseable) => report.unparseable += 1,
                Err(error) => {
                    warn!("email ingest: failed to process {filename}: {error}");
                    report.errors += 1;
                }
            },
            Err(join_err) => {
                warn!("email ingest: task panicked: {join_err}");
                report.errors += 1;
            }
        }

        if let Some(att) = attachments_iter.next() {
            let store = store.clone();
            let filename = att.filename.clone();
            join_set.spawn(async move {
                let result = process_attachment(&att, &store).await;
                (filename, result)
            });
        }
    }

    info!(
        "email ingest complete: {} scanned, {} invoices, {} bank confirmations skipped, {} unparseable, {} errors",
        report.scanned,
        report.invoices_imported,
        report.bank_confirmations_skipped,
        report.unparseable,
        report.errors
    );

    Ok(report)
}

async fn process_attachment(
    attachment: &EmailAttachment,
    store: &IngestStore,
) -> Result<DocumentClass, IngestError> {
    // Extract text from the PDF (digital text first, OCR fallback).
    let text_bytes = attachment.bytes.clone();
    let extracted_text = tokio::task::spawn_blocking(move || pdf::extract_text(&text_bytes))
        .await
        .map_err(|e| IngestError::Pdf(PdfExtractError::TempFile(format!("join error: {e}"))))??;

    // Classify: invoice vs. bank confirmation vs. unparseable.
    let class = classify::classify(&extracted_text);

    match class {
        DocumentClass::BankConfirmation => {
            store
                .record_document(
                    &attachment.content_hash,
                    &attachment.filename,
                    attachment.email_date,
                    &attachment.email_subject,
                    &attachment.email_recipient,
                    "bank_confirmation",
                    Some(&extracted_text),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            info!(
                "email ingest: skipped bank confirmation: {}",
                attachment.filename
            );
            Ok(DocumentClass::BankConfirmation)
        }
        DocumentClass::Invoice => {
            let parsed = parse::parse_invoice(&extracted_text);

            let Some(gross) = parsed.gross else {
                warn!(
                    "email ingest: invoice classified but no gross amount found: {}",
                    attachment.filename
                );
                store
                    .record_document(
                        &attachment.content_hash,
                        &attachment.filename,
                        attachment.email_date,
                        &attachment.email_subject,
                        &attachment.email_recipient,
                        "unparseable",
                        Some(&extracted_text),
                        parsed.vendor.as_deref(),
                        parsed.invoice_date,
                        None,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await?;
                return Ok(DocumentClass::Unparseable);
            };
            let gross_money = Money::non_negative(gross)
                .map_err(|e| IngestError::Config(format!("invalid gross: {e}")))?;

            let net_money = parsed.net.and_then(|n| Money::non_negative(n).ok());

            let vat_money = parsed.vat.and_then(|v| Money::non_negative(v).ok());

            // Classify vendor and assign a cost category.
            let classification = vendor::classify_vendor(parsed.vendor.as_deref(), &extracted_text);
            let canonical_vendor = classification.vendor.or(parsed.vendor.clone());
            let category = classification.category;

            let source =
                SourceSystem::new("email-ocr").map_err(|e| IngestError::Config(e.to_string()))?;

            let booked_on = parsed
                .invoice_date
                .unwrap_or_else(|| attachment.email_date.date_naive());

            let entry = LedgerEntry {
                id: Uuid::new_v4(),
                external_id: attachment.content_hash.clone(),
                kind: EntryKind::Expense,
                booked_on,
                gross: gross_money,
                net: net_money,
                vat: vat_money,
                category: category.clone(),
                counterparty: canonical_vendor.clone(),
                source,
            };

            let entry_id = store.upsert_entry(&entry).await?;

            store
                .record_document(
                    &attachment.content_hash,
                    &attachment.filename,
                    attachment.email_date,
                    &attachment.email_subject,
                    &attachment.email_recipient,
                    "invoice",
                    Some(&extracted_text),
                    canonical_vendor.as_deref(),
                    parsed.invoice_date,
                    Some(gross),
                    parsed.net,
                    parsed.vat,
                    parsed.vat_rate,
                    Some(entry_id),
                )
                .await?;

            info!(
                "email ingest: imported invoice from {}: gross={}, vendor={:?}, category={:?}",
                attachment.filename, gross, canonical_vendor, category
            );

            Ok(DocumentClass::Invoice)
        }
        DocumentClass::Unparseable => {
            store
                .record_document(
                    &attachment.content_hash,
                    &attachment.filename,
                    attachment.email_date,
                    &attachment.email_subject,
                    &attachment.email_recipient,
                    "unparseable",
                    Some(&extracted_text),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            warn!(
                "email ingest: unparseable document: {}",
                attachment.filename
            );
            Ok(DocumentClass::Unparseable)
        }
    }
}
