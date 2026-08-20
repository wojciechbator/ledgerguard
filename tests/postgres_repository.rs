use chrono::NaiveDate;
use ledgerguard::{
    application::LedgerRepository,
    domain::{EntryKind, LedgerEntry, Money, Month, SourceSystem},
    infrastructure::postgres::PgLedgerRepository,
};
use rust_decimal_macros::dec;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires PostgreSQL"]
async fn postgres_upsert_is_idempotent_and_month_scoped() {
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect PostgreSQL");

    sqlx::migrate!().run(&pool).await.expect("run migrations");
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
        source,
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
}
