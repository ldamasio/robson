//! Atomic storage for a Spot transition and its first original-order cancel.
//! The return value describes the acknowledged database write only.
use std::time::Instant;

use robson_exec::{
    balance_liability::TransitionKind,
    order_journal::{IntentKind, IntentRow, COMMIT_BOUND},
    venue::VenueId,
    venue_ledger::VenueIntId,
};
use rust_decimal::Decimal;
use sqlx::Connection as _;
use tokio::time::timeout_at;
use tracing::Instrument as _;
use uuid::Uuid;

use super::{CommitFailure, PgJournalBackend};
use crate::ledger_connection::LedgerConnection;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedTransition {
    New(Uuid),
    ExistingOpen(Uuid),
}

#[derive(Clone, Copy)]
struct TransitionWrite<'a> {
    symbol: &'a str,
    kind: TransitionKind,
    target_trigger: Decimal,
    reason: &'a str,
    expected_accounting_version: i64,
    expected_transition: ExpectedTransition,
}

#[derive(Debug, PartialEq, Eq)]
struct StoredFirstCancel {
    transition_id: Uuid,
    intent_id: Uuid,
    accounting_version: i64,
}

const DEADLINE_BEFORE_COMMIT: &str = "first-cancel deadline expired before COMMIT";

const INSERT_CANCEL: &str = "
    INSERT INTO order_intents
        (intent_id, tenant_id, venue_id, account_scope, client_order_id, position_id,
         kind, cancels_order_id, params, created_at)
    VALUES ($1, $2, $3, $4, $5, $6, 'cancel', $7, $8, $9)
    ON CONFLICT DO NOTHING
    RETURNING intent_id";

const INSERT_TRANSITION: &str = "
    INSERT INTO balance_transitions
        (transition_id, position_id, tenant_id, venue_id, account_scope, symbol, kind,
         target_trigger, reason, first_cancel_intent_id, original_target_order_id, started_at)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
    ON CONFLICT DO NOTHING
    RETURNING transition_id";

const ATTACH_TO_OPEN: &str = "
    UPDATE balance_transitions AS t
    SET (kind, reason, target_trigger, first_cancel_intent_id, original_target_order_id) =
        (SELECT CASE WHEN stronger THEN $7 ELSE t.kind END,
                CASE WHEN stronger THEN $9 ELSE t.reason END,
                greatest(t.target_trigger, $8), $10, $11
           FROM (SELECT ($7 = 'exit' AND t.kind <> 'exit')
                        OR ($7 = 'flatten' AND t.kind IN ('replace', 'reprotect')) AS stronger
                ) AS proposed)
    WHERE t.transition_id=$1 AND t.position_id=$2 AND t.tenant_id=$3 AND t.venue_id=$4
      AND t.account_scope=$5 AND t.symbol=$6 AND t.completed_at IS NULL
      AND t.first_cancel_intent_id IS NULL AND t.original_target_order_id IS NULL
    RETURNING t.transition_id";

/// One statement after the cancel and transition, under the accounting lock.
/// No row is a refusal.
const FIRST_CANCEL_PROJECTION: &str = include_str!("spot_order_set_store/first_cancel.sql");

fn known(error: sqlx::Error) -> CommitFailure {
    CommitFailure::Known(error.to_string())
}

impl PgJournalBackend {
    async fn commit_first_cancel(
        &self,
        intent: &IntentRow,
        request: TransitionWrite<'_>,
    ) -> Result<StoredFirstCancel, CommitFailure> {
        let started = Instant::now();
        let deadline = tokio::time::Instant::now() + COMMIT_BOUND;
        let (transition_id, transition_sql) = match request.expected_transition {
            ExpectedTransition::New(id) => (id, INSERT_TRANSITION),
            ExpectedTransition::ExistingOpen(id) => (id, ATTACH_TO_OPEN),
        };
        let invalid = || CommitFailure::Known("invalid first-cancel transition input".into());
        let position = intent.position_id.filter(|id| !id.is_nil()).ok_or_else(invalid)?;
        let original = intent.cancels_order_id.as_deref().ok_or_else(invalid)?;
        let next_version = request
            .expected_accounting_version
            .checked_add(1)
            .filter(|&version| version > 0)
            .ok_or_else(invalid)?;
        if intent.venue_id != VenueId::BINANCE_SPOT
            || intent.kind != IntentKind::Cancel
            || intent.intent_id.is_nil()
            || transition_id.is_nil()
            || intent.account_scope.trim().is_empty()
            || intent.client_order_id.trim().is_empty()
            || request.symbol.trim().is_empty()
            || request.reason.trim().is_empty()
            || request.target_trigger <= Decimal::ZERO
            || !original.parse::<VenueIntId>().is_ok_and(|id| id.to_string() == original)
        {
            return Err(invalid());
        }
        let kind = match request.kind {
            TransitionKind::Replace => "replace",
            TransitionKind::Reprotect => "reprotect",
            TransitionKind::Flatten => "flatten",
            TransitionKind::Exit => "exit",
        };
        // `None` keeps the legacy statements. An opted binding is checked
        // before any connection is acquired.
        let projection = self.order_set_projection.as_ref();
        if projection.is_some_and(|generation| {
            !generation.is_valid()
                || generation.tenant_id != self.tenant_id
                || generation.account_scope != intent.account_scope
        }) {
            return Err(CommitFailure::Known(
                "order-set first cancel refused: invalid binding".into(),
            ));
        }

        let mut commit_started = false;
        let operation = async {
            let connection = self.pool.acquire().await.map_err(known)?;
            let mut owner = LedgerConnection::new(connection);
            let mut tx = owner.connection().begin()
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 2_u64))
                .await.map_err(known)?;
            let version = self
                .prepare_accounting_write(&intent.venue_id, COMMIT_BOUND, &mut tx, started)
                .await?;
            if version != next_version {
                return Err(CommitFailure::Known("stale first-cancel accounting version".into()));
            }

