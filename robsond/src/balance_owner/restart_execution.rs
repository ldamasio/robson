//! Typed local restart blocker for one already-known durable linked SELL after
//! an unclean successor epoch. The old process's pending ticket and marks are
//! gone; this owner never reconstructs them. It holds one exact durable
//! candidate blocked and resolves it only when a fresh exact venue record
//! equals durable state that stayed byte-identical across the lookup. It is
//! read-only: no ledger, projection or gate write. No live loop selects it.

use robson_domain::OrderSide;
use robson_exec::{
    venue::{OrderObservation, OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{LedgerError, LegOrderKind, OrderKey, OrderLink, OrderOwner},
};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use tokio::time::Instant;
use uuid::Uuid;

use super::JournalGeneration;
use crate::{
    process_epoch::{EpochStart, Predecessor},
    venue_clock_owner::GovernedRestartRead,
};

/// One statement, one snapshot: the linked order, its journal row, the old
/// installed projection, current accounting, the latest epoch and the epoch
/// directly before it.
const DURABLE_CANDIDATE: &str = "
SELECT o.client_order_id, o.last_client_order_id, o.side, o.order_type, o.status, o.phase,
       o.orig_qty, o.executed_cum, o.cumulative_quote, o.venue_update_time_ms, o.stop_price,
       o.expiry_reason, j.client_order_id AS journal_client, j.kind AS journal_kind,
       j.position_id AS journal_position, j.not_sent, j.venue_outcome, va.uid, g.generation_id, g.source_audit_id, g.process_epoch,
       g.accounting_version AS projected_version, a.version AS accounting_version,
       latest.epoch_id AS latest_epoch, latest.epoch_seq AS latest_seq, latest.clean_stop_at IS NULL AS latest_open,
       prior.epoch_id AS prior_epoch, prior.clean_stop_at IS NULL AS prior_unclean
FROM public.venue_orders o
JOIN public.order_intents j
  ON (j.intent_id,j.tenant_id,j.venue_id,j.account_scope)
   = (o.intent_id,o.tenant_id,o.venue_id,o.account_scope)
JOIN public.spot_order_set_generation g
  ON (g.tenant_id,g.venue_id,g.account_scope) = (o.tenant_id,o.venue_id,o.account_scope)
JOIN public.accounting_state a ON (a.tenant_id,a.venue_id) = (o.tenant_id,o.venue_id)
JOIN public.venue_accounts va
  ON (va.tenant_id,va.venue_id,va.account_scope) = (o.tenant_id,o.venue_id,o.account_scope)
CROSS JOIN LATERAL (
    SELECT epoch_id, epoch_seq, clean_stop_at FROM public.process_epochs
    WHERE tenant_id=$1 ORDER BY epoch_seq DESC LIMIT 1) latest
CROSS JOIN LATERAL (
    SELECT epoch_id, clean_stop_at FROM public.process_epochs
    WHERE tenant_id=$1 AND epoch_seq < latest.epoch_seq ORDER BY epoch_seq DESC LIMIT 1) prior
WHERE o.tenant_id=$1 AND o.venue_id='binance:spot' AND o.account_scope=$2 AND o.symbol=$3
  AND o.exchange_order_id=$4 AND o.owner='leg' AND o.kind='sell'
  AND o.intent_id=$5 AND o.position_id=$6";

/// The exact durable fingerprint; any difference across a lookup refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DurableCandidate {
    client: String,
    last_client: String,
    side: String,
    order_type: String,
    status: String,
    phase: String,
    orig_qty: Decimal,
    executed: Decimal,
    quote: Option<Decimal>,
    update_time_ms: Option<i64>,
    stop_price: Option<Decimal>,
    expiry_reason: Option<String>,
    journal_client: String,
    journal_kind: String,
    journal_position: Option<Uuid>,
    not_sent: bool,
    venue_outcome: Option<String>,
    uid: String,
    generation_id: Uuid,
    source_audit_id: Option<Uuid>,
    projection_epoch: Option<Uuid>,
    projected_version: Option<i64>,
    accounting_version: i64,
    latest_epoch: Uuid,
    latest_seq: i64,
    latest_open: bool,
    prior_epoch: Uuid,
    prior_unclean: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RestartState {
    Blocked,
    Resolved,
}

pub(super) struct RestartExecutionBlocker<'pool> {
    pool: &'pool PgPool,
    generation: JournalGeneration,
    key: OrderKey,
    link: OrderLink,
    intent: Uuid,
    position: Uuid,
    successor: crate::process_epoch::Epoch,
    durable: DurableCandidate,
    state: RestartState,
    retained: Option<(Decimal, GovernedRestartRead)>,
}

