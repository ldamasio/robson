//! Entry-fee finalization (EP-2): one inert writer with no runtime caller yet.
//! The accounting row is the first write/lock, as for orders and trades.
use std::sync::atomic::{AtomicBool, Ordering};

use robson_exec::{
    venue::{OrderPhase, VenueId},
    venue_ledger::{
        plan_entry_fee, EntryFeeInput, EntryFeePlan, EntryFillTotals, EntryOrderFacts, LedgerError,
        LegPartition,
    },
};
use rust_decimal::Decimal;
use sqlx::{postgres::PgRow, Connection, PgConnection, Postgres, Row, Transaction};
use tracing::Instrument as _;
use uuid::Uuid;

use super::{
    backend, commit_error, orders::order_error, trades::TRADE_GENERATION, LedgerConnection,
    PgVenueLedger,
};

/// The leg, its current admission's lot step, its entry order and that
/// order's fill totals in ONE statement. A sum is decoded only when
/// `ledger_amount_fits` proves it exact: SQLx's NUMERIC decode may round.
const ENTRY_FEE_CONTEXT: &str = "SELECT l.tenant_id,l.venue_id,l.account_scope,l.symbol,
    l.gross_qty,l.acquisition_cost,l.base_commission,l.base_commission_final,l.executable_qty,
    l.consumed_cum,l.closed_at IS NOT NULL AS leg_closed,a.lot_step,
    o.phase AS entry_phase,o.executed_cum AS entry_executed,o.cumulative_quote AS entry_quote,
    o.fills_qty AS entry_fills,s.fills,s.non_base,
    s.qty_exact,CASE WHEN s.qty_exact THEN s.qty END AS fill_qty,
    s.quote_exact,CASE WHEN s.quote_exact THEN s.quote END AS fill_quote,
    s.fee_exact,CASE WHEN s.fee_exact THEN s.fee END AS fill_fee
    FROM balance_legs l
    LEFT JOIN balance_leg_admissions a ON a.entry_intent_id=l.admission_intent_id
        AND a.position_id=l.position_id AND a.superseded_at IS NULL
    LEFT JOIN venue_orders o ON o.position_id=l.position_id AND o.kind='entry'
        AND o.intent_id=l.admission_intent_id
    LEFT JOIN LATERAL (SELECT t.*,ledger_amount_fits(t.qty) AS qty_exact,
            ledger_amount_fits(t.quote) AS quote_exact,ledger_amount_fits(t.fee) AS fee_exact
        FROM (SELECT count(*) AS fills,count(*) FILTER (WHERE f.commission_asset<>$2) AS non_base,
                COALESCE(sum(f.qty),0) AS qty,COALESCE(sum(f.quote_qty),0) AS quote,
                COALESCE(sum(f.commission) FILTER (WHERE f.commission_asset=$2),0) AS fee
            FROM order_fills f WHERE f.venue_id=o.venue_id AND f.account_scope=o.account_scope
                AND f.symbol=o.symbol AND f.exchange_order_id=o.exchange_order_id) t) s ON true
    WHERE l.position_id=$1";

struct EntryFeeContext {
    venue: String,
    account_scope: String,
    symbol: String,
    input: EntryFeeInput,
}

