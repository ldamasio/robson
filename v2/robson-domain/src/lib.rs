//! Robson v2 Domain Layer
//!
//! Pure domain logic with zero I/O dependencies.
//! Contains entities, value objects, and domain rules.

#![warn(missing_docs)]
#![warn(clippy::all)]

// Public modules
pub mod entities;
pub mod value_objects;

// Re-export commonly used types
pub use entities::{
    AccountId, DetectorSignal, ExitReason, Order, OrderId, OrderStatus, OrderType,
    Position, PositionId, PositionState,
};
pub use value_objects::{
    DomainError, Leverage, OrderSide, Price, Quantity, Side, Symbol, TechnicalStopDistance,
};