            let cancel: Option<Uuid> = sqlx::query_scalar(INSERT_CANCEL)
                .bind(intent.intent_id).bind(self.tenant_id).bind(&intent.venue_id)
                .bind(&intent.account_scope).bind(&intent.client_order_id).bind(position)
                .bind(original).bind(&intent.params).bind(intent.created_at)
                .fetch_optional(&mut *tx)
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                .await.map_err(known)?;
            if cancel.is_none() {
                return Err(CommitFailure::IdSpent);
            }

            let transition = sqlx::query_scalar(transition_sql)
                .bind(transition_id)
                .bind(position)
                .bind(self.tenant_id)
                .bind(&intent.venue_id)
                .bind(&intent.account_scope)
                .bind(request.symbol)
                .bind(kind)
                .bind(request.target_trigger)
                .bind(request.reason)
                .bind(intent.intent_id)
                .bind(original);
            let transition = match request.expected_transition {
                ExpectedTransition::New(_) => transition.bind(intent.created_at),
                ExpectedTransition::ExistingOpen(_) => transition,
            };
            let stored: Option<Uuid> = transition.fetch_optional(&mut *tx)
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                .await.map_err(known)?;
            if stored != Some(transition_id) {
                return Err(CommitFailure::Known(
                    "first-cancel transition is no longer available".into(),
                ));
            }
            if let Some(generation) = projection {
                let projected: Option<i64> = sqlx::query_scalar(FIRST_CANCEL_PROJECTION)
                    .bind(generation.tenant_id)
                    .bind(&generation.account_scope)
                    .bind(generation.generation_id)
                    .bind(generation.process_epoch)
                    .bind(generation.source_audit_id)
                    .bind(version)
                    .bind(intent.intent_id)
                    .bind(position)
                    .bind(original)
                    .bind(request.symbol)
                    .fetch_optional(&mut *tx)
                    .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64))
                    .await.map_err(known)?;
                if projected != Some(version) {
                    return Err(CommitFailure::Known(
                        "order-set first cancel refused: stale generation or nonmatching source"
                            .into(),
                    ));
                }
            }
            #[cfg(test)]
            faults::before_commit().await;
            // timeout_at polls the operation before its timer. If SQL and the
            // deadline became ready together, do not enter COMMIT.
            if tokio::time::Instant::now() >= deadline {
                return Err(CommitFailure::Known(DEADLINE_BEFORE_COMMIT.into()));
            }
            commit_started = true;
            tx.commit()
                .instrument(tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 3_u64))
                .await.map_err(|error| CommitFailure::Uncertain(error.to_string()))?;
            if tokio::time::Instant::now() >= deadline {
                return Err(CommitFailure::Uncertain(
                    "first-cancel commit acknowledgement missed its deadline".into(),
                ));
            }
            owner.finished = true;
            Ok(StoredFirstCancel {
                transition_id,
                intent_id: intent.intent_id,
                accounting_version: version,
            })
        };
        match timeout_at(deadline, operation).await {
            Ok(result) => result,
            Err(_) if commit_started => Err(CommitFailure::Uncertain(
                "first-cancel commit did not answer within its bound".into(),
            )),
            Err(_) => Err(CommitFailure::Known(DEADLINE_BEFORE_COMMIT.into())),
        }
    }
}

#[cfg(test)]
#[path = "order_journal_transition_positive.rs"]
mod positive;

#[cfg(test)]
#[path = "order_journal_transition_faults.rs"]
mod faults;

#[cfg(test)]
#[path = "order_journal_transition_prepared.rs"]
mod prepared;

#[cfg(test)]
#[path = "order_journal_transition_projected_pg_tests.rs"]
mod projected;
