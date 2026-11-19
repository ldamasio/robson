//! Flagged journal statements, isolated to keep the legacy debug frame small.
use std::time::{Duration, Instant};

use robson_exec::order_journal::{CommitFailure, JournalBatch};
use sqlx::{Connection, Postgres, Transaction};
use tracing::Instrument as _;

use super::PgJournalBackend;
use crate::{ledger_connection::LedgerConnection, spot_order_set_store::JournalGeneration};

fn known(error: sqlx::Error) -> CommitFailure {
    CommitFailure::Known(error.to_string())
}

/// Leave room for COMMIT and account for time already spent
/// acquiring/beginning.
fn statement_budget(bound: Duration, elapsed: Duration) -> Result<Duration, CommitFailure> {
    let remaining = bound.saturating_sub(elapsed).saturating_sub(Duration::from_millis(100));
    let budget = remaining.min(Duration::from_millis(300));
    if budget < Duration::from_millis(20) {
        return Err(CommitFailure::Known("balance journal commit budget exhausted".to_string()));
    }
    Ok(budget)
}

impl PgJournalBackend {
    pub(super) async fn commit_balance(
        &self,
        batch: &JournalBatch,
        started: Instant,
        bound: Duration,
        projection: Option<&JournalGeneration>,
    ) -> Result<(), CommitFailure> {
        let connection = self.pool.acquire().await.map_err(known)?;
        let mut write = LedgerConnection::new(connection);
        // Same operation_io spans as the unflagged commit path, so the
        // operation's database attempts include the balance statements.
        let mut tx = write
            .connection()
            .begin()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64),
            )
            .await
            .map_err(known)?;
        let version = Box::pin(self.prepare_accounting_write(
            &batch.intent.venue_id,
            bound,
            &mut tx,
            started,
        ))
        .await?;
        let prior_member_expected = self.write(batch, &mut tx).await?;
        if let Some(generation) = projection {
            Box::pin(self.maintain_order_set(
                generation,
                batch,
                version,
                prior_member_expected,
                &mut tx,
                started,
                bound,
            ))
            .await?;
        }
        // Preserve J's existing conservative COMMIT classification.
        tx.commit()
            .instrument(
                tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64),
            )
            .await
            .map_err(|e| CommitFailure::Uncertain(e.to_string()))?;
        write.finished = true;
        Ok(())
    }

    pub(super) async fn prepare_accounting_write(
        &self,
        venue_id: &str,
        commit_bound: Duration,
        tx: &mut Transaction<'_, Postgres>,
        started: Instant,
    ) -> Result<i64, CommitFailure> {
        let timeout =
            format!("{}ms", statement_budget(commit_bound, started.elapsed())?.as_millis());
        sqlx::query("SELECT set_config('lock_timeout', $1, true), set_config('statement_timeout', $1, true), set_config('idle_in_transaction_session_timeout', '1s', true)")
            .bind(timeout).execute(&mut **tx)
            .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
            .await.map_err(known)?;
        let version = sqlx::query_scalar::<_, i64>(
            "UPDATE accounting_state SET version = version + 1 WHERE tenant_id = $1 AND venue_id = $2 RETURNING version",
        ).bind(self.tenant_id).bind(venue_id).fetch_optional(&mut **tx)
            .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
            .await.map_err(known)?;
        version.ok_or_else(|| {
            CommitFailure::Known("balance journal has no accounting row".to_string())
        })
    }

    pub(super) async fn write_balance(
        &self,
        batch: &JournalBatch,
        tx: &mut Transaction<'_, Postgres>,
        clock: Option<&super::clocked::Prepared>,
    ) -> Result<Vec<super::clocked::ClockReceipt>, CommitFailure> {
        let Some(balance) = &batch.balance else {
            return Ok(Vec::new());
        };
        let intent = &batch.intent;
        let mut receipts = Vec::new();
        if let Some(clock) = clock {
            // Replaces the five-field insert: inserting a NULL clock first
            // would make the immutable row conflict with the receipt.
            receipts = Box::pin(self.insert_clocks(batch, clock, tx)).await?;
        } else if !batch.attempts.is_empty() {
            let ids: Vec<_> = batch.attempts.iter().map(|attempt| attempt.attempt_id).collect();
            let created: Vec<_> = batch.attempts.iter().map(|attempt| attempt.created_at).collect();
            sqlx::query(
                "INSERT INTO spot_intent_clock (attempt_id, intent_id, tenant_id, venue_id, attempt_created_at) \
                 SELECT id, $2, $3, $4, created_at FROM UNNEST($1::uuid[], $5::timestamptz[]) AS incoming(id, created_at) \
                 ON CONFLICT (attempt_id) DO NOTHING",
            ).bind(ids).bind(intent.intent_id).bind(self.tenant_id).bind(&intent.venue_id)
                .bind(created).execute(&mut **tx)
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                .await.map_err(known)?;
        }
        if let Some(admission) = &balance.admission {
            sqlx::query(
                "SELECT balance_leg_admit($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24)",
            ).bind(self.tenant_id).bind(&intent.venue_id).bind(&intent.account_scope)
                .bind(&admission.symbol).bind(admission.position_id).bind(intent.intent_id)
                .bind(admission.decided_month).bind(admission.taker_bps).bind(admission.gap_bps)
                .bind(admission.notional_margin_bps).bind(admission.min_notional).bind(admission.lot_step)
                .bind(&admission.fee_asset_policy).bind(&admission.envelope_source).bind(admission.envelope_as_of)
                .bind(&admission.envelope_hash).bind(admission.planned_qty).bind(admission.requested_qty)
                .bind(admission.reference_price).bind(admission.remainder_reserve).bind(admission.admission_charge)
                .bind(admission.initial_trigger).bind(intent.created_at).bind(balance.handed_off)
                .execute(&mut **tx)
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                .await.map_err(known)?;
        }
        Ok(receipts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_statement_budget_counts_acquisition_and_reserves_commit_time() {
        let ms = Duration::from_millis;
        assert_eq!(statement_budget(ms(500), ms(0)), Ok(ms(300)));
        assert_eq!(statement_budget(ms(500), ms(240)), Ok(ms(160)));
        assert_eq!(statement_budget(ms(500), ms(380)), Ok(ms(20)));
        assert!(matches!(statement_budget(ms(500), ms(381)), Err(CommitFailure::Known(_))));
        assert!(statement_budget(ms(50), ms(0)).is_err());
        assert!(statement_budget(ms(500), ms(600)).is_err());
    }
}
