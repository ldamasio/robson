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
pub mod executable_stop;
pub mod market_data;
pub mod policy;
pub mod stop_policy;
pub mod trading_rules;
pub mod value_objects;

// Re-export commonly used types
pub use context::IdentityScope;
pub use credentials::{
    ApiCredentials, CredentialError, CredentialId, CredentialProfile, CredentialStatus, Exchange,
    StoredCredential,
};
pub use detected_position::{CalculatedStop, DetectedPosition, StopMethod};
pub use entities::{
    calculate_margin_required, calculate_notional_value, size_entry, AccountId,
    AccountSnapshotEvidence, AnchorType, ClosureEvidence, DetectorSignal, EntryLifecycleStage,
    EstimatedEvidence, EstimationBasis, ExitOrderFillSource, ExitReason, Order, OrderFillEvidence,
    OrderId, OrderStatus, OrderType, Position, PositionId, PositionState, RealFillEvidence,
    ReconciliationEvidence, SizedEntry, StopAnchor, StopQuality, StopQualityClassification,
    TechnicalStopAnalysisAudit, TechnicalStopConfidenceSnapshot, TechnicalStopConfigSnapshot,
    TechnicalStopMethodSnapshot, UserTradeEvidence,
};
pub use events::{entry_lifecycle_stage, Event};
pub use executable_stop::{
    build_executable_stop_plan, worst_case_loss_per_unit_planned, ExecutableSpanSource,
    ExecutableStopPlan, StopPlanInputs, SPAN_CAP_RATIO,
};
pub use market_data::{Candle, MarketDataEvent, OrderBookSnapshot, Tick};
pub use policy::{
    ApprovalPolicy, EntryPolicy, EntryPolicyConfig, MonthlyBudgetModel, SignalEvaluationOutcome,
    StrategyId, TechStopConfig, TradingPolicy,
};
pub use stop_policy::StopPolicy;
pub use trading_rules::SymbolTradingRules;
pub use value_objects::{
    DomainError, OrderSide, Price, Quantity, RiskConfig, Side, StopDistanceBounds, Symbol,
    TechnicalStopDistance,
};
