use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use thiserror::Error;
use uuid::Uuid;

use crate::domain::LedgerEntry;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(String),
}

/// Database access for the email-OCR ingestion pipeline. Wraps a `PgPool`
/// and provides the two operations the pipeline needs: checking which
/// content hashes have already been processed, and recording a new
/// ingested document (with optional ledger entry upsert).
#[derive(Debug, Clone)]
pub struct IngestStore {
    pool: PgPool,
}

impl IngestStore {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns all content hashes already processed by the pipeline.
    /// Used to skip PDFs that were already seen on a previous run.
    pub async fn fetch_known_hashes(
        &self,
    ) -> Result<std::collections::HashSet<String>, StoreError> {
        let rows = sqlx::query("SELECT content_hash FROM ingested_documents")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| row.get::<String, _>("content_hash"))
            .collect())
    }

    /// Upserts a ledger entry. The `UNIQUE (source, external_id)` constraint
    /// means re-processing the same PDF updates the entry rather than
    /// duplicating it.
    pub async fn upsert_entry(&self, entry: &LedgerEntry) -> Result<(), StoreError> {
        sqlx::query(
            r#"
            INSERT INTO ledger_entries (
                id, external_id, kind, booked_on, gross, net, vat,
                category, counterparty, source
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (source, external_id) DO UPDATE SET
                gross = EXCLUDED.gross,
                net = EXCLUDED.net,
                vat = EXCLUDED.vat,
                counterparty = EXCLUDED.counterparty,
                updated_at = now()
            "#,
        )
        .bind(entry.id)
        .bind(&entry.external_id)
        .bind(kind_to_str(entry.kind))
        .bind(entry.booked_on)
        .bind(entry.gross.amount())
        .bind(entry.net.map(|m| m.amount()))
        .bind(entry.vat.map(|m| m.amount()))
        .bind(&entry.category)
        .bind(&entry.counterparty)
        .bind(entry.source.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(())
    }

    /// Records an ingested document in the audit table. If `ledger_entry_id`
    /// is provided, it links the document to its ledger entry.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_document(
        &self,
        content_hash: &str,
        filename: &str,
        email_date: DateTime<Utc>,
        email_subject: &str,
        email_recipient: &str,
        classification: &str,
        extracted_text: Option<&str>,
        vendor: Option<&str>,
        invoice_date: Option<NaiveDate>,
        gross: Option<Decimal>,
        net: Option<Decimal>,
        vat: Option<Decimal>,
        vat_rate: Option<Decimal>,
    ) -> Result<(), StoreError> {
        let id = Uuid::new_v4();

        // Look up the ledger entry ID if this was classified as an invoice.
        let ledger_entry_id: Option<Uuid> = if classification == "invoice" {
            sqlx::query_scalar::<_, Option<Uuid>>(
                "SELECT id FROM ledger_entries WHERE source = 'email-ocr' AND external_id = $1",
            )
            .bind(content_hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Database(e.to_string()))?
            .flatten()
        } else {
            None
        };

        sqlx::query(
            r#"
            INSERT INTO ingested_documents (
                id, content_hash, filename, email_date, email_subject,
                email_recipient, classification, extracted_text,
                vendor, invoice_date, gross, net, vat, vat_rate,
                ledger_entry_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            ON CONFLICT (content_hash) DO UPDATE SET
                classification = EXCLUDED.classification,
                extracted_text = EXCLUDED.extracted_text,
                vendor = EXCLUDED.vendor,
                invoice_date = EXCLUDED.invoice_date,
                gross = EXCLUDED.gross,
                net = EXCLUDED.net,
                vat = EXCLUDED.vat,
                vat_rate = EXCLUDED.vat_rate,
                ledger_entry_id = EXCLUDED.ledger_entry_id
            "#,
        )
        .bind(id)
        .bind(content_hash)
        .bind(filename)
        .bind(email_date)
        .bind(email_subject)
        .bind(email_recipient)
        .bind(classification)
        .bind(extracted_text)
        .bind(vendor)
        .bind(invoice_date)
        .bind(gross)
        .bind(net)
        .bind(vat)
        .bind(vat_rate)
        .bind(ledger_entry_id)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(())
    }

    /// Returns a monthly cost summary grouped by vendor, with gross/net/VAT
    /// totals. Only includes `email-ocr` source entries.
    pub async fn cost_summary_by_vendor(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<VendorCostSummary>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                COALESCE(counterparty, '(unknown)') AS vendor,
                COUNT(*) AS invoice_count,
                SUM(gross) AS total_gross,
                SUM(net) AS total_net,
                SUM(vat) AS total_vat
            FROM ledger_entries
            WHERE source = 'email-ocr'
              AND kind = 'expense'
              AND EXTRACT(YEAR FROM booked_on) = $1
              AND EXTRACT(MONTH FROM booked_on) = $2
            GROUP BY counterparty
            ORDER BY total_gross DESC
            "#,
        )
        .bind(year)
        .bind(month as i32)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| VendorCostSummary {
                vendor: row.get("vendor"),
                invoice_count: row.get::<i64, _>("invoice_count") as usize,
                total_gross: row.get("total_gross"),
                total_net: row.get("total_net"),
                total_vat: row.get("total_vat"),
            })
            .collect())
    }

    /// Returns a monthly cost summary grouped by category, with gross/net/VAT
    /// totals. Only includes `email-ocr` source entries.
    pub async fn cost_summary_by_category(
        &self,
        year: i32,
        month: u32,
    ) -> Result<Vec<CategoryCostSummary>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                COALESCE(category, '(uncategorized)') AS category,
                COUNT(*) AS invoice_count,
                SUM(gross) AS total_gross,
                SUM(net) AS total_net,
                SUM(vat) AS total_vat
            FROM ledger_entries
            WHERE source = 'email-ocr'
              AND kind = 'expense'
              AND EXTRACT(YEAR FROM booked_on) = $1
              AND EXTRACT(MONTH FROM booked_on) = $2
            GROUP BY category
            ORDER BY total_gross DESC
            "#,
        )
        .bind(year)
        .bind(month as i32)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| CategoryCostSummary {
                category: row.get("category"),
                invoice_count: row.get::<i64, _>("invoice_count") as usize,
                total_gross: row.get("total_gross"),
                total_net: row.get("total_net"),
                total_vat: row.get("total_vat"),
            })
            .collect())
    }

    /// Returns recent ingested documents, newest first.
    pub async fn recent_documents(
        &self,
        limit: i64,
    ) -> Result<Vec<IngestedDocumentRow>, StoreError> {
        let rows = sqlx::query(
            r#"
            SELECT
                content_hash, filename, email_date, email_subject,
                classification, vendor, invoice_date, gross, net, vat, vat_rate
            FROM ingested_documents
            ORDER BY email_date DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Database(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| IngestedDocumentRow {
                content_hash: row.get("content_hash"),
                filename: row.get("filename"),
                email_date: row.get("email_date"),
                email_subject: row.get("email_subject"),
                classification: row.get("classification"),
                vendor: row.get("vendor"),
                invoice_date: row.get("invoice_date"),
                gross: row.get("gross"),
                net: row.get("net"),
                vat: row.get("vat"),
                vat_rate: row.get("vat_rate"),
            })
            .collect())
    }
}

