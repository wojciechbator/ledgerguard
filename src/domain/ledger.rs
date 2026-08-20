use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Expense,
    Revenue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSystem {
    Saldeo,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub id: Uuid,
    pub external_id: String,
    pub kind: EntryKind,
    pub booked_on: NaiveDate,
    pub gross: Money,
    pub net: Option<Money>,
    pub vat: Option<Money>,
    pub category: Option<String>,
    pub counterparty: Option<String>,
    pub source: SourceSystem,
}
