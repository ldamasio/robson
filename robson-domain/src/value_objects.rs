//! Value Objects for Robson v2 Domain
//!
//! Immutable, validated domain primitives.
//! All value objects enforce invariants at construction time.

use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Domain errors for value object validation
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// Price must be positive
    #[error("Invalid price: {0}")]
    InvalidPrice(String),

    /// Quantity must be positive
    #[error("Invalid quantity: {0}")]
    InvalidQuantity(String),

    /// Symbol must be valid trading pair
    #[error("Invalid symbol: {0}")]
    InvalidSymbol(String),

    /// TechnicalStopDistance validation error
    #[error("Invalid technical stop distance: {0}")]
    InvalidTechnicalStopDistance(String),

    /// RiskConfig validation error
    #[error("Invalid risk config: {0}")]
    InvalidRiskConfig(String),

    /// Position sizing error
    #[error("Position sizing error: {0}")]
    PositionSizingError(String),

    /// Invalid signal (mismatched position, symbol, or side)
    #[error("Invalid signal: {0}")]
    InvalidSignal(String),

    /// Invalid state transition
    #[error("Invalid state transition: {0}")]
    InvalidStateTransition(String),
}

// =============================================================================
// Price
// =============================================================================

/// Price represents a positive decimal price
///
/// # Invariants
/// - Must be > 0
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Price(Decimal);

impl Price {
    /// Create a new Price with validation
    ///
    /// # Errors
    /// Returns `DomainError::InvalidPrice` if value <= 0
    pub fn new(value: Decimal) -> Result<Self, DomainError> {
        if value <= Decimal::ZERO {
            return Err(DomainError::InvalidPrice("Price must be positive".to_string()));
        }
        Ok(Self(value))
    }

    /// Get the underlying Decimal value
    pub fn as_decimal(&self) -> Decimal {
        self.0
    }

    /// Create a zero price (for initialization only)
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// Quantity
// =============================================================================

/// Quantity represents a positive decimal quantity
///
/// # Invariants
/// - Must be > 0
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Quantity(Decimal);

impl Quantity {
    /// Create a new Quantity with validation
    ///
    /// # Errors
    /// Returns `DomainError::InvalidQuantity` if value <= 0
    pub fn new(value: Decimal) -> Result<Self, DomainError> {
        if value <= Decimal::ZERO {
            return Err(DomainError::InvalidQuantity("Quantity must be positive".to_string()));
        }
        Ok(Self(value))
    }

    /// Get the underlying Decimal value
    pub fn as_decimal(&self) -> Decimal {
        self.0
    }

    /// Create a zero quantity (for initialization only)
    pub fn zero() -> Self {
        Self(Decimal::ZERO)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// =============================================================================
// Symbol
// =============================================================================

/// Symbol represents a trading pair (e.g., BTCUSDT)
///
/// # Invariants
/// - Must be valid format (base + quote)
/// - Base and quote must be non-empty
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    base: String,
    quote: String,
}

impl Symbol {
    /// Create a Symbol from a trading pair string
    ///
    /// # Examples
    /// ```
    /// # use robson_domain::value_objects::Symbol;
    /// let symbol = Symbol::from_pair("BTCUSDT").unwrap();
    /// assert_eq!(symbol.base(), "BTC");
    /// assert_eq!(symbol.quote(), "USDT");
    /// ```
    ///
    /// # Errors
    /// Returns `DomainError::InvalidSymbol` if format is invalid
    pub fn from_pair(pair: &str) -> Result<Self, DomainError> {
        // Common quote currencies (extend as needed)
        const QUOTE_CURRENCIES: &[&str] = &["USDT", "BUSD", "BTC", "ETH", "BNB"];

        for quote in QUOTE_CURRENCIES {
            if let Some(base) = pair.strip_suffix(quote) {
                if !base.is_empty() {
                    return Ok(Self {
                        base: base.to_string(),
                        quote: quote.to_string(),
                    });
                }
            }
        }

        Err(DomainError::InvalidSymbol(format!("Cannot parse trading pair: {}", pair)))
    }

