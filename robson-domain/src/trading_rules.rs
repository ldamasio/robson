//! Runtime symbol trading rules (issue #154 deliverable 1).
//!
//! Exchange metadata (`exchangeInfo` filters) as a validated domain value:
//! tick size, lot step, minimum quantity/notional, and precisions. The
//! empirical fact motivating this module: BTCUSDT USD-M futures has
//! `tickSize = 0.10` while `pricePrecision = 2`, so precision-driven rounding
//! produces prices that are NOT tick-aligned and are accepted only through
//! undocumented Binance leniency on the algo-order path. Capital-real
//! protection must not depend on that leniency: quantization is driven by
//! `tickSize` (grid anchored at `minPrice`), never by decimal precision.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::value_objects::{DomainError, OrderSide, Price, Symbol};

/// Validated per-symbol trading rules loaded from exchange metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolTradingRules {
    symbol: Symbol,
    tick_size: Decimal,
    min_price: Decimal,
    step_size: Decimal,
    min_qty: Decimal,
    max_qty: Decimal,
    min_notional: Decimal,
    price_precision: u32,
    quantity_precision: u32,
}

impl SymbolTradingRules {
    /// Build validated rules.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidTradingRules`] when a filter value is
    /// out of range (`tick_size <= 0`, `step_size <= 0`, `min_qty <= 0`,
    /// `max_qty < min_qty`, negative `min_price`/`min_notional`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        symbol: Symbol,
        tick_size: Decimal,
        min_price: Decimal,
        step_size: Decimal,
        min_qty: Decimal,
        max_qty: Decimal,
        min_notional: Decimal,
        price_precision: u32,
        quantity_precision: u32,
    ) -> Result<Self, DomainError> {
        if tick_size <= Decimal::ZERO {
            return Err(DomainError::InvalidTradingRules(format!(
                "tick_size must be positive: {tick_size}"
            )));
        }
        if step_size <= Decimal::ZERO {
            return Err(DomainError::InvalidTradingRules(format!(
                "step_size must be positive: {step_size}"
            )));
        }
        if min_qty <= Decimal::ZERO {
            return Err(DomainError::InvalidTradingRules(format!(
                "min_qty must be positive: {min_qty}"
            )));
        }
        if max_qty < min_qty {
            return Err(DomainError::InvalidTradingRules(format!(
                "max_qty {max_qty} must be >= min_qty {min_qty}"
            )));
        }
        if min_price < Decimal::ZERO {
            return Err(DomainError::InvalidTradingRules(format!(
                "min_price must be non-negative: {min_price}"
            )));
        }
        if min_notional < Decimal::ZERO {
            return Err(DomainError::InvalidTradingRules(format!(
                "min_notional must be non-negative: {min_notional}"
            )));
        }
        Ok(Self {
            symbol,
            tick_size,
            min_price,
            step_size,
            min_qty,
            max_qty,
            min_notional,
            price_precision,
            quantity_precision,
        })
    }

    /// Symbol these rules belong to.
    pub fn symbol(&self) -> &Symbol {
        &self.symbol
    }

    /// PRICE_FILTER tick size.
    pub fn tick_size(&self) -> Decimal {
        self.tick_size
    }

    /// PRICE_FILTER minimum price (grid anchor).
    pub fn min_price(&self) -> Decimal {
        self.min_price
    }

    /// LOT_SIZE / MARKET_LOT_SIZE step size.
    pub fn step_size(&self) -> Decimal {
        self.step_size
    }

    /// LOT_SIZE / MARKET_LOT_SIZE minimum quantity.
    pub fn min_qty(&self) -> Decimal {
        self.min_qty
    }

    /// LOT_SIZE / MARKET_LOT_SIZE maximum quantity.
    pub fn max_qty(&self) -> Decimal {
        self.max_qty
    }

    /// MIN_NOTIONAL threshold.
    pub fn min_notional(&self) -> Decimal {
        self.min_notional
    }

    /// Exchange-reported price precision (informational; quantization uses
    /// `tick_size`, never this).
    pub fn price_precision(&self) -> u32 {
        self.price_precision
    }

    /// Exchange-reported quantity precision (informational).
    pub fn quantity_precision(&self) -> u32 {
        self.quantity_precision
    }

    /// True when `price` satisfies the PRICE_FILTER grid:
    /// `(price - min_price) % tick_size == 0`.
    pub fn is_tick_aligned(&self, price: Decimal) -> bool {
        let steps = (price - self.min_price) / self.tick_size;
        steps == steps.trunc()
    }

    /// Quantize a protective stop trigger to the tick grid, rounding AWAY
    /// from the position so the trigger never lands tighter than the derived
    /// executable level: a Buy stop (closing a short) rounds UP, a Sell stop
    /// (closing a long) rounds DOWN. Already-aligned prices are unchanged.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidPrice`] if the quantized value is not a
    /// valid positive price.
    pub fn quantize_stop_trigger(
        &self,
        protective_side: OrderSide,
        price: Price,
    ) -> Result<Price, DomainError> {
        let steps = (price.as_decimal() - self.min_price) / self.tick_size;
        let rounded_steps = match protective_side {
            OrderSide::Buy => steps.ceil(),
            OrderSide::Sell => steps.floor(),
        };
        Price::new(self.min_price + rounded_steps * self.tick_size)
    }

    /// Quantize an order quantity DOWN to the lot step grid. Never rounds up.
    pub fn quantize_qty_down(&self, quantity: Decimal) -> Decimal {
        (quantity / self.step_size).floor() * self.step_size
    }

    /// Validate an already-quantized order quantity against LOT_SIZE bounds.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidQuantity`] when below `min_qty`, above
    /// `max_qty`, or off the step grid.
    pub fn validate_order_qty(&self, quantity: Decimal) -> Result<(), DomainError> {
        if quantity < self.min_qty {
            return Err(DomainError::InvalidQuantity(format!(
                "Quantity {quantity} below {} minimum {}",
                self.symbol.as_pair(),
                self.min_qty
            )));
        }
        if quantity > self.max_qty {
            return Err(DomainError::InvalidQuantity(format!(
                "Quantity {quantity} above {} maximum {}",
                self.symbol.as_pair(),
                self.max_qty
            )));
        }
        let steps = quantity / self.step_size;
        if steps != steps.trunc() {
            return Err(DomainError::InvalidQuantity(format!(
                "Quantity {quantity} not aligned to {} step {}",
                self.symbol.as_pair(),
                self.step_size
            )));
        }
        Ok(())
    }

    /// Validate the order notional against MIN_NOTIONAL.
    ///
    /// # Errors
    /// Returns [`DomainError::InvalidQuantity`] when `price x quantity` is
    /// below the exchange minimum notional.
    pub fn validate_notional(&self, price: Decimal, quantity: Decimal) -> Result<(), DomainError> {
        let notional = price * quantity;
        if notional < self.min_notional {
            return Err(DomainError::InvalidQuantity(format!(
                "Notional {notional} below {} minimum {}",
                self.symbol.as_pair(),
                self.min_notional
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    /// Real BTCUSDT USD-M futures filters (verified 2026-08-02): tickSize is
    /// 0.10 while pricePrecision is 2, so tick and precision genuinely
    /// diverge on the production symbol.
    pub(crate) fn btcusdt_rules() -> SymbolTradingRules {
        SymbolTradingRules::new(
            Symbol::from_pair("BTCUSDT").unwrap(),
            dec!(0.10),
            dec!(556.80),
            dec!(0.001),
            dec!(0.001),
            dec!(1000),
            dec!(100),
            2,
            3,
        )
        .unwrap()
    }

    #[test]
    fn rejects_invalid_filters() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        let build = |tick: Decimal, step: Decimal, min_qty: Decimal, max_qty: Decimal| {
            SymbolTradingRules::new(
                symbol.clone(),
                tick,
                dec!(0),
                step,
                min_qty,
                max_qty,
                dec!(0),
                2,
                3,
            )
        };
        assert!(build(dec!(0), dec!(0.001), dec!(0.001), dec!(1000)).is_err());
        assert!(build(dec!(0.10), dec!(0), dec!(0.001), dec!(1000)).is_err());
        assert!(build(dec!(0.10), dec!(0.001), dec!(0), dec!(1000)).is_err());
        assert!(build(dec!(0.10), dec!(0.001), dec!(1), dec!(0.5)).is_err());
        assert!(build(dec!(0.10), dec!(0.001), dec!(0.001), dec!(1000)).is_ok());
    }

    #[test]
    fn tick_alignment_uses_min_price_anchor() {
        let rules = btcusdt_rules();
        // Grid anchored at 556.80 with tick 0.10: any multiple of 0.10 above
        // the anchor is aligned.
        assert!(rules.is_tick_aligned(dec!(62873.90)));
        assert!(rules.is_tick_aligned(dec!(556.80)));
        // The precision-2 rounded prices from the first real trade are NOT
        // aligned to the 0.10 tick.
        assert!(!rules.is_tick_aligned(dec!(62166.57)));
        assert!(!rules.is_tick_aligned(dec!(62405.63)));
        assert!(!rules.is_tick_aligned(dec!(63122.81)));
    }

    #[test]
    fn quantize_stop_trigger_rounds_away_from_the_position() {
        let rules = btcusdt_rules();
        let unaligned = Price::new(dec!(62873.86105)).unwrap();

        // Short protection (Buy stop above): round UP to the next tick.
        assert_eq!(
            rules.quantize_stop_trigger(OrderSide::Buy, unaligned).unwrap().as_decimal(),
            dec!(62873.90)
        );
        // Long protection (Sell stop below): round DOWN to the previous tick.
        assert_eq!(
            rules.quantize_stop_trigger(OrderSide::Sell, unaligned).unwrap().as_decimal(),
            dec!(62873.80)
        );
    }

    #[test]
    fn quantize_stop_trigger_is_identity_on_aligned_prices() {
        let rules = btcusdt_rules();
        let aligned = Price::new(dec!(62873.90)).unwrap();
        for side in [OrderSide::Buy, OrderSide::Sell] {
            assert_eq!(rules.quantize_stop_trigger(side, aligned).unwrap(), aligned);
        }
    }

    #[test]
    fn qty_quantization_and_validation() {
        let rules = btcusdt_rules();
        assert_eq!(rules.quantize_qty_down(dec!(0.0219)), dec!(0.021));
        assert_eq!(rules.quantize_qty_down(dec!(0.021)), dec!(0.021));
        assert!(rules.validate_order_qty(dec!(0.021)).is_ok());
        assert!(rules.validate_order_qty(dec!(0.0005)).is_err());
        assert!(rules.validate_order_qty(dec!(1500)).is_err());
        assert!(rules.validate_order_qty(dec!(0.0215)).is_err());
        assert!(rules.validate_notional(dec!(60000), dec!(0.002)).is_ok());
        assert!(rules.validate_notional(dec!(60000), dec!(0.001)).is_err());
    }
}
