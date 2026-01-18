//! Robson v2 Domain Layer
//!
//! Pure domain logic with zero I/O dependencies.
//! Contains entities, value objects, domain events, and domain rules.

#![warn(missing_docs)]
#![warn(clippy::all)]

// Public modules
pub mod entities;
pub mod events;
pub mod market_data;
pub mod trailing;
pub mod value_objects;

// Re-export commonly used types
pub use entities::{
    AccountId, DetectorSignal, ExitReason, Order, OrderId, OrderStatus, OrderType, Position,
    PositionId, PositionState, calculate_margin_required, calculate_notional_value,
    calculate_position_size,
};
pub use events::Event;
pub use market_data::{Candle, MarketDataEvent, OrderBookSnapshot, Tick};
pub use trailing::{TrailingStopUpdate, is_trailing_stop_hit, update_trailing_stop_anchored};
pub use value_objects::{
    DomainError, OrderSide, Price, Quantity, RiskConfig, Side, Symbol, TechnicalStopDistance,
};