impl PgVenueLedger {
    /// Make the leg's entry commission B final once its terminal entry's fill
    /// set is complete and corroborates Q and C. No runtime caller yet. Five
    /// statements when nothing changes (BEGIN, the bounds, the certificate's
    /// key, one read, ROLLBACK); a finalization is eight statements, nine when
    /// opted.
    /// `base_asset` comes from the Spot rules: the ledger stores none.
    /// Returns the plan acted on; `Finalize` was committed.
    pub async fn finalize_entry_fee(
        &self,
        position_id: Uuid,
        base_asset: &str,
    ) -> Result<EntryFeePlan, LedgerError> {
        let committing = AtomicBool::new(false);
        self.bounded(async {
            let generation = match (&self.trade_projection, &self.order_projection) {
                (Some(trade), Some(order)) if trade != order => {
                    return Err(LedgerError::Invariant("entry fee projections disagree".into()));
                },
                (Some(generation), _) | (None, Some(generation)) => Some(generation),
                (None, None) => None,
            };
            if base_asset.is_empty() {
                return Err(LedgerError::InvalidEvidence("empty base asset".into()));
            }
            let connection = self.pool.acquire().await.map_err(backend)?;
            let mut held = LedgerConnection::new(connection);
            // The pre-read runs inside the bounded transaction, as main's
            // order and trade pre-reads do (d8e002e1): never a bare SELECT
            // before the ledger's timeouts.
            self.statement();
            let mut tx = held.connection().begin().await.map_err(backend)?;
            self.timeouts(&mut tx).await?;
            // The certificate's shared key right after the bounds, before any read
            // or row lock (8m PLAN-r4 §6.7).
            self.statement();
            let _key = robson_eventlog::certificate::participate(&mut tx).await.map_err(backend)?;
            let before = self.read_entry_fee(&mut tx, position_id, base_asset).await?;
            // Consistency only: the Spot rules are the authority on the asset.
            if !before.symbol.starts_with(base_asset) {
                return Err(LedgerError::InvalidEvidence(
                    "base asset does not prefix the leg symbol".into(),
                ));
            }
            if let Some(generation) = generation {
                if !generation.is_valid()
                    || generation.tenant_id != self.tenant_id
                    || before.venue != VenueId::BINANCE_SPOT
                    || generation.account_scope != before.account_scope
                {
                    return Err(LedgerError::Invariant("entry fee projection scope mismatch".into()));
                }
            }
            let proposed = plan_entry_fee(&before.input)?;
            if !matches!(proposed, EntryFeePlan::Finalize { .. }) {
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished = true;
                return Ok(proposed);
            }
            let venue = before.venue;
            self.statement();
            let version = if generation.is_some() {
                let version = sqlx::query_scalar::<_, i64>("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2 RETURNING version")
                    .bind(self.tenant_id).bind(&venue).fetch_optional(&mut *tx).await.map_err(backend)?;
                Some(version.ok_or(LedgerError::NoAccountingRow)?)
            } else {
                let bumped = sqlx::query("UPDATE accounting_state SET version=version+1 WHERE tenant_id=$1 AND venue_id=$2")
                    .bind(self.tenant_id).bind(&venue).execute(&mut *tx).await.map_err(backend)?;
                if bumped.rows_affected() != 1 {
                    return Err(LedgerError::NoAccountingRow);
                }
                None
            };
            let locked = self.read_entry_fee(&mut tx, position_id, base_asset).await?;
            let planned = plan_entry_fee(&locked.input)?;
            let EntryFeePlan::Finalize { base_fee, executable, .. } = planned else {
                // Another finalizer won, or the leg moved: nothing to write.
                self.statement();
                tx.rollback().await.map_err(backend)?;
                held.finished = true;
                return Ok(planned);
            };
            let partition = locked
                .input
                .partition
                .ok_or_else(|| LedgerError::Invariant("finalized leg has no partition".into()))?;
            self.write_entry_fee(&mut tx, position_id, &venue, &partition, base_fee, executable)
                .await?;
            if let (Some(generation), Some(version)) = (generation, version) {
                // Finalization touches no SELL/CANCEL membership input: one
                // exact version CAS, as for a trade page. A refusal returns
                // unfinished, so the guard rolls back every native write.
                self.statement();
                let advanced = sqlx::query_scalar::<_, i64>(TRADE_GENERATION)
                    .bind(generation.tenant_id).bind(&generation.account_scope).bind(generation.generation_id)
                    .bind(generation.process_epoch).bind(generation.source_audit_id).bind(version)
                    .fetch_optional(&mut *tx)
                    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                    .await.map_err(backend)?;
                if advanced != Some(version) {
                    return Err(LedgerError::Invariant("entry fee projection generation refused".into()));
                }
            }
            self.statement();
            committing.store(true, Ordering::Relaxed);
            tx.commit().await.map_err(commit_error)?;
            held.finished = true;
            Ok(planned)
        }, &committing)
        .await
    }

    /// The one finalization statement, a compare-and-set on the partition the
    /// plan read: Q and C are never set; a final B or a changed X refuses.
    pub(super) async fn write_entry_fee(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        position_id: Uuid,
        venue: &str,
        prior: &LegPartition,
        base_fee: Decimal,
        executable: Decimal,
    ) -> Result<(), LedgerError> {
        self.statement();
        let updated = sqlx::query(
            "UPDATE balance_legs SET base_commission=$1,base_commission_final=true,executable_qty=$2
            WHERE position_id=$3 AND tenant_id=$4 AND venue_id=$5 AND gross_qty=$6 AND acquisition_cost=$7
                AND executable_qty=$8 AND NOT base_commission_final AND closed_at IS NULL",
        )
        .bind(base_fee)
        .bind(executable)
        .bind(position_id)
        .bind(self.tenant_id)
        .bind(venue)
        .bind(prior.gross)
        .bind(prior.cost)
        .bind(prior.executable)
        .execute(&mut **tx)
        .await
        .map_err(order_error)?;
        if updated.rows_affected() != 1 {
            return Err(LedgerError::Invariant("entry fee compare-and-set refused".into()));
        }
        Ok(())
    }

