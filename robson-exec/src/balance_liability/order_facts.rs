use robson_domain::OrderSide;
use rust_decimal::Decimal;

use super::FactError;
use crate::venue_ledger::{exact_sub, OrderKey, OrderOwner, StoredOrder};

/// Local numeric/shape facts only. No lifecycle, currentness or R3 verdict.
/// Contains no borrowed identity or allocated payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderFacts {
    remaining: Decimal,
    sell: bool,
    stop_loss: bool,
    trigger: Option<Decimal>,
}

impl OrderFacts {
    pub fn remaining(&self) -> Decimal {
        self.remaining
    }

    /// A shape match is NOT protection. Status, cancellations, complete sibling
    /// evidence and tenant/current-version qualification remain later checks.
    pub fn matches_protection_shape(
        &self,
        governed_remainder: Decimal,
        target: Decimal,
    ) -> Result<bool, FactError> {
        if governed_remainder < Decimal::ZERO {
            return Err(FactError::InvalidQuantity);
        }
        if target <= Decimal::ZERO {
            return Err(FactError::InvalidTarget);
        }
        Ok(self.sell
            && self.stop_loss
            && self.remaining == governed_remainder
            && self.trigger.is_some_and(|trigger| trigger > Decimal::ZERO && trigger >= target))
    }
}

/// Compare the canonical key and exact governed link, not a cancel-renamed
/// client ID. Expected values are untrusted; this does not authenticate tenant
/// or evidence provenance. String comparisons cost O(input identity bytes).
pub fn order_facts(
    order: &StoredOrder,
    expected_key: &OrderKey,
    expected_owner: &OrderOwner,
) -> Result<OrderFacts, FactError> {
    if !matches!(expected_owner, OrderOwner::Leg { .. })
        || order.owner != *expected_owner
        || order.key != *expected_key
        || expected_key.venue.as_str().is_empty()
        || expected_key.account_scope.is_empty()
        || expected_key.symbol.is_empty()
    {
        return Err(FactError::IdentityMismatch);
    }
    if order.executed_cum < Decimal::ZERO || order.executed_cum > order.orig_qty {
        return Err(FactError::InvalidQuantity);
    }
    Ok(OrderFacts {
        remaining: exact_sub(order.orig_qty, order.executed_cum)
            .map_err(|_| FactError::Unrepresentable)?,
        sell: order.side == OrderSide::Sell,
        stop_loss: order.order_type == "STOP_LOSS",
        trigger: order.stop_price,
    })
}