    /// Create a Symbol from explicit base and quote
    pub fn new(base: String, quote: String) -> Result<Self, DomainError> {
        if base.is_empty() || quote.is_empty() {
            return Err(DomainError::InvalidSymbol("Base and quote must be non-empty".to_string()));
        }
        Ok(Self { base, quote })
    }

    /// Get the base currency
    pub fn base(&self) -> &str {
        &self.base
    }

    /// Get the quote currency
    pub fn quote(&self) -> &str {
        &self.quote
    }

    /// Get the trading pair as string (e.g., "BTCUSDT")
    pub fn as_pair(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_pair())
    }
}

// =============================================================================
// Side
// =============================================================================

/// Side represents the position direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Long position (buy low, sell high)
    Long,
    /// Short position (sell high, buy low)
    Short,
}

impl Side {
    /// Get the entry action for this side
    ///
    /// Long → Buy, Short → Sell
    pub fn entry_action(&self) -> OrderSide {
        match self {
            Side::Long => OrderSide::Buy,
            Side::Short => OrderSide::Sell,
        }
    }

    /// Get the exit action for this side
    ///
    /// Long → Sell, Short → Buy
    pub fn exit_action(&self) -> OrderSide {
        match self {
            Side::Long => OrderSide::Sell,
            Side::Short => OrderSide::Buy,
        }
    }
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Long => write!(f, "LONG"),
            Side::Short => write!(f, "SHORT"),
        }
    }
}

/// OrderSide represents the order direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderSide {
    /// Buy order
    Buy,
    /// Sell order
    Sell,
}

impl fmt::Display for OrderSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderSide::Buy => write!(f, "BUY"),
            OrderSide::Sell => write!(f, "SELL"),
        }
    }
}

// =============================================================================
// RiskConfig
// =============================================================================

/// Risk configuration for position sizing (v3 policy)
///
/// Risk per trade is FIXED at 1% of capital. This is a v3 policy decision:
/// no configuration, no alternative modes, no overrides.
///
/// Position size is derived from the Golden Rule:
///
/// ```text
/// Position Size = (Capital × 1%) / Stop Distance
/// ```
///
/// # Example
///
/// ```
/// # use robson_domain::value_objects::RiskConfig;
/// # use rust_decimal_macros::dec;
/// let config = RiskConfig::new(dec!(10000)).unwrap();
/// assert_eq!(config.capital(), dec!(10000));
/// assert_eq!(config.risk_per_trade_pct(), dec!(1));
/// assert_eq!(config.max_risk_amount(), dec!(100)); // 1% of 10000
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskConfig {
    /// Available capital in quote currency (e.g., USDT)
    capital: Decimal,
}

impl RiskConfig {
    /// Fixed leverage
    pub const LEVERAGE: u8 = 10;

    /// Fixed risk per trade: 1% of capital (v3 policy — non-negotiable)
    pub const RISK_PER_TRADE_PCT: Decimal = Decimal::ONE;

    /// Create a new RiskConfig with validation
    ///
    /// Risk is always 1% — not configurable. Only capital is provided.
    ///
    /// # Errors
    /// Returns `DomainError::InvalidRiskConfig` if capital <= 0
    pub fn new(capital: Decimal) -> Result<Self, DomainError> {
        if capital <= Decimal::ZERO {
            return Err(DomainError::InvalidRiskConfig("Capital must be positive".to_string()));
        }

        Ok(Self { capital })
    }

    /// Get capital
    pub fn capital(&self) -> Decimal {
        self.capital
    }

    /// Get risk percentage (always 1%)
    pub fn risk_per_trade_pct(&self) -> Decimal {
        Self::RISK_PER_TRADE_PCT
    }

