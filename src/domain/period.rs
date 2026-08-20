use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MonthError {
    #[error("month must be in 1..=12")]
    InvalidMonth,
    #[error("year is outside the supported range")]
    InvalidYear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Month {
    pub year: i32,
    pub month: u32,
}

impl Month {
    pub fn new(year: i32, month: u32) -> Result<Self, MonthError> {
        if !(1..=12).contains(&month) {
            return Err(MonthError::InvalidMonth);
        }
        if !(2000..=2200).contains(&year) {
            return Err(MonthError::InvalidYear);
        }
        Ok(Self { year, month })
    }

    pub fn start(self) -> NaiveDate {
        NaiveDate::from_ymd_opt(self.year, self.month, 1)
            .expect("validated month must always produce a date")
    }

    pub fn next_start(self) -> NaiveDate {
        if self.month == 12 {
            NaiveDate::from_ymd_opt(self.year + 1, 1, 1)
                .expect("validated year must always produce a date")
        } else {
            NaiveDate::from_ymd_opt(self.year, self.month + 1, 1)
                .expect("validated month must always produce a date")
        }
    }
}
