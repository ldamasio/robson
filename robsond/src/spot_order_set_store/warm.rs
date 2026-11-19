//! One private structural warm observation of a persisted order set. It is
//! factual storage only: absent aggregates stay absent and nothing here
//! establishes readiness, risk or any order authority.
use rust_decimal::Decimal;
use sqlx::PgConnection;
use tracing::Instrument as _;
use uuid::Uuid;

use super::ExpectedGeneration;

pub(crate) const WARM_SQL: &str = include_str!("warm_order_set.sql");

#[derive(Clone, Copy)]
pub(crate) struct WarmKeys<'a> {
    pub(crate) binding: ExpectedGeneration<'a>,
    pub(crate) position_id: Uuid,
    pub(crate) symbol: &'a str,
    pub(crate) candidate_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(crate) struct WarmObservation {
    pub(crate) generation_id: Uuid,
    pub(crate) process_epoch: Uuid,
    pub(crate) source_audit_id: Uuid,
    pub(crate) accounting_version: i64,
    pub(crate) position_id: Uuid,
    pub(crate) symbol: String,
    pub(crate) gross_qty: Option<Decimal>,
    pub(crate) acquisition_cost: Option<Decimal>,
    pub(crate) base_commission: Option<Decimal>,
    pub(crate) base_commission_final: bool,
    pub(crate) executable_qty: Option<Decimal>,
    pub(crate) consumed_cum: Decimal,
    pub(crate) target_trigger: Decimal,
    pub(crate) leg_sell: Option<i64>,
    pub(crate) leg_sell_conflicts: Option<i64>,
    pub(crate) symbol_sell: Option<i64>,
    pub(crate) symbol_sell_conflicts: Option<i64>,
    pub(crate) unbound_sell: Option<i64>,
    pub(crate) unbound_sell_conflicts: Option<i64>,
    pub(crate) grouped_cancel: Option<i64>,
    pub(crate) grouped_cancel_conflicts: Option<i64>,
    pub(crate) ungroupable_cancel: Option<i64>,
    pub(crate) ungroupable_cancel_conflicts: Option<i64>,
    pub(crate) unbound_cancel: Option<i64>,
    pub(crate) unbound_cancel_conflicts: Option<i64>,
    pub(crate) candidate_scope: Option<String>,
    pub(crate) candidate_position: Option<Uuid>,
    pub(crate) candidate_potential: Option<bool>,
    pub(crate) candidate_conflict: Option<bool>,
    pub(crate) target_cancel_pending: Option<i64>,
    pub(crate) target_cancel_conflicts: Option<i64>,
    pub(crate) target_cancel_terminal: Option<bool>,
    pub(crate) stop_id: Option<String>,
    pub(crate) stop_phase: Option<String>,
    pub(crate) stop_status: Option<String>,
    pub(crate) stop_side: Option<String>,
    pub(crate) stop_type: Option<String>,
    pub(crate) stop_qty: Option<Decimal>,
    pub(crate) stop_executed: Option<Decimal>,
    pub(crate) stop_trigger: Option<Decimal>,
    pub(crate) exact_stop_identity: bool,
}

/// One data statement on the caller's connection; no retry, connect or write.
pub(crate) async fn observe_warm(
    connection: &mut PgConnection,
    keys: WarmKeys<'_>,
) -> Result<Option<WarmObservation>, sqlx::Error> {
    let binding = keys.binding;
    if !binding.is_valid() || keys.position_id.is_nil() || keys.symbol.trim().is_empty() {
        return Ok(None);
    }
    sqlx::query_as::<_, WarmObservation>(WARM_SQL)
        .bind(binding.tenant_id)
        .bind(binding.venue.as_str())
        .bind(binding.account_scope)
        .bind(binding.generation_id)
        .bind(binding.process_epoch)
        .bind(binding.source_audit_id)
        .bind(keys.position_id)
        .bind(keys.symbol)
        .bind(keys.candidate_id)
        .fetch_optional(connection)
        .instrument(
            tracing::info_span!(target: "robson_operation", "operation_io", io_kind = 1_u64),
        )
        .await
}

#[cfg(test)]
#[path = "warm_pg_tests.rs"]
mod pg_tests;
