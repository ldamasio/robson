//! Pure handoff of exact commission records. No clock, refresh or caller
//! wiring.
use chrono::{DateTime, Utc};
use robson_exec::balance_costs::{CommissionComponents, CostCandidate, CostError, CostIdentity};

use super::records::{SpotCommission, SpotCommissionRates};

impl SpotCommission {
    /// Validated candidate only; a positive persisted receipt from the future
    /// 8h age authority is still required before selecting immutable terms.
    pub fn cost_candidate(
        &self,
        identity: CostIdentity,
        as_of: DateTime<Utc>,
    ) -> Result<CostCandidate, CostError> {
        if self.symbol != identity.symbol().as_pair() {
            return Err(CostError::Identity);
        }
        let rates =
            |rates: &SpotCommissionRates| [rates.maker, rates.taker, rates.buyer, rates.seller];
        CostCandidate::commission(identity, as_of, CommissionComponents {
            standard: rates(&self.standard),
            tax: rates(&self.tax),
            special: rates(&self.special),
            enabled_for_account: self.discount.enabled_for_account,
            enabled_for_symbol: self.discount.enabled_for_symbol,
            discount: self.discount.discount,
            discount_asset: self.discount.discount_asset.clone(),
        })
    }
}