impl<'pool> RestartExecutionBlocker<'pool> {
    /// Bind the obsolete projection read-only under an actual unclean
    /// successor. Any error leaves nothing installed: the caller stays held.
    pub(super) async fn reconstruct(
        pool: &'pool PgPool,
        generation: &JournalGeneration,
        key: OrderKey,
        link: OrderLink,
        successor: &EpochStart,
    ) -> Result<Self, LedgerError> {
        let OrderOwner::Leg {
            position_id,
            intent_id,
            kind: LegOrderKind::Sell,
        } = link.owner
        else {
            return Err(LedgerError::IdentityConflict);
        };
        if !generation.is_valid()
            || successor.epoch.epoch_id.is_nil()
            || successor.epoch.epoch_id == generation.process_epoch
            || successor.predecessor_epoch_id != Some(generation.process_epoch)
            || !matches!(successor.predecessor, Predecessor::NoMarker { .. })
            || successor.unclean_generation.is_none()
            || intent_id.is_nil()
            || position_id.is_nil()
            || key.venue.as_str() != VenueId::BINANCE_SPOT
            || key.venue != link.venue
            || key.account_scope != generation.account_scope
            || key.account_scope != link.account_scope
            || link.client_order_id.is_empty()
        {
            return Err(LedgerError::IdentityConflict);
        }
        let durable =
            read_candidate(pool, generation.tenant_id, &key, intent_id, position_id).await?;
        let blocker = Self {
            pool,
            generation: generation.clone(),
            key,
            link,
            intent: intent_id,
            position: position_id,
            successor: successor.epoch,
            durable,
            state: RestartState::Blocked,
            retained: None,
        };
        blocker.admissible()?;
        Ok(blocker)
    }

    pub(super) fn state(&self) -> RestartState {
        self.state
    }

    pub(super) fn key(&self) -> &OrderKey {
        &self.key
    }

    pub(super) fn retained_execution(&self) -> Option<Decimal> {
        self.retained.as_ref().map(|(executed, _)| *executed)
    }

    /// The exact pre-lookup snapshot must still be current. The returned
    /// borrow is the sole path to applying one subsequent governed receipt.
    pub(super) async fn precheck(&mut self) -> Result<RestartPrecheck<'_, 'pool>, LedgerError> {
        if self.state != RestartState::Blocked || self.read().await? != self.durable {
            return Err(LedgerError::Invariant("restart candidate moved or resolved".into()));
        }
        Ok(RestartPrecheck { owner: self, at: Instant::now() })
    }

    fn admissible(&self) -> Result<(), LedgerError> {
        let (d, g) = (&self.durable, &self.generation);
        let exact = d.latest_epoch == self.successor.epoch_id
            && d.latest_seq == self.successor.epoch_seq
            && d.latest_open
            && d.latest_seq > 0
            && d.prior_epoch == g.process_epoch
            && d.prior_unclean
            && d.generation_id == g.generation_id
            && d.source_audit_id == Some(g.source_audit_id)
            && d.projection_epoch == Some(g.process_epoch)
            && d.projected_version == Some(d.accounting_version)
            && d.client == self.link.client_order_id
            && d.last_client == self.link.client_order_id
            && d.journal_client == self.link.client_order_id
            && d.journal_kind == "exit"
            && !d.not_sent
            && d.venue_outcome.as_deref().is_none_or(|outcome| outcome == "success")
            && d.uid == robson_connectors::inventory_loopback::LOOPBACK_UID.to_string()
            && d.journal_position == Some(self.position)
            && d.side == "SELL"
            && d.order_type == "MARKET"
            && d.status == "PARTIALLY_FILLED"
            && d.phase == "live"
            && d.stop_price.is_none()
            && d.expiry_reason.is_none()
            && d.executed > Decimal::ZERO
            && d.executed < d.orig_qty;
        if !exact {
            return Err(LedgerError::IdentityConflict);
        }
        Ok(())
    }

    fn matches(&self, r: &VenueOrderRecord) -> bool {
        let d = &self.durable;
        r.exchange_order_id == self.key.exchange_order_id.to_string()
            && r.client_order_id == self.link.client_order_id
            && r.orig_client_order_id
                .as_ref()
                .is_none_or(|id| id == &self.link.client_order_id)
            && r.symbol == self.key.symbol
            && r.side == OrderSide::Sell
            && r.order_type == d.order_type
            && r.status == d.status
            && r.phase == OrderPhase::Live
            && r.protection_kind.is_none()
            && r.stop_price.is_none_or(|price| price == Decimal::ZERO)
            && r.expiry_reason.is_none()
            && r.quantities_consistent()
            && r.orig_qty == d.orig_qty
            && r.executed_qty == d.executed
            && r.cumulative_quote_qty.is_some()
            && r.cumulative_quote_qty == d.quote
            && r.update_time_ms.is_some()
            && r.update_time_ms == d.update_time_ms
    }

