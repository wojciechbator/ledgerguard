use std::ops::{Add, AddAssign};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
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
