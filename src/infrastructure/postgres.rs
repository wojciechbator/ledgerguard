use async_trait::async_trait;
use sqlx::{PgPool, Postgres, QueryBuilder, Row};
use uuid::Uuid;

use crate::{
    application::{LedgerRepository, RepositoryError},
    domain::{EntryKind, LedgerEntry, Money, Month, SourceSystem},
};

// Ten bind parameters are emitted per row. Keeping batches at 5k stays safely
// below PostgreSQL's 65,535 bind-parameter ceiling while collapsing a 10k sync
// from 10,000 network round-trips to two statements.
const UPSERT_ROWS_PER_BATCH: usize = 5_000;

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
        if entries.is_empty() {
            return Ok(());
        }

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|err| RepositoryError::Storage(err.to_string()))?;

        for entries in entries.chunks(UPSERT_ROWS_PER_BATCH) {
            let mut query = QueryBuilder::<Postgres>::new(
                r#"
                INSERT INTO ledger_entries (
                    id, external_id, kind, booked_on, gross, net, vat,
                    category, counterparty, source
                )
                "#,
            );

            query.push_values(entries, |mut row, entry| {
                row.push_bind(entry.id)
                    .push_bind(&entry.external_id)
                    .push_bind(kind_to_str(entry.kind))
                    .push_bind(entry.booked_on)
                    .push_bind(entry.gross.amount())
                    .push_bind(entry.net.map(Money::amount))
                    .push_bind(entry.vat.map(Money::amount))
                    .push_bind(&entry.category)
                    .push_bind(&entry.counterparty)
                    .push_bind(entry.source.as_str());
            });

            query.push(
                r#"
                ON CONFLICT (source, external_id) DO UPDATE SET
                    kind = EXCLUDED.kind,
                    booked_on = EXCLUDED.booked_on,
                    gross = EXCLUDED.gross,
                    net = EXCLUDED.net,
                    vat = EXCLUDED.vat,
                    category = EXCLUDED.category,
                    counterparty = EXCLUDED.counterparty,
                    updated_at = now()
                WHERE (
                    ledger_entries.kind,
                    ledger_entries.booked_on,
                    ledger_entries.gross,
                    ledger_entries.net,
                    ledger_entries.vat,
                    ledger_entries.category,
                    ledger_entries.counterparty
                ) IS DISTINCT FROM (
                    EXCLUDED.kind,
                    EXCLUDED.booked_on,
                    EXCLUDED.gross,
                    EXCLUDED.net,
                    EXCLUDED.vat,
                    EXCLUDED.category,
                    EXCLUDED.counterparty
                )
                "#,
            );

            query
                .build()
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

    // Uses the (booked_on, id) index from migration 0003 in reverse; the
    // LIMIT keeps the dashboard preview O(limit) instead of O(month).
    async fn recent_entries_for_month(
        &self,
        month: Month,
        limit: i64,
    ) -> Result<Vec<LedgerEntry>, RepositoryError> {
        let rows = sqlx::query(
            r#"
            SELECT id, external_id, kind, booked_on, gross, net, vat,
                   category, counterparty, source
              FROM ledger_entries
             WHERE booked_on >= $1 AND booked_on < $2
             ORDER BY booked_on DESC, id DESC
             LIMIT $3
            "#,
        )
        .bind(month.start())
        .bind(month.next_start())
        .bind(limit)
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

    let source = SourceSystem::new(row.get::<String, _>("source"))
        .map_err(|err| RepositoryError::Storage(err.to_string()))?;
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