    /// Calculate max risk amount in quote currency
    ///
    /// Returns: Capital × 1% (fixed)
    pub fn max_risk_amount(&self) -> Decimal {
        self.capital * Self::RISK_PER_TRADE_PCT / Decimal::from(100)
    }

    /// Get fixed leverage
    pub fn leverage(&self) -> u8 {
        Self::LEVERAGE
    }
}

impl fmt::Display for RiskConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RiskConfig {{ capital: {}, risk: 1% (fixed), leverage: {}x }}",
            self.capital,
            Self::LEVERAGE
        )
    }
}

// =============================================================================
// TechnicalStopDistance
// =============================================================================

/// TechnicalStopDistance represents the distance from entry to technical stop
///
/// This is the structural foundation of position sizing AND trailing stop
/// logic. The distance is called the "span" (palmo) — the central unit of the
/// position.
///
/// # Technical Stop Calculation
///
/// The technical stop is determined at position entry by:
/// - **LONG**: Second technical support on the 15-minute chart
/// - **SHORT**: Second technical resistance on the 15-minute chart
///
/// # The Span (Palmo)
///
/// `span = abs(entry - technical_stop)`
///
/// The span is:
/// - Unit of risk (position size = 1% of capital / span)
/// - Unit of movement (trailing stop moves in integer multiples of span)
/// - Unit of decision (only complete span events trigger action)
///
/// # Trailing Stop Logic (v3 — Discrete Step)
///
/// The trailing stop moves in INTEGER MULTIPLES of the span:
///
/// For LONG:
///   - entry=95000, stop=93500, span=1500
///   - price reaches 96500 (entry + 1×span) → stop moves to 95000
///   - price reaches 98000 (entry + 2×span) → stop moves to 96500
///   - Partial movements are IGNORED
///
/// For SHORT: symmetric (stop moves down in span steps)
///
/// Key: the stop NEVER reacts to partial price movement.
///
/// # Invariants
/// - Distance must be positive
/// - Distance percentage must be between 0.1% and 10%
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TechnicalStopDistance {
    /// Absolute distance in quote currency (used for trailing)
    pub distance: Decimal,
    /// Percentage of entry price
    pub distance_pct: Decimal,
    /// Entry price
    pub entry_price: Price,
    /// Initial technical stop price (from chart analysis)
    pub initial_stop: Price,
}

impl TechnicalStopDistance {
    /// Create TechnicalStopDistance from entry and technical stop prices
    ///
    /// # Examples
    /// ```
    /// # use robson_domain::value_objects::{TechnicalStopDistance, Price};
    /// # use rust_decimal_macros::dec;
    /// let entry = Price::new(dec!(95000.0)).unwrap();
    /// let stop = Price::new(dec!(93500.0)).unwrap();
    /// let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
    ///
    /// assert_eq!(tech_stop.distance, dec!(1500.0));
    /// // Distance percentage = 1500 / 95000 * 100 = 1.578...%
    /// ```
    pub fn from_entry_and_stop(entry: Price, initial_stop: Price) -> Self {
        let distance = (entry.as_decimal() - initial_stop.as_decimal()).abs();
        let distance_pct = if entry.as_decimal() != Decimal::ZERO {
            distance / entry.as_decimal() * Decimal::from(100)
        } else {
            Decimal::ZERO
        };

        Self {
            distance,
            distance_pct,
            entry_price: entry,
            initial_stop,
        }
    }

