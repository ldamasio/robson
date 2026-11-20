//! Pure entry-fee finalization (EP-2). B becomes final once the terminal
//! entry's fill set is complete and corroborates the recorded Q and C. No
//! database or venue call happens here.
use rust_decimal::Decimal;

use super::{LedgerError, LegPartition};
use crate::{
    balance_liability::{final_fee_quantity, FactError, FinalFeeInput},
    venue::OrderPhase,
};

/// The leg's entry order, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryOrderFacts {
    pub phase: OrderPhase,
    pub executed_cum: Decimal,
    pub cumulative_quote: Option<Decimal>,
    pub fills_qty: Decimal,
}

/// Exact totals over the entry order's fills. Only commissions charged in the
/// base asset are summed; fills charged in any other asset are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFillTotals {
    pub fills: i64,
    pub qty: Decimal,
    pub quote: Decimal,
    pub base_commission: Decimal,
    pub non_base_fills: i64,
}

/// One read: the leg, its current admission's lot step, its entry order and
/// that order's fill totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryFeeInput {
    pub partition: Option<LegPartition>,
    pub consumed: Decimal,
    pub closed: bool,
    pub lot_step: Decimal,
    pub entry: Option<EntryOrderFacts>,
    pub totals: EntryFillTotals,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFeePending {
    /// EP-1 recorded no partition: an unfilled or non-terminal entry.
    NoPartition,
    EntryNotTerminal,
    /// Σ fill qty < Q: the settlement queue keeps paging.
    FillsIncomplete,
}

/// The settlement barrier holds and nothing is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFeeHold {
    QuoteMismatch {
        fills: Decimal,
        cost: Decimal,
    },
    /// A commission charged in another asset is never counted as zero.
    NonBaseCommission {
        fills: i64,
    },
    FeeExceedsGross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryFeePlan {
    NotReady(EntryFeePending),
    Held(EntryFeeHold),
    /// B is already final with these same values.
    Unchanged,
    /// The one write: final B and X′ = min(X, floor_step(Q − B)). A deficit
    /// (consumed > X′) is reported, never hidden.
    Finalize {
        base_fee: Decimal,
        executable: Decimal,
        reduced: bool,
        deficit: bool,
    },
}

/// Decide the entry fee from one read. Equality checks plus the accepted
/// final-fee boundary; no other arithmetic.
pub fn plan_entry_fee(input: &EntryFeeInput) -> Result<EntryFeePlan, LedgerError> {
    let Some(partition) = input.partition else {
        return Ok(EntryFeePlan::NotReady(EntryFeePending::NoPartition));
    };
    let Some(entry) = input.entry.filter(|entry| entry.phase == OrderPhase::Terminal) else {
        return Ok(EntryFeePlan::NotReady(EntryFeePending::EntryNotTerminal));
    };
    if input.closed {
        return Err(LedgerError::Invariant("entry fee for a closed leg".into()));
    }
    if entry.executed_cum != partition.gross || entry.cumulative_quote != Some(partition.cost) {
        return Err(LedgerError::IdentityConflict);
    }
    let totals = input.totals;
    if entry.fills_qty != totals.qty {
        return Err(LedgerError::Invariant("entry fills disagree with fills_qty".into()));
    }
    if totals.qty < partition.gross {
        return Ok(EntryFeePlan::NotReady(EntryFeePending::FillsIncomplete));
    }
    if totals.qty > partition.gross {
        return Err(LedgerError::Invariant("entry fills exceed the gross quantity".into()));
    }
    if totals.quote != partition.cost {
        return Ok(EntryFeePlan::Held(EntryFeeHold::QuoteMismatch {
            fills: totals.quote,
            cost: partition.cost,
        }));
    }
    if totals.non_base_fills != 0 {
        return Ok(EntryFeePlan::Held(EntryFeeHold::NonBaseCommission {
            fills: totals.non_base_fills,
        }));
    }
    let base_fee = totals.base_commission;
    if base_fee > partition.gross {
        return Ok(EntryFeePlan::Held(EntryFeeHold::FeeExceedsGross));
    }
    let quantity = final_fee_quantity(FinalFeeInput {
        gross: partition.gross,
        base_fee,
        prior_executable: partition.executable,
        lot_step: input.lot_step,
        consumed: input.consumed,
    })
    .map_err(|error| match error {
        FactError::Unrepresentable => LedgerError::Unrepresentable,
        _ => LedgerError::Invariant("entry fee terms refused".into()),
    })?;
    let executable = quantity.executable();
    if partition.base_fee_final {
        // A final B is frozen: an equal recomputation is a replay, anything
        // else is a conflict and never a rewrite.
        return if partition.base_fee == base_fee && partition.executable == executable {
            Ok(EntryFeePlan::Unchanged)
        } else {
            Err(LedgerError::IdentityConflict)
        };
    }
    Ok(EntryFeePlan::Finalize {
        base_fee,
        executable,
        reduced: executable < partition.executable,
        deficit: input.consumed > executable,
    })
}

#[cfg(test)]
mod tests;
