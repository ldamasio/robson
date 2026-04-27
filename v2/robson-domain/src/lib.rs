//! Robson v2 Domain Layer
//!
//! Pure domain logic with zero I/O dependencies.
//! Contains entities, value objects, domain events, and domain rules.

#![warn(missing_docs)]
#![warn(clippy::all)]

// Public modules
pub mod context;
pub mod credentials;
pub mod detected_position;
pub mod entities;
pub mod events;
pub mod market_data;
pub mod policy;
pub mod value_objects;

// Re-export commonly used types
pub use context::IdentityScope;
pub use credentials::{
    ApiCredentials, CredentialError, CredentialId, CredentialProfile, CredentialStatus, Exchange,
    StoredCredential,
};
pub use detected_position::{CalculatedStop, DetectedPosition, StopMethod};
pub use entities::{
    calculate_margin_required, calculate_notional_value, calculate_position_size, AccountId,
    DetectorSignal, EntryLifecycleStage, ExitReason, Order, OrderId, OrderStatus, OrderType,
    Position, PositionId, PositionState, TechnicalStopAnalysisAudit,
    TechnicalStopConfidenceSnapshot, TechnicalStopConfigSnapshot, TechnicalStopMethodSnapshot,
};
pub use events::{entry_lifecycle_stage, Event};
pub use market_data::{Candle, MarketDataEvent, OrderBookSnapshot, Tick};
pub use policy::{
    ApprovalPolicy, EntryPolicy, EntryPolicyConfig, SignalEvaluationOutcome, StrategyId,
    TechStopConfig, TradingPolicy,
};
pub use value_objects::{
    DomainError, OrderSide, Price, Quantity, RiskConfig, Side, Symbol, TechnicalStopDistance,
};
