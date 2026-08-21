use std::path::Path;

use chrono::NaiveDate;
use ledgerguard::{
    application::LedgerRepository,
    domain::{EntryKind, LedgerEntry, Money, Month, SourceSystem},
    infrastructure::postgres::PgLedgerRepository,
};
use rust_decimal_macros::dec;
use sqlx::{migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn postgres_upsert_is_idempotent_month_scoped_and_batch_safe() {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect PostgreSQL");

    Migrator::new(Path::new("migrations"))
        .await
        .expect("load migrations")
        .run(&pool)
        .await
        .expect("run migrations");

    let category_index_exists: bool = sqlx::query_scalar(
        "SELECT to_regclass('ledger_entries_category_idx') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("inspect ledger category index");
    assert!(
        !category_index_exists,
        "unused category index should not add write amplification to imports"
    );

    sqlx::query("DELETE FROM ledger_entries")
        .execute(&pool)
        .await
        .expect("clear fixture table");

    let repository = PgLedgerRepository::new(pool.clone());
    let source = SourceSystem::new("saldeo").unwrap();
    let first_id = Uuid::new_v4();

    let first = LedgerEntry {
        id: first_id,
        external_id: "doc-42".to_owned(),
        kind: EntryKind::Expense,
        booked_on: NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        gross: Money::non_negative(dec!(100.00)).unwrap(),
        net: Money::non_negative(dec!(81.30)).ok(),
        vat: Money::non_negative(dec!(18.70)).ok(),
        category: Some("software".to_owned()),
        counterparty: Some("Vendor".to_owned()),
        source: source.clone(),
    };
    repository.upsert_entries(&[first]).await.unwrap();

    let corrected = LedgerEntry {
        id: Uuid::new_v4(),
        external_id: "doc-42".to_owned(),
        kind: EntryKind::Expense,
        booked_on: NaiveDate::from_ymd_opt(2026, 8, 20).unwrap(),
        gross: Money::non_negative(dec!(123.00)).unwrap(),
        net: Money::non_negative(dec!(100.00)).ok(),
        vat: Money::non_negative(dec!(23.00)).ok(),
        category: Some("software".to_owned()),
        counterparty: Some("Vendor corrected".to_owned()),
        source: source.clone(),
    };
    repository.upsert_entries(&[corrected]).await.unwrap();

    let august = repository
        .entries_for_month(Month::new(2026, 8).unwrap())
        .await
        .unwrap();
    assert_eq!(august.len(), 1);
    assert_eq!(august[0].id, first_id);
    assert_eq!(august[0].gross.amount(), dec!(123.00));
    assert_eq!(august[0].counterparty.as_deref(), Some("Vendor corrected"));

    let september = repository
        .entries_for_month(Month::new(2026, 9).unwrap())
        .await
        .unwrap();
    assert!(september.is_empty());

    // Cross the repository's 5k statement chunk boundary. This is a regression
    // contract for the high-throughput path and for PostgreSQL's bind ceiling.
    let batch = (0..5_001)
        .map(|index| LedgerEntry {
            id: Uuid::new_v4(),
            external_id: format!("batch-{index}"),
            kind: EntryKind::Expense,
            booked_on: NaiveDate::from_ymd_opt(2026, 8, 21).unwrap(),
            gross: Money::non_negative(dec!(1.00)).unwrap(),
            net: None,
            vat: None,
            category: Some("batch".to_owned()),
            counterparty: None,
            source: source.clone(),
        })
        .collect::<Vec<_>>();
    repository.upsert_entries(&batch).await.unwrap();

    let august = repository
        .entries_for_month(Month::new(2026, 8).unwrap())
        .await
        .unwrap();
    assert_eq!(august.len(), 5_002);
}
