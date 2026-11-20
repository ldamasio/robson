//! Raw observations for mathematics, never currentness or admission
//! certificates.
use chrono::{DateTime, NaiveDate, Utc};
use robson_domain::{ExecutableStopPlan, RiskConfig};
use rust_decimal::Decimal;
use uuid::Uuid;

use crate::{
    balance_costs::{CostIdentity, CostPolicy, CostTerms},
    ports::CandleInterval,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBinding {
    pub account: CostIdentity,
    pub preparation_id: Uuid,
    pub accounting_version: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationRef {
    pub binding: PreparedBinding,
    pub observation_id: Uuid,
    pub source_generation: u64,
    pub original_as_of: DateTime<Utc>,
}
#[derive(Debug, Clone)]
pub struct BasisInput {
    pub amount: Decimal,
    pub month: NaiveDate,
    pub observation: ObservationRef,
}
#[derive(Debug, Clone)]
pub struct CashInput {
    pub free: Decimal,
    pub locked: Decimal,
    pub quote_asset: String,
    pub observation: ObservationRef,
}
#[derive(Debug, Clone)]
pub struct PriceInput {
    pub value: Decimal,
    pub observation: ObservationRef,
}
#[derive(Debug, Clone)]
pub struct ChartInput {
    /// Actual analyzer second support on the15m chart, never a percentage stop.
    pub support: Decimal,
    pub interval: CandleInterval,
    pub guard: Option<Decimal>,
    pub plan: ExecutableStopPlan,
    pub observation: ObservationRef,
}
#[derive(Debug, Clone, Copy)]
pub struct PriceLimits {
    pub min: Decimal,
    pub max: Decimal,
    pub tick: Decimal,
}
#[derive(Debug, Clone, Copy)]
pub struct LotLimits {
    pub min: Decimal,
    pub max: Decimal,
    pub step: Decimal,
}
#[derive(Debug, Clone, Copy)]
pub struct NotionalLimits {
    pub min: Decimal,
    pub max: Decimal,
    pub apply_min: bool,
    pub apply_max: bool,
    pub average_minutes: u32,
}
#[derive(Debug, Clone)]
pub struct SizingRules {
    pub price: PriceLimits,
    pub lot: LotLimits,
    pub market_lot: Option<LotLimits>,
    pub notional: NotionalLimits,
    /// Raw symbol availability, not account permission or freshness.
    pub trading: bool,
    pub market: bool,
    pub stop_loss: bool,
    pub observation: ObservationRef,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPriceSource {
    VenueReference,
    Average { minutes: u32 },
    LastPrice,
}
#[derive(Debug, Clone)]
pub struct FilterPriceInput {
    pub value: Decimal,
    pub source: FilterPriceSource,
    pub observation: ObservationRef,
    pub rules_generation: u64,
}
/// Public raw data only. Matching references do not establish
/// truth/currentness.
#[derive(Debug, Clone)]
pub struct SizingInputs {
    pub binding: PreparedBinding,
    pub basis: BasisInput,
    pub cash: CashInput,
    pub price: PriceInput,
    pub chart: ChartInput,
    pub rules: SizingRules,
    pub filter_price: Option<FilterPriceInput>,
    pub terms: CostTerms,
    pub policy: CostPolicy,
    pub risk: RiskConfig,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateRefusal {
    Quantity,
    EntryMinimum,
    ProtectionMinimum,
    PhysicalCap,
    OperatorCap,
    ChargeCap,
    Filter,
    Unrepresentable,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SizingError {
    #[error("inconsistent observation identity")]
    Identity,
    #[error("invalid sizing configuration")]
    Config,
    #[error("invalid or missing full sizing rules")]
    Rules,
    #[error("invalid canonical chart geometry")]
    Geometry,
    #[error("missing or inconsistent filter valuation")]
    FilterPrice,
    #[error("required exact result is not representable")]
    Unrepresentable,
    #[error("sizing arithmetic proof invariant failed")]
    TechnicalWorkLimit,
    #[error("candidate refused: {reason:?} after {attempts} attempts")]
    Exhausted {
        reason: CandidateRefusal,
        attempts: u8,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizedAmounts {
    pub protection: Decimal,
    pub gross_entry: Decimal,
    pub adverse_fill: Decimal,
    pub loss_per_unit: Decimal,
    pub remainder: Decimal,
    pub charge: Decimal,
    pub gross_notional: Decimal,
}
/// Immutable, unqualified data; no admission, send or current-age capability.
/// ```compile_fail
/// use robson_exec::balance_sizing::SizingCalculation;
/// fn overwrite_attempts(mut result: SizingCalculation) {
///     result.attempts = 1;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SizingCalculation {
    pub(super) selected_lot_step: Decimal,
    pub(super) amounts: SizedAmounts,
    pub(super) inputs: SizingInputs,
    pub(super) attempts: u8,
}
impl SizingCalculation {
    /// Effective nested lot step actually used by the calculation.
    /// ```compile_fail
    /// use robson_exec::balance_sizing::SizingCalculation;
    /// fn overwrite_step(mut result: SizingCalculation) {
    ///     result.selected_lot_step = Default::default();
    /// }
    /// ```
    pub fn selected_lot_step(&self) -> Decimal {
        self.selected_lot_step
    }
    pub fn amounts(&self) -> &SizedAmounts {
        &self.amounts
    }
    pub fn inputs(&self) -> &SizingInputs {
        &self.inputs
    }
    pub fn attempts(&self) -> u8 {
        self.attempts
    }
}
