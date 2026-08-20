use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, Serializer};

use super::Money;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Healthy,
    Tight,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerPolicy {
    /// Extra headroom required after all explicit reserves and buffers.
    pub tight_threshold: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannerInput {
    /// Current spendable cash snapshot. Already-paid historical expenses must
    /// not be deducted again from this value.
    pub available_cash: Money,
    pub committed_costs: Money,
    pub tax_reserve: Money,
    pub vat_reserve: Money,
    pub zus_reserve: Money,
    pub minimum_cash_buffer: Money,
    pub planned_spend: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PlannerResult {
    /// Signed amount after obligations and buffers. Negative means deficit.
    #[serde(serialize_with = "serialize_decimal_string")]
    pub headroom: Decimal,
    /// Amount that can be spent without crossing the configured floor.
    pub safe_to_spend: Money,
    pub decision: Decision,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Planner;

impl Planner {
    #[must_use]
    pub fn evaluate(input: PlannerInput, policy: PlannerPolicy) -> PlannerResult {
        Self::evaluate_with_extra_spend(input, policy, Decimal::ZERO)
    }

    #[must_use]
    pub fn simulate_purchase(
        input: PlannerInput,
        policy: PlannerPolicy,
        purchase_gross: Money,
    ) -> PlannerResult {
        Self::evaluate_with_extra_spend(input, policy, purchase_gross.amount())
    }

    fn evaluate_with_extra_spend(
        input: PlannerInput,
        policy: PlannerPolicy,
        extra_spend: Decimal,
    ) -> PlannerResult {
        let deductions = input.committed_costs.amount()
            + input.tax_reserve.amount()
            + input.vat_reserve.amount()
            + input.zus_reserve.amount()
            + input.minimum_cash_buffer.amount()
            + input.planned_spend.amount()
            + extra_spend;
        let headroom = input.available_cash.amount() - deductions;
        let safe_to_spend = Money::non_negative(headroom.max(Decimal::ZERO))
            .expect("headroom cannot exceed the validated available-cash bound");

        let decision = if headroom <= Decimal::ZERO {
            Decision::Blocked
        } else if headroom <= policy.tight_threshold.amount() {
            Decision::Tight
        } else {
            Decision::Healthy
        };

        PlannerResult {
            headroom,
            safe_to_spend,
            decision,
        }
    }
}

fn serialize_decimal_string<S>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    use super::*;

    fn money(value: Decimal) -> Money {
        Money::non_negative(value).unwrap()
    }

    #[test]
    fn calculates_safe_to_spend_without_double_counting_history() {
        let result = Planner::evaluate(
            PlannerInput {
                available_cash: money(dec!(30_000)),
                committed_costs: money(dec!(2_000)),
                tax_reserve: money(dec!(4_000)),
                vat_reserve: money(dec!(3_000)),
                zus_reserve: money(dec!(2_000)),
                minimum_cash_buffer: money(dec!(10_000)),
                planned_spend: money(dec!(1_000)),
            },
            PlannerPolicy {
                tight_threshold: money(dec!(2_000)),
            },
        );

        assert_eq!(result.headroom, dec!(8_000));
        assert_eq!(result.safe_to_spend.amount(), dec!(8_000));
        assert_eq!(result.decision, Decision::Healthy);
    }

    #[test]
    fn purchase_can_move_plan_into_blocked_state() {
        let input = PlannerInput {
            available_cash: money(dec!(20_000)),
            committed_costs: money(dec!(2_000)),
            tax_reserve: money(dec!(3_000)),
            vat_reserve: money(dec!(2_000)),
            zus_reserve: money(dec!(1_000)),
            minimum_cash_buffer: money(dec!(8_000)),
            planned_spend: money(dec!(1_000)),
        };
        let policy = PlannerPolicy {
            tight_threshold: money(dec!(1_500)),
        };

        let result = Planner::simulate_purchase(input, policy, money(dec!(4_000)));

        assert_eq!(result.headroom, dec!(-1_000));
        assert_eq!(result.safe_to_spend.amount(), Decimal::ZERO);
        assert_eq!(result.decision, Decision::Blocked);
    }

    #[test]
    fn very_large_purchase_does_not_mutate_money_outside_its_range() {
        let input = PlannerInput {
            available_cash: money(dec!(1)),
            committed_costs: Money::zero(),
            tax_reserve: Money::zero(),
            vat_reserve: Money::zero(),
            zus_reserve: Money::zero(),
            minimum_cash_buffer: Money::zero(),
            planned_spend: money(Decimal::from_str("999999999999999999.99").unwrap()),
        };
        let result = Planner::simulate_purchase(
            input,
            PlannerPolicy {
                tight_threshold: Money::zero(),
            },
            money(dec!(0.01)),
        );

        assert_eq!(result.decision, Decision::Blocked);
        assert_eq!(result.safe_to_spend, Money::zero());
    }

    #[test]
    fn planner_result_serializes_all_amounts_as_decimal_strings() {
        let result = Planner::evaluate(
            PlannerInput {
                available_cash: money(dec!(10)),
                committed_costs: Money::zero(),
                tax_reserve: Money::zero(),
                vat_reserve: Money::zero(),
                zus_reserve: Money::zero(),
                minimum_cash_buffer: Money::zero(),
                planned_spend: Money::zero(),
            },
            PlannerPolicy {
                tight_threshold: Money::zero(),
            },
        );

        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["headroom"], "10");
        assert_eq!(json["safe_to_spend"], "10");
    }
}