    /// Create TechnicalStopDistance with side-aware validation (hard-stop
    /// invariants)
    ///
    /// This constructor enforces critical domain invariants ONLY:
    /// - Distance must be > 0 (stop cannot equal entry price)
    /// - For LONG: stop must be below entry
    /// - For SHORT: stop must be above entry
    ///
    /// Note: Percentage bounds (0.1% to 10%) are enforced at the engine/policy
    /// level, not in the domain. Use this constructor when you only need
    /// domain invariant validation.
    ///
    /// # Errors
    ///
    /// Returns `DomainError::InvalidTechnicalStopDistance` if invariants are
    /// violated.
    ///
    /// # Examples
    ///
    /// ```
    /// # use robson_domain::value_objects::{TechnicalStopDistance, Price, Side};
    /// # use rust_decimal_macros::dec;
    /// let entry = Price::new(dec!(95000)).unwrap();
    /// let stop = Price::new(dec!(93500)).unwrap();
    ///
    /// // Valid LONG setup (stop below entry)
    /// let tech_stop = TechnicalStopDistance::new_validated(entry, stop, Side::Long).unwrap();
    ///
    /// // Invalid LONG setup (stop above entry)
    /// let bad_stop = Price::new(dec!(96500)).unwrap();
    /// assert!(TechnicalStopDistance::new_validated(entry, bad_stop, Side::Long).is_err());
    ///
    /// // Invalid setup (stop at same price as entry)
    /// let same_stop = Price::new(dec!(95000)).unwrap();
    /// assert!(TechnicalStopDistance::new_validated(entry, same_stop, Side::Long).is_err());
    /// ```
    pub fn new_validated(
        entry: Price,
        initial_stop: Price,
        side: Side,
    ) -> Result<Self, DomainError> {
        // Check distance > 0 (hard-stop: stop cannot be at same price as entry)
        let distance = (entry.as_decimal() - initial_stop.as_decimal()).abs();
        if distance <= Decimal::ZERO {
            return Err(DomainError::InvalidTechnicalStopDistance(
                "Stop distance must be positive (stop cannot equal entry price)".to_string(),
            ));
        }

        // Check stop is on correct side for position direction
        match side {
            Side::Long => {
                if initial_stop.as_decimal() >= entry.as_decimal() {
                    return Err(DomainError::InvalidTechnicalStopDistance(
                        "LONG position requires stop below entry price".to_string(),
                    ));
                }
            },
            Side::Short => {
                if initial_stop.as_decimal() <= entry.as_decimal() {
                    return Err(DomainError::InvalidTechnicalStopDistance(
                        "SHORT position requires stop above entry price".to_string(),
                    ));
                }
            },
        }

        // Note: We do NOT call validate() here - percentage bounds are policy, not
        // domain invariants
        Ok(Self::from_entry_and_stop(entry, initial_stop))
    }

    /// Validate the TechnicalStopDistance
    ///
    /// # Errors
    /// Returns `DomainError::InvalidTechnicalStopDistance` if:
    /// - Distance is <= 0
    /// - Distance percentage is > 10%
    /// - Distance percentage is < 0.1%
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.distance <= Decimal::ZERO {
            return Err(DomainError::InvalidTechnicalStopDistance(
                "Distance must be positive".to_string(),
            ));
        }

        if self.distance_pct > Decimal::from(10) {
            return Err(DomainError::InvalidTechnicalStopDistance(
                "Stop too wide (>10%)".to_string(),
            ));
        }

