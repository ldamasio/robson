//! Synchronous full metadata projection. No fetch, qualification or send grant.
use robson_exec::{
    balance_costs::CostIdentity,
    balance_sizing::{
        LotLimits, NotionalLimits, ObservationRef, PriceLimits, SizingError, SizingRules,
    },
    venue::VenueId,
};

use super::SpotSymbolRules;
impl SpotSymbolRules {
    /// Preserve full sizing filters from an already-read observation.
    /// Permission sets and STP remain the later caller's responsibility,
    /// not a bool proof.
    pub fn sizing_inputs(
        &self,
        account: &CostIdentity,
        observation: ObservationRef,
    ) -> Result<SizingRules, SizingError> {
        if account.venue().as_str() != VenueId::BINANCE_SPOT
            || &observation.binding.account != account
            || observation.observation_id.is_nil()
            || self.symbol != account.symbol().as_pair()
            || self.base_asset != account.symbol().base()
            || self.quote_asset != account.symbol().quote()
        {
            return Err(SizingError::Identity);
        }
        if self.status != "TRADING"
            || self.order_types.len() > 32
            || self.order_types.iter().any(|s| {
                s.len() > 32
                    || !matches!(
                        s.as_str(),
                        "LIMIT"
                            | "MARKET"
                            | "STOP_LOSS"
                            | "STOP_LOSS_LIMIT"
                            | "TAKE_PROFIT"
                            | "TAKE_PROFIT_LIMIT"
                            | "LIMIT_MAKER"
                    )
            })
            || !self.order_types.iter().any(|s| s == "MARKET")
            || !self.order_types.iter().any(|s| s == "STOP_LOSS")
        {
            return Err(SizingError::Rules);
        }
        let p = self.price_filter.as_ref().ok_or(SizingError::Rules)?;
        let l = self.lot_size.as_ref().ok_or(SizingError::Rules)?;
        let n = self.notional.as_ref().ok_or(SizingError::Rules)?;
        let lot = |v: &super::SpotLotFilter| LotLimits {
            min: v.min_qty,
            max: v.max_qty,
            step: v.step_size,
        };
        Ok(SizingRules {
            price: PriceLimits {
                min: p.min_price,
                max: p.max_price,
                tick: p.tick_size,
            },
            lot: lot(l),
            market_lot: self.market_lot_size.as_ref().map(lot),
            notional: NotionalLimits {
                min: n.min_notional,
                max: n.max_notional,
                apply_min: n.apply_min_to_market,
                apply_max: n.apply_max_to_market,
                average_minutes: n.avg_price_mins,
            },
            trading: true,
            market: true,
            stop_loss: true,
            observation,
        })
    }
}
#[cfg(test)]
#[path = "sizing_inputs_tests.rs"]
mod tests;