/// Monthly cost summary for a single vendor.
#[derive(Debug, Clone, Serialize)]
pub struct VendorCostSummary {
    pub vendor: String,
    pub invoice_count: usize,
    pub total_gross: Decimal,
    pub total_net: Option<Decimal>,
    pub total_vat: Option<Decimal>,
}

/// Monthly cost summary for a single category.
#[derive(Debug, Clone, Serialize)]
pub struct CategoryCostSummary {
    pub category: String,
    pub invoice_count: usize,
    pub total_gross: Decimal,
    pub total_net: Option<Decimal>,
    pub total_vat: Option<Decimal>,
}

/// A row from the `ingested_documents` audit table.
#[derive(Debug, Clone, Serialize)]
pub struct IngestedDocumentRow {
    pub content_hash: String,
    pub filename: String,
    pub email_date: DateTime<Utc>,
    pub email_subject: String,
    pub classification: String,
    pub vendor: Option<String>,
    pub invoice_date: Option<NaiveDate>,
    pub gross: Option<Decimal>,
    pub net: Option<Decimal>,
    pub vat: Option<Decimal>,
    pub vat_rate: Option<Decimal>,
}

fn kind_to_str(kind: crate::domain::EntryKind) -> &'static str {
    match kind {
        crate::domain::EntryKind::Expense => "expense",
        crate::domain::EntryKind::Revenue => "revenue",
    }
}