        if self.distance_pct < Decimal::new(1, 1) {
            // 0.1%
            return Err(DomainError::InvalidTechnicalStopDistance(
                "Stop too tight (<0.1%)".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the span (palmo) — alias for `distance`
    ///
    /// The span is the unit of risk, movement, and decision for this position.
    pub fn span(&self) -> Decimal {
        self.distance
    }

    /// Check if current price triggers the trailing stop (LONG position)
    ///
    /// Returns true if price <= trailing_stop
    pub fn should_exit_long(&self, current_price: Decimal, trailing_stop: Decimal) -> bool {
        current_price <= trailing_stop
    }

    /// Check if current price triggers the trailing stop (SHORT position)
    ///
    /// Returns true if price >= trailing_stop
    pub fn should_exit_short(&self, current_price: Decimal, trailing_stop: Decimal) -> bool {
        current_price >= trailing_stop
    }
}

// Make Price constructable internally (for trailing stop calculations)
impl From<Decimal> for Price {
    fn from(value: Decimal) -> Self {
        Self(value)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use rust_decimal_macros::dec;

    use super::*;

    // Price tests
    #[test]
    fn test_price_validation() {
        assert!(Price::new(dec!(100.0)).is_ok());
        assert!(Price::new(dec!(0.01)).is_ok());
        assert!(Price::new(dec!(-1.0)).is_err());
        assert!(Price::new(dec!(0.0)).is_err());
    }

    #[test]
    fn test_price_as_decimal() {
        let price = Price::new(dec!(12345.67)).unwrap();
        assert_eq!(price.as_decimal(), dec!(12345.67));
    }

    // Quantity tests
    #[test]
    fn test_quantity_validation() {
        assert!(Quantity::new(dec!(0.001)).is_ok());
        assert!(Quantity::new(dec!(100.0)).is_ok());
        assert!(Quantity::new(dec!(-0.1)).is_err());
        assert!(Quantity::new(dec!(0.0)).is_err());
    }

    // Symbol tests
    #[test]
    fn test_symbol_from_pair() {
        let symbol = Symbol::from_pair("BTCUSDT").unwrap();
        assert_eq!(symbol.base(), "BTC");
        assert_eq!(symbol.quote(), "USDT");
        assert_eq!(symbol.as_pair(), "BTCUSDT");
    }

    #[test]
    fn test_symbol_from_pair_eth() {
        let symbol = Symbol::from_pair("ETHBUSD").unwrap();
        assert_eq!(symbol.base(), "ETH");
        assert_eq!(symbol.quote(), "BUSD");
    }

    #[test]
    fn test_symbol_invalid() {
        assert!(Symbol::from_pair("INVALID").is_err());
        assert!(Symbol::from_pair("").is_err());
    }

    // Side tests
    #[test]
    fn test_side_actions() {
        assert_eq!(Side::Long.entry_action(), OrderSide::Buy);
        assert_eq!(Side::Long.exit_action(), OrderSide::Sell);
        assert_eq!(Side::Short.entry_action(), OrderSide::Sell);
        assert_eq!(Side::Short.exit_action(), OrderSide::Buy);
    }

    // RiskConfig tests (v3: fixed 1% risk)
    #[test]
    fn test_risk_config_validation() {
        // Valid: positive capital
        assert!(RiskConfig::new(dec!(10000)).is_ok());
        assert!(RiskConfig::new(dec!(1000)).is_ok());
        assert!(RiskConfig::new(dec!(100000)).is_ok());

        // Invalid: zero capital
        assert!(RiskConfig::new(dec!(0)).is_err());

        // Invalid: negative capital
        assert!(RiskConfig::new(dec!(-1000)).is_err());
    }

    #[test]
    fn test_risk_config_fixed_one_percent() {
        // Risk is always 1%, regardless of capital
        let config = RiskConfig::new(dec!(10000)).unwrap();
        assert_eq!(config.risk_per_trade_pct(), dec!(1));
        assert_eq!(config.max_risk_amount(), dec!(100)); // 1% of 10000

        let config2 = RiskConfig::new(dec!(50000)).unwrap();
        assert_eq!(config2.risk_per_trade_pct(), dec!(1)); // still 1%
        assert_eq!(config2.max_risk_amount(), dec!(500)); // 1% of 50000
    }

    #[test]
    fn test_risk_config_leverage() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        assert_eq!(config.leverage(), 10);
        assert_eq!(RiskConfig::LEVERAGE, 10);
    }

    #[test]
    fn test_risk_config_capital() {
        let config = RiskConfig::new(dec!(10000)).unwrap();
        assert_eq!(config.capital(), dec!(10000));
        assert_eq!(config.risk_per_trade_pct(), dec!(1));
    }

    // TechnicalStopDistance tests
    #[test]
    fn test_tech_stop_distance_calculation() {
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        assert_eq!(tech_stop.distance, dec!(1500.0));
        // 1500 / 95000 * 100 = 1.578947368421052631578947368%
        // Note: precision may vary slightly, so we check it's close
        assert!(tech_stop.distance_pct > dec!(1.578) && tech_stop.distance_pct < dec!(1.579));
    }

    #[test]
    fn test_tech_stop_distance_validation() {
        // Valid tech stop (1.58%)
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        assert!(tech_stop.validate().is_ok());
    }

    #[test]
    fn test_tech_stop_distance_too_wide() {
        // Too wide (>10%)
        let entry = Price::new(dec!(100.0)).unwrap();
        let stop = Price::new(dec!(80.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        assert!(tech_stop.validate().is_err());
    }

    #[test]
    fn test_tech_stop_distance_too_tight() {
        // Too tight (<0.1%)
        let entry = Price::new(dec!(100000.0)).unwrap();
        let stop = Price::new(dec!(99990.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        assert!(tech_stop.validate().is_err());
    }

    // Span alias test
    #[test]
    fn test_span_alias() {
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);
        assert_eq!(tech_stop.span(), dec!(1500.0));
        assert_eq!(tech_stop.span(), tech_stop.distance);
    }

    // Exit condition tests
    #[test]
    fn test_should_exit_long() {
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(93500.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let trailing_stop = dec!(94500.0);

        // Price above stop - should NOT exit
        assert!(!tech_stop.should_exit_long(dec!(94600.0), trailing_stop));
        assert!(!tech_stop.should_exit_long(dec!(95000.0), trailing_stop));

        // Price at stop - should exit
        assert!(tech_stop.should_exit_long(dec!(94500.0), trailing_stop));

        // Price below stop - should exit
        assert!(tech_stop.should_exit_long(dec!(94400.0), trailing_stop));
    }

    #[test]
    fn test_should_exit_short() {
        let entry = Price::new(dec!(95000.0)).unwrap();
        let stop = Price::new(dec!(96500.0)).unwrap();
        let tech_stop = TechnicalStopDistance::from_entry_and_stop(entry, stop);

        let trailing_stop = dec!(95500.0);

        // Price below stop - should NOT exit
        assert!(!tech_stop.should_exit_short(dec!(95400.0), trailing_stop));
        assert!(!tech_stop.should_exit_short(dec!(95000.0), trailing_stop));

        // Price at stop - should exit
        assert!(tech_stop.should_exit_short(dec!(95500.0), trailing_stop));

        // Price above stop - should exit
        assert!(tech_stop.should_exit_short(dec!(95600.0), trailing_stop));
    }

    // Tests for new_validated (hard-stop invariants)
    #[test]
    fn test_new_validated_long_rejects_stop_at_entry() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(95000)).unwrap(); // Same as entry

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Long);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DomainError::InvalidTechnicalStopDistance(_)));
    }

    #[test]
    fn test_new_validated_long_rejects_stop_above_entry() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(96000)).unwrap(); // Above entry (wrong for long)

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Long);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_validated_short_rejects_stop_at_entry() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(95000)).unwrap(); // Same as entry

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Short);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_validated_short_rejects_stop_below_entry() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(94000)).unwrap(); // Below entry (wrong for short)

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Short);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_validated_long_accepts_valid_setup() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(93500)).unwrap(); // Below entry (correct for long)

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Long);
        assert!(result.is_ok());
        let tech_stop = result.unwrap();
        assert_eq!(tech_stop.distance, dec!(1500));
    }

    #[test]
    fn test_new_validated_short_accepts_valid_setup() {
        let entry = Price::new(dec!(95000)).unwrap();
        let stop = Price::new(dec!(96500)).unwrap(); // Above entry (correct for short)

        let result = TechnicalStopDistance::new_validated(entry, stop, Side::Short);
        assert!(result.is_ok());
        let tech_stop = result.unwrap();
        assert_eq!(tech_stop.distance, dec!(1500));
    }
}
