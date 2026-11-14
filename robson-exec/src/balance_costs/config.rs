//! Injected text only: no process environment reads or legacy fee fallback.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::Deserialize;
use uuid::Uuid;

use super::{exact::ExactDecimal, terms::CostCandidate, CostError, CostIdentity};
use crate::venue::VenueId;

pub const MAX_OVERRIDE_BYTES: usize = 4096;

/// Policy text validated before any conversion could round a digit. Values are
/// stored in their declared units; sizing and environment wiring are later
/// work.
#[derive(Debug, Clone)]
pub struct CostPolicy {
    gap: ExactDecimal,
    margin_bps: Decimal,
    max_entry_notional: Option<Decimal>,
}
impl CostPolicy {
    pub fn parse(
        gap_bps: &str,
        margin_bps: Option<&str>,
        max_entry_notional: Option<&str>,
    ) -> Result<Self, CostError> {
        let parse = |text: &str| text.parse::<ExactDecimal>().map_err(|_| CostError::Config);
        let gap = parse(gap_bps)?;
        let margin = parse(margin_bps.unwrap_or("200"))?.value();
        let cap = max_entry_notional.map(parse).transpose()?.map(ExactDecimal::value);
        if gap.value() < Decimal::ZERO
            || gap.value() > Decimal::from(100)
            || margin < Decimal::ZERO
            || margin >= Decimal::from(10_000)
            || cap.is_some_and(|value| value <= Decimal::ZERO)
        {
            return Err(CostError::Config);
        }
        Ok(Self {
            gap,
            margin_bps: margin,
            max_entry_notional: cap,
        })
    }
    pub fn gap(&self) -> ExactDecimal {
        self.gap
    }
    pub fn margin_bps(&self) -> Decimal {
        self.margin_bps
    }
    pub fn max_entry_notional(&self) -> Option<Decimal> {
        self.max_entry_notional
    }
}

/// Version1 requires explicit account identity, bps strings, original date and
/// note. Date validity/freshness is NOT inferred here: the sealed receipt-age
/// authority must qualify the original date at selection. Unknown JSON fields,
/// duplicate fields, numeric rates and oversize bodies are configuration
/// errors. The supplied identity already contains the configured canonical
/// symbol; this parser checks its explicit fields and never guesses quote
/// suffixes or determines whether an unknown venue symbol is tradable.
pub fn parse_dated_override(
    body: &str,
    configured: &CostIdentity,
) -> Result<CostCandidate, CostError> {
    if body.len() > MAX_OVERRIDE_BYTES {
        return Err(CostError::Config);
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Raw {
        version: u8,
        tenant_id: Uuid,
        venue: String,
        scope: String,
        uid: u64,
        symbol: String,
        maker_bps: ExactDecimal,
        taker_bps: ExactDecimal,
        as_of: DateTime<Utc>,
        note: String,
    }
    let raw: Raw = serde_json::from_str(body).map_err(|_| CostError::Config)?;
    if raw.version != 1 || configured.venue().as_str() != VenueId::BINANCE_SPOT {
        return Err(CostError::Config);
    }
    if raw.tenant_id != configured.tenant()
        || raw.venue != configured.venue().as_str()
        || raw.scope != configured.scope()
        || raw.uid != configured.uid()
        || raw.symbol != configured.symbol().as_pair()
    {
        return Err(CostError::Identity);
    }
    CostCandidate::dated(
        configured.clone(),
        raw.as_of,
        raw.maker_bps.value(),
        raw.taker_bps.value(),
        raw.note,
    )
}
