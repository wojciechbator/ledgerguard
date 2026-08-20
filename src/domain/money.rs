use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use thiserror::Error;

const MAX_AMOUNT: &str = "999999999999999999.99";
const MAX_SCALE: u32 = 2;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MoneyError {
    #[error("money amount must not be negative")]
    Negative,
    #[error("money amount must have at most two fractional digits")]
    TooPrecise,
    #[error("money amount exceeds the supported range")]
    TooLarge,
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
        let amount = amount.normalize();
        if amount.is_sign_negative() {
            return Err(MoneyError::Negative);
        }
        if amount.scale() > MAX_SCALE {
            return Err(MoneyError::TooPrecise);
        }
        let maximum = Decimal::from_str(MAX_AMOUNT).expect("MAX_AMOUNT is a valid decimal");
        if amount > maximum {
            return Err(MoneyError::TooLarge);
        }
        Ok(Self(amount))
    }

    pub const fn amount(self) -> Decimal {
        self.0
    }

    pub fn checked_add(self, rhs: Self) -> Result<Self, MoneyError> {
        Self::non_negative(self.0 + rhs.0)
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

    #[test]
    fn rejects_precision_that_postgres_would_round() {
        assert_eq!(
            Money::non_negative(dec!(1.001)).unwrap_err(),
            MoneyError::TooPrecise
        );
        assert_eq!(
            Money::non_negative(dec!(1.230)).unwrap().amount(),
            dec!(1.23)
        );
    }

    #[test]
    fn rejects_values_outside_numeric_20_2_storage_range() {
        let too_large = Decimal::from_str("1000000000000000000").unwrap();
        assert_eq!(
            Money::non_negative(too_large).unwrap_err(),
            MoneyError::TooLarge
        );
    }

    #[test]
    fn addition_cannot_bypass_supported_range() {
        let maximum = Money::non_negative(Decimal::from_str(MAX_AMOUNT).unwrap()).unwrap();
        let one_cent = Money::non_negative(dec!(0.01)).unwrap();
        assert_eq!(
            maximum.checked_add(one_cent).unwrap_err(),
            MoneyError::TooLarge
        );
    }
}
