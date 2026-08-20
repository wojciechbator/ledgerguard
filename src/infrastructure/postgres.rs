use async_trait::async_trait;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::{
    application::{LedgerRepository, RepositoryError},
    domain::{EntryKind, LedgerEntry, Money, Month, SourceSystem},
};

#[derive(Debug, Clone)]
pub struct PgLedgerRepository {
    pool: PgPool,
}

impl PgLedgerRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl LedgerRepository for PgLedgerRepository {
    async fn upsert_entries(&self, entries: &[LedgerEntry]) -> Result<(), RepositoryError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| RepositoryError::Storage(err.to_string()))?;

        for entry in entries {
            sqlx::query(
                r#"
                INSERT INTO ledger_entries (
                    id, external_id, kind, booked_on, gross, net, vat,
                    category, counterparty, source
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                ON CONFLICT (source, external_id) DO UPDATE SET
                    kind = EXCLUDED.kind,
                    booked_on = EXCLUDED.booked_on,
                    gross = EXCLUDED.gross,
                    net = EXCLUDED.net,
                    vat = EXCLUDED.vat,
                    category = EXCLUDED.category,
                    counterparty = EXCLUDED.counterparty,
                    updated_at = now()
                "#,
            )
            .bind(entry.id)
            .bind(&entry.external_id)
            .bind(kind_to_str(entry.kind))
            .bind(entry.booked_on)
            .bind(entry.gross.amount())
            .bind(entry.net.map(Money::amount))
            .bind(entry.vat.map(Money::amount))
            .bind(&entry.category)
            .bind(&entry.counterparty)
            .bind(source_to_str(entry.source))
            .execute(&mut *tx)
            .await
            .map_err(|err| RepositoryError::Storage(err.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|err| RepositoryError::Storage(err.to_string()))?;
        Ok(())
    }

    async fn entries_for_month(&self, month: Month) -> Result<Vec<LedgerEntry>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, external_id, kind, booked_on, gross, net, vat,
                   category, counterparty, source
              FROM ledger_entries
             WHERE booked_on >= $1 AND booked_on < $2
             ORDER BY booked_on, id
            "#,
        )
        .bind(month.start())
        .bind(month.next_start())
        .fetch_all(&self.pool)
        .await
        .map_err(|err| RepositoryError::Storage(err.to_string()))?;

        rows.into_iter().map(row_to_entry).collect()
    }
}

fn row_to_entry(row: sqlx::postgres::PgRow) -> Result<LedgerEntry, RepositoryError> {
    let kind = match row.get::<String, _>("kind").as_str() {
        "expense" => EntryKind::Expense,
        "revenue" => EntryKind::Revenue,
        other => {
            return Err(RepositoryError::Storage(format!(
                "unknown entry kind: {other}"
            )));
        }
    };
    let source = match row.get::<String, _>("source").as_str() {
        "saldeo" => SourceSystem::Saldeo,
        "manual" => SourceSystem::Manual,
        other => {
            return Err(RepositoryError::Storage(format!(
                "unknown source: {other}"
            )));
        }
    };

    let gross = Money::non_negative(row.get("gross"))
        .map_err(|err| RepositoryError::Storage(err.to_string()))?;
    let net = row
        .get::<Option<rust_decimal::Decimal>, _>("net")
        .map(Money::non_negative)
        .transpose()
        .map_err(|err| RepositoryError::Storage(err.to_string()))?;
    let vat = row
        .get::<Option<rust_decimal::Decimal>, _>("vat")
        .map(Money::non_negative)
        .transpose()
        .map_err(|err| RepositoryError::Storage(err.to_string()))?;

    Ok(LedgerEntry {
        id: row.get::<Uuid, _>("id"),
        external_id: row.get("external_id"),
        kind,
        booked_on: row.get("booked_on"),
        gross,
        net,
        vat,
        category: row.get("category"),
        counterparty: row.get("counterparty"),
        source,
    })
}

const fn kind_to_str(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Expense => "expense",
        EntryKind::Revenue => "revenue",
    }
}

const fn source_to_str(source: SourceSystem) -> &'static str {
    match source {
        SourceSystem::Saldeo => "saldeo",
        SourceSystem::Manual => "manual",
    }
}
