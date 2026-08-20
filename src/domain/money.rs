use std::{
    ops::{Add, AddAssign},
    str::FromStr,
};

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MoneyError {
    #[error("money amount must not be negative")]
    Negative,
}

/// Monetary value in PLN.
///
/// LedgerGuard v0.1 intentionally models one accounting currency. Currency
/// conversion belongs at the integration boundary, not inside the planner.
/// JSON uses decimal strings exclusively to make the precision contract explicit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money(Decimal);

impl Money {
    pub const fn zero() -> Self {
        Self(Decimal::ZERO)
    }

    pub fn non_negative(amount: Decimal) -> Result<Self, MoneyError> {
        if amount.is_sign_negative() {
            return Err(MoneyError::Negative);
        }
        Ok(Self(amount))
    }

    pub const fn amount(self) -> Decimal {
        self.0
    }
}

impl Serialize for Money {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Money {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let amount = Decimal::from_str(&raw).map_err(de::Error::custom)?;
        Self::non_negative(amount).map_err(de::Error::custom)
    }
}

impl Add for Money {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Money {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    #[test]
    fn json_contract_is_decimal_string_only() {
        let money = Money::non_negative(dec!(123.45)).unwrap();
        assert_eq!(serde_json::to_string(&money).unwrap(), r#""123.45""#);
        assert_eq!(serde_json::from_str::<Money>(r#""123.45""#).unwrap(), money);
        assert!(serde_json::from_str::<Money>("123.45").is_err());
    }

    #[test]
    fn deserialization_preserves_non_negative_invariant() {
        let result = serde_json::from_str::<Money>(r#""-0.01""#);
        assert_eq!(
            result.unwrap_err().to_string(),
            "money amount must not be negative"
        );
    }
}