    async fn read_entry_fee(
        &self,
        connection: &mut PgConnection,
        position_id: Uuid,
        base_asset: &str,
    ) -> Result<EntryFeeContext, LedgerError> {
        self.statement();
        let row = sqlx::query(ENTRY_FEE_CONTEXT)
            .bind(position_id)
            .bind(base_asset)
            .fetch_optional(connection)
            .await
            .map_err(backend)?
            .ok_or_else(|| LedgerError::Invariant("entry fee for an unknown leg".into()))?;
        decode_context(&row, self.tenant_id)
    }
}

fn decode_context(row: &PgRow, tenant: Uuid) -> Result<EntryFeeContext, LedgerError> {
    if row.try_get::<Uuid, _>("tenant_id").map_err(backend)? != tenant {
        return Err(LedgerError::IdentityConflict);
    }
    let amount = |column: &str| row.try_get::<Option<Decimal>, _>(column).map_err(backend);
    let partition = match (
        amount("gross_qty")?,
        amount("acquisition_cost")?,
        amount("base_commission")?,
        amount("executable_qty")?,
    ) {
        (Some(gross), Some(cost), Some(base_fee), Some(executable)) => Some(LegPartition {
            gross,
            cost,
            base_fee,
            base_fee_final: row.try_get("base_commission_final").map_err(backend)?,
            executable,
        }),
        (None, None, None, None) => None,
        _ => return Err(LedgerError::Invariant("partial leg partition".into())),
    };
    let lot_step = amount("lot_step")?.ok_or_else(|| {
        LedgerError::Invariant("entry fee without current admission terms".into())
    })?;
    let entry = match row.try_get::<Option<&str>, _>("entry_phase").map_err(backend)? {
        None => None,
        Some(phase) => Some(EntryOrderFacts {
            phase: match phase {
                "live" => OrderPhase::Live,
                "terminal" => OrderPhase::Terminal,
                "unknown" => OrderPhase::Unknown,
                _ => return Err(LedgerError::Invariant("invalid stored order phase".into())),
            },
            executed_cum: row.try_get("entry_executed").map_err(backend)?,
            cumulative_quote: row.try_get("entry_quote").map_err(backend)?,
            fills_qty: row.try_get("entry_fills").map_err(backend)?,
        }),
    };
    let totals = EntryFillTotals {
        fills: row.try_get("fills").map_err(backend)?,
        qty: exact_sum(row.try_get("qty_exact").map_err(backend)?, amount("fill_qty")?)?,
        quote: exact_sum(row.try_get("quote_exact").map_err(backend)?, amount("fill_quote")?)?,
        base_commission: exact_sum(
            row.try_get("fee_exact").map_err(backend)?,
            amount("fill_fee")?,
        )?,
        non_base_fills: row.try_get("non_base").map_err(backend)?,
    };
    Ok(EntryFeeContext {
        venue: row.try_get("venue_id").map_err(backend)?,
        account_scope: row.try_get("account_scope").map_err(backend)?,
        symbol: row.try_get("symbol").map_err(backend)?,
        input: EntryFeeInput {
            partition,
            consumed: row.try_get("consumed_cum").map_err(backend)?,
            closed: row.try_get("leg_closed").map_err(backend)?,
            lot_step,
            entry,
            totals,
        },
    })
}

/// A fill sum the database could not prove exact is refused before planning.
fn exact_sum(exact: bool, sum: Option<Decimal>) -> Result<Decimal, LedgerError> {
    match (exact, sum) {
        (true, Some(sum)) => Ok(sum),
        (false, _) => Err(LedgerError::Unrepresentable),
        (true, None) => Err(LedgerError::Invariant("exact fill sum missing".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_proven_exact_fill_sum_is_decoded() {
        assert_eq!(exact_sum(true, Some(Decimal::TEN)), Ok(Decimal::TEN));
        // Each of Σ qty, Σ quote and Σ base commission maps a false guard here.
        assert_eq!(exact_sum(false, None), Err(LedgerError::Unrepresentable));
        assert_eq!(exact_sum(false, Some(Decimal::TEN)), Err(LedgerError::Unrepresentable));
        assert!(matches!(exact_sum(true, None), Err(LedgerError::Invariant(_))));
    }
}
