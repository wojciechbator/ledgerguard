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

/// Cost ceiling as a share of monthly income. Hardcoded business rules:
/// costs under 70% of income are Healthy, 70-85% are Tight, over 85% are
/// Blocked. The 80% stretch line is shown in the UI as a reference.
const HEALTHY_CEILING_BP: u16 = 7_000; // 70%
const STRETCH_CEILING_BP: u16 = 8_000; // 80% — display reference
const MAX_CEILING_BP: u16 = 8_500; // 85%

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetPolicy {
    /// Expected monthly income (gross PLN). The cost budget is derived from
    /// this: 70% = Healthy ceiling, 85% = Blocked ceiling.
    pub monthly_income: Money,
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

fn bp_to_decimal(bp: u16) -> Decimal {
    Decimal::from(bp) / Decimal::from(10_000)
}

impl BudgetPolicy {
    /// 70% of income — costs above this are Tight.
    #[must_use]
    pub fn healthy_ceiling(&self) -> Money {
        Money::non_negative(self.monthly_income.amount() * bp_to_decimal(HEALTHY_CEILING_BP))
            .unwrap_or_else(|_| Money::zero())
    }

    /// 80% of income — the stretch reference line shown in the UI.
    #[must_use]
    pub fn stretch_ceiling(&self) -> Money {
        Money::non_negative(self.monthly_income.amount() * bp_to_decimal(STRETCH_CEILING_BP))
            .unwrap_or_else(|_| Money::zero())
    }

    /// 85% of income — costs above this are Blocked.
    #[must_use]
    pub fn max_ceiling(&self) -> Money {
        Money::non_negative(self.monthly_income.amount() * bp_to_decimal(MAX_CEILING_BP))
            .unwrap_or_else(|_| Money::zero())
    }

    /// Always returns a verdict — the budget is derived from income, so
    /// there is no "not configured" state.
    #[must_use]
    pub fn afford(&self, summary: &MonthSummary, planned: Money) -> AffordabilityVerdict {
        let max_ceiling = self.max_ceiling().amount();
        let healthy_ceiling = self.healthy_ceiling().amount();
        let total = summary.costs.amount() + planned.amount();
        let headroom = max_ceiling - total;

        let decision = if total >= max_ceiling {
            Decision::Blocked
        } else if total > healthy_ceiling {
            Decision::Tight
        } else {
            Decision::Healthy
        };

        AffordabilityVerdict {
            planned,
            headroom,
            decision,
        }
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
    fn income_derived_budget_always_returns_a_verdict() {
        let policy = BudgetPolicy {
            monthly_income: money(dec!(10_000)),
        };
        let summary = MonthSummary::default_of_costs(dec!(100));
        // No "not configured" state — always returns a verdict.
        let verdict = policy.afford(&summary, money(dec!(10)));
        assert_eq!(verdict.decision, Decision::Healthy);
    }

    #[test]
    fn healthy_below_70_percent_tight_70_to_85_blocked_above_85() {
        let policy = BudgetPolicy {
            monthly_income: money(dec!(10_000)),
        };
        // 70% of 10 000 = 7 000, 85% = 8 500

        // 5 000 costs = 50% → Healthy
        let spent = MonthSummary::default_of_costs(dec!(5_000));
        assert_eq!(
            policy.afford(&spent, money(dec!(0))).decision,
            Decision::Healthy
        );

        // 7 500 costs = 75% → Tight (above 70%, below 85%)
        let spent = MonthSummary::default_of_costs(dec!(7_500));
        assert_eq!(
            policy.afford(&spent, money(dec!(0))).decision,
            Decision::Tight
        );

        // 8 500 costs = 85% → Blocked (at the ceiling)
        let spent = MonthSummary::default_of_costs(dec!(8_500));
        assert_eq!(
            policy.afford(&spent, money(dec!(0))).decision,
            Decision::Blocked
        );

        // 9 000 costs = 90% → Blocked (over the ceiling)
        let spent = MonthSummary::default_of_costs(dec!(9_000));
        assert_eq!(
            policy.afford(&spent, money(dec!(0))).decision,
            Decision::Blocked
        );

        // Planned spend pushes total over 85%: 6 000 costs + 3 000 planned = 90%
        let spent = MonthSummary::default_of_costs(dec!(6_000));
        assert_eq!(
            policy.afford(&spent, money(dec!(3_000))).decision,
            Decision::Blocked
        );

        // Planned spend pushes total into Tight: 5 000 costs + 2 500 planned = 75%
        let spent = MonthSummary::default_of_costs(dec!(5_000));
        assert_eq!(
            policy.afford(&spent, money(dec!(2_500))).decision,
            Decision::Tight
        );
    }

    #[test]
    fn derived_ceilings_are_correct_fractions_of_income() {
        let policy = BudgetPolicy {
            monthly_income: money(dec!(26_500)),
        };
        // 70% of 26 500 = 18 550
        assert_eq!(policy.healthy_ceiling().amount(), dec!(18_550));
        // 80% of 26 500 = 21 200
        assert_eq!(policy.stretch_ceiling().amount(), dec!(21_200));
        // 85% of 26 500 = 22 525
        assert_eq!(policy.max_ceiling().amount(), dec!(22_525));
    }

    #[test]
    fn verdict_serializes_amounts_as_decimal_strings() {
        let policy = BudgetPolicy {
            monthly_income: money(dec!(10_000)),
        };
        let summary = MonthSummary::default_of_costs(dec!(100));
        let json = serde_json::to_value(policy.afford(&summary, money(dec!(50)))).unwrap();
        // headroom = 8500 - 100 - 50 = 8350
        assert_eq!(json["headroom"], "8350");
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
