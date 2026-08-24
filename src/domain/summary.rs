use rust_decimal::Decimal;
use serde::Serialize;

use super::{EntryKind, LedgerEntry, Money};

/// Aggregated income and costs for one calendar month, straight from the
/// normalized ledger. Gross is what hits the bank; net/VAT stay per-entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MonthSummary {
    #[serde(serialize_with = "serialize_money")]
    pub income: Money,
    #[serde(serialize_with = "serialize_money")]
    pub costs: Money,
    /// Income minus costs. Negative means the month ran a deficit so far.
    #[serde(serialize_with = "serialize_decimal_string")]
    pub net: Decimal,
    pub entries: usize,
}

impl MonthSummary {
    #[must_use]
    pub fn from_entries(entries: &[LedgerEntry]) -> Self {
        let mut income = Money::zero();
        let mut costs = Money::zero();
        let mut count = 0_usize;
        for entry in entries {
            count += 1;
            match entry.kind {
                EntryKind::Revenue => income = add(income, entry.gross),
                EntryKind::Expense => costs = add(costs, entry.gross),
            }
        }
        Self {
            net: income.amount() - costs.amount(),
            income,
            costs,
            entries: count,
        }
    }
}

fn add(left: Money, right: Money) -> Money {
    // Both operands are non-negative by construction; checked_add keeps a
    // runaway import from overflowing the money range on the read path.
    left.checked_add(right)
        .unwrap_or_else(|_| Money::max_value())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetPolicy {
    /// Planned cost ceiling for a calendar month. `None` means the operator
    /// has not declared one yet and affordability cannot be judged honestly.
    pub monthly_cost_budget: Option<Money>,
    /// When remaining headroom falls to this share of the budget (basis
    /// points), the verdict becomes Tight instead of Healthy.
    pub tight_share_basis_points: u16,
}

/// The answer to "can I take another cost this month?".
///
/// Deliberately budget-based, not cash-based: Saldeo documents do not know
/// the bank balance, and pretending they do would fabricate the one number
/// this service exists to protect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AffordabilityVerdict {
    #[serde(serialize_with = "serialize_money")]
    pub planned: Money,
    #[serde(serialize_with = "serialize_decimal_string")]
    pub headroom: Decimal,
    pub decision: Decision,
}

use super::planner::Decision;

impl BudgetPolicy {
    /// Remaining budget after this month's recorded costs and the planned
    /// spend. `None` when no budget is configured — the caller must then say
    /// so instead of inventing a verdict.
    #[must_use]
    pub fn afford(&self, summary: &MonthSummary, planned: Money) -> Option<AffordabilityVerdict> {
        let budget = self.monthly_cost_budget?;
        let tight_line = budget
            .amount()
            .checked_mul(Decimal::from(self.tight_share_basis_points))
            .and_then(|value| value.checked_div(Decimal::from(10_000)))
            .unwrap_or_default();

        let headroom = budget.amount() - summary.costs.amount() - planned.amount();

        let decision = if headroom <= Decimal::ZERO {
            Decision::Blocked
        } else if headroom <= tight_line {
            Decision::Tight
        } else {
            Decision::Healthy
        };

        Some(AffordabilityVerdict {
            planned,
            headroom,
            decision,
        })
    }
}

fn serialize_decimal_string<S>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn serialize_money<S>(value: &Money, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.amount().to_string())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use chrono::NaiveDate;
    use rust_decimal_macros::dec;
    use uuid::Uuid;

    use super::*;
    use crate::domain::SourceSystem;

    fn entry(kind: EntryKind, gross: Decimal) -> LedgerEntry {
        LedgerEntry {
            id: Uuid::new_v4(),
            external_id: format!("ext-{}-{}", gross, count()),
            kind,
            booked_on: NaiveDate::from_ymd_opt(2026, 8, 14).expect("valid date"),
            gross: Money::non_negative(gross).unwrap(),
            net: None,
            vat: None,
            category: None,
            counterparty: None,
            source: SourceSystem::manual(),
        }
    }

    fn count() -> usize {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed) as usize
    }

    #[test]
    fn summarizes_income_and_costs_by_kind() {
        let entries = [
            entry(EntryKind::Revenue, dec!(12_000)),
            entry(EntryKind::Expense, dec!(3_000)),
            entry(EntryKind::Expense, dec!(1_500)),
            entry(EntryKind::Revenue, dec!(800)),
        ];
        let summary = MonthSummary::from_entries(&entries);
        assert_eq!(summary.income.amount(), dec!(12_800));
        assert_eq!(summary.costs.amount(), dec!(4_500));
        assert_eq!(summary.net, dec!(8_300));
        assert_eq!(summary.entries, 4);
    }

    #[test]
    fn empty_month_is_all_zero_not_an_error() {
        let summary = MonthSummary::from_entries(&[]);
        assert_eq!(summary.net, Decimal::ZERO);
        assert_eq!(summary.entries, 0);
    }

    #[test]
    fn deficit_is_a_negative_net() {
        let entries = [entry(EntryKind::Expense, dec!(2_000))];
        assert_eq!(MonthSummary::from_entries(&entries).net, dec!(-2_000));
    }

    #[test]
    fn no_budget_configured_refuses_a_verdict() {
        let policy = BudgetPolicy {
            monthly_cost_budget: None,
            tight_share_basis_points: 1_000,
        };
        let summary = MonthSummary::default_of_costs(dec!(100));
        assert!(policy.afford(&summary, money(dec!(10))).is_none());
    }

    #[test]
    fn healthy_above_tight_share_and_blocked_at_zero() {
        let policy = BudgetPolicy {
            monthly_cost_budget: Some(money(dec!(10_000))),
            tight_share_basis_points: 1_000,
        };
        let spent = MonthSummary::default_of_costs(dec!(6_000));

        // 4 000 left of 10 000 = above the 10 % line.
        assert_eq!(
            policy.afford(&spent, money(dec!(0))).unwrap().decision,
            Decision::Healthy
        );
        // Taking 3 500 more leaves 500 = below the 1 000 line but positive.
        assert_eq!(
            policy.afford(&spent, money(dec!(3_500))).unwrap().decision,
            Decision::Tight
        );
        // Taking 4 000 more leaves zero: blocked, not tight.
        assert_eq!(
            policy.afford(&spent, money(dec!(4_000))).unwrap().decision,
            Decision::Blocked
        );
        // Overdrawing goes negative and stays Blocked.
        assert_eq!(
            policy.afford(&spent, money(dec!(5_000))).unwrap().decision,
            Decision::Blocked
        );
    }

    #[test]
    fn verdict_serializes_amounts_as_decimal_strings() {
        let policy = BudgetPolicy {
            monthly_cost_budget: Some(money(dec!(1_000))),
            tight_share_basis_points: 1_000,
        };
        let summary = MonthSummary::default_of_costs(dec!(100));
        let json = serde_json::to_value(policy.afford(&summary, money(dec!(50))).unwrap()).unwrap();
        assert_eq!(json["headroom"], "850");
        assert_eq!(json["planned"], "50");
    }

    fn money(value: Decimal) -> Money {
        Money::non_negative(value).unwrap()
    }

    impl MonthSummary {
        fn default_of_costs(costs: Decimal) -> Self {
            Self {
                income: Money::zero(),
                costs: money(costs),
                net: -costs,
                entries: 1,
            }
        }
    }

    #[allow(dead_code)]
    fn _assert_from_str_still_used() {
        let _ = Decimal::from_str("1");
    }
}
