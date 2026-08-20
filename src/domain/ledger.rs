use std::fmt;

use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;
use uuid::Uuid;

use super::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Expense,
    Revenue,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SourceSystemError {
    #[error("source system must not be empty")]
    Empty,
    #[error("source system must be at most 32 characters")]
    TooLong,
    #[error("source system must be a lowercase slug using [a-z0-9_-]")]
    InvalidCharacter,
}

/// Stable provenance identifier for a normalized ledger entry.
///
/// This is deliberately provider-agnostic so adding a new accounting adapter
/// never requires changing the planner domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceSystem(String);

impl SourceSystem {
    pub fn new(value: impl Into<String>) -> Result<Self, SourceSystemError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SourceSystemError::Empty);
        }
        if value.len() > 32 {
            return Err(SourceSystemError::TooLong);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-')
        {
            return Err(SourceSystemError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn manual() -> Self {
        Self("manual".to_owned())
    }
}

impl fmt::Display for SourceSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SourceSystem {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SourceSystem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(de::Error::custom)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_system_is_open_for_new_adapters_but_validated() {
        assert_eq!(SourceSystem::new("ifirma").unwrap().as_str(), "ifirma");
        assert_eq!(
            SourceSystem::new("Bad Provider").unwrap_err(),
            SourceSystemError::InvalidCharacter
        );
    }

    #[test]
    fn source_deserialization_preserves_slug_invariant() {
        let result = serde_json::from_str::<SourceSystem>(r#""UPPER""#);
        assert_eq!(
            result.unwrap_err().to_string(),
            "source system must be a lowercase slug using [a-z0-9_-]"
        );
    }
}