    async fn read(&self) -> Result<DurableCandidate, LedgerError> {
        read_candidate(self.pool, self.generation.tenant_id, &self.key, self.intent, self.position)
            .await
    }
}

pub(super) struct RestartPrecheck<'owner, 'pool> {
    owner: &'owner mut RestartExecutionBlocker<'pool>,
    at: Instant,
}

impl RestartPrecheck<'_, '_> {
    pub(super) async fn apply(
        self,
        receipt: GovernedRestartRead,
    ) -> Result<RestartState, LedgerError> {
        let Some(OrderObservation::Found(observed)) =
            receipt.after(self.at, self.owner.successor.epoch_id)
        else {
            return Ok(RestartState::Blocked);
        };
        if !self.owner.matches(observed) {
            // Evidence for this key at or above the durable cumulative amount
            // stays held with its original clock/EOF receipt. An older exact
            // reply cannot erase it. Advancing this evidence needs a separate
            // durable reconciliation path.
            let amount = observed.executed_qty;
            if amount >= self.owner.durable.executed
                && self.owner.retained_execution().is_none_or(|retained| amount >= retained)
            {
                self.owner.retained = Some((amount, receipt));
            }
            return Ok(RestartState::Blocked);
        }
        if self.owner.retained.is_some() || self.owner.read().await? != self.owner.durable {
            return Ok(RestartState::Blocked);
        }
        self.owner.state = RestartState::Resolved;
        Ok(self.owner.state)
    }
}

async fn read_candidate(
    pool: &PgPool,
    tenant: Uuid,
    key: &OrderKey,
    intent: Uuid,
    position: Uuid,
) -> Result<DurableCandidate, LedgerError> {
    let row = sqlx::query(DURABLE_CANDIDATE)
        .bind(tenant)
        .bind(&key.account_scope)
        .bind(&key.symbol)
        .bind(key.exchange_order_id.to_string())
        .bind(intent)
        .bind(position)
        .fetch_optional(pool)
        .await
        .map_err(|error| LedgerError::Uncertain(error.to_string()))?
        .ok_or(LedgerError::IdentityConflict)?;
    let get = |error: sqlx::Error| LedgerError::Invariant(error.to_string());
    Ok(DurableCandidate {
        client: row.try_get("client_order_id").map_err(get)?,
        last_client: row.try_get("last_client_order_id").map_err(get)?,
        side: row.try_get("side").map_err(get)?,
        order_type: row.try_get("order_type").map_err(get)?,
        status: row.try_get("status").map_err(get)?,
        phase: row.try_get("phase").map_err(get)?,
        orig_qty: row.try_get("orig_qty").map_err(get)?,
        executed: row.try_get("executed_cum").map_err(get)?,
        quote: row.try_get("cumulative_quote").map_err(get)?,
        update_time_ms: row.try_get("venue_update_time_ms").map_err(get)?,
        stop_price: row.try_get("stop_price").map_err(get)?,
        expiry_reason: row.try_get("expiry_reason").map_err(get)?,
        journal_client: row.try_get("journal_client").map_err(get)?,
        journal_kind: row.try_get("journal_kind").map_err(get)?,
        journal_position: row.try_get("journal_position").map_err(get)?,
        not_sent: row.try_get("not_sent").map_err(get)?,
        venue_outcome: row.try_get("venue_outcome").map_err(get)?,
        uid: row.try_get("uid").map_err(get)?,
        generation_id: row.try_get("generation_id").map_err(get)?,
        source_audit_id: row.try_get("source_audit_id").map_err(get)?,
        projection_epoch: row.try_get("process_epoch").map_err(get)?,
        projected_version: row.try_get("projected_version").map_err(get)?,
        accounting_version: row.try_get("accounting_version").map_err(get)?,
        latest_epoch: row.try_get("latest_epoch").map_err(get)?,
        latest_seq: row.try_get("latest_seq").map_err(get)?,
        latest_open: row.try_get("latest_open").map_err(get)?,
        prior_epoch: row.try_get("prior_epoch").map_err(get)?,
        prior_unclean: row.try_get("prior_unclean").map_err(get)?,
    })
}
