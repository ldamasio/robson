//! Callable 8j receive/persist/ack boundary in the isolated composition.
//! The owner retains one exact cumulative-order observation under its actual
//! operation gate before any persistence await. No live loop selects it.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use robson_exec::{
    gate::{
        OperationGate, PendingExecutionEvidence, PendingExecutionOwner,
        PendingExecutionRegistration, PendingExecutionTicket,
    },
    venue::{OrderPhase, VenueId, VenueOrderRecord},
    venue_ledger::{LedgerError, LegOrderKind, OrderKey, OrderLink, OrderOwner, SeenAt},
};
use rust_decimal::Decimal;

use super::{JournalGeneration, PgVenueLedger};

pub(super) struct OrderExecutionReceiver<'ledger> {
    gate: PendingExecutionOwner,
    ledger: &'ledger PgVenueLedger,
    generation: JournalGeneration,
    key: OrderKey,
    link: OrderLink,
}

pub(super) struct RegisteredExecution<'receiver, 'ledger> {
    owner: &'receiver OrderExecutionReceiver<'ledger>,
    ticket: PendingExecutionTicket,
    record: Arc<VenueOrderRecord>,
    seen: SeenAt,
}

pub(super) struct ExecutionAcknowledgement<'receiver> {
    gate: &'receiver PendingExecutionOwner,
    ticket: PendingExecutionTicket,
    pub(super) accounting_version: i64,
}

impl<'ledger> OrderExecutionReceiver<'ledger> {
    pub(super) fn bind(
        gate: &Arc<OperationGate>,
        ledger: &'ledger PgVenueLedger,
        key: OrderKey,
        link: OrderLink,
    ) -> Result<Self, LedgerError> {
        let generation = ledger.order_projection.as_ref().ok_or(LedgerError::IdentityConflict)?;
        if !generation.is_valid()
            || generation.tenant_id != ledger.tenant_id
            || key.venue.as_str() != VenueId::BINANCE_SPOT
            || key.venue != link.venue
            || key.account_scope != generation.account_scope
            || key.account_scope != link.account_scope
            || link.client_order_id.is_empty()
            || !matches!(link.owner, OrderOwner::Leg { intent_id, position_id, .. }
                if !intent_id.is_nil() && !position_id.is_nil())
        {
            return Err(LedgerError::IdentityConflict);
        }
        let gate = gate
            .bind_pending_execution(generation.process_epoch)
            .ok_or(LedgerError::IdentityConflict)?;
        Ok(Self {
            gate,
            ledger,
            generation: generation.clone(),
            key,
            link,
        })
    }

    /// This synchronous entry takes ownership of the record before its write.
    pub(super) fn register(
        &self,
        record: VenueOrderRecord,
        evidence: PendingExecutionEvidence,
    ) -> Result<Option<RegisteredExecution<'_, 'ledger>>, LedgerError> {
        if OrderKey::from_record(&record, &self.link)? != self.key
            || record.exchange_order_id != self.key.exchange_order_id.to_string()
            || record.client_order_id != self.link.client_order_id
        {
            return Err(LedgerError::IdentityConflict);
        }
        let side_matches = matches!(
            (&self.link.owner, record.side),
            (OrderOwner::Leg { kind: LegOrderKind::Entry, .. }, robson_domain::OrderSide::Buy)
                | (
                    OrderOwner::Leg { kind: LegOrderKind::Sell, .. },
                    robson_domain::OrderSide::Sell
                )
        );
        if !side_matches
            || record.order_type != "MARKET"
            || record.protection_kind.is_some()
            || record.stop_price.is_some_and(|price| price != Decimal::ZERO)
            || record.expiry_reason.is_some()
            || record
                .orig_client_order_id
                .as_ref()
                .is_some_and(|id| id != &self.link.client_order_id)
            || record.cumulative_quote_qty.is_some_and(|qty| qty < Decimal::ZERO)
            || record.phase != OrderPhase::Live
            || record.status != "PARTIALLY_FILLED"
            || !record.quantities_consistent()
            || record.executed_qty <= Decimal::ZERO
            || record.executed_qty >= record.orig_qty
            || record.executed_qty != evidence.executed
            || evidence.receive.epoch != self.generation.process_epoch
        {
            return Err(LedgerError::InvalidEvidence("not an exact cumulative execution".into()));
        }
        let seen = receive_mark(evidence)?;
        let record = Arc::new(record);
        match self.gate.register(evidence, Arc::clone(&record)) {
            PendingExecutionRegistration::Issued(ticket) => {
                Ok(Some(RegisteredExecution { owner: self, ticket, record, seen }))
            },
            PendingExecutionRegistration::Retained => Ok(None),
            PendingExecutionRegistration::Refused => {
                Err(LedgerError::InvalidEvidence("pending slot refused observation".into()))
            },
        }
    }

    pub(super) fn take_retained(
        &self,
    ) -> Result<Option<RegisteredExecution<'_, 'ledger>>, LedgerError> {
        let Some((evidence, record, ticket)) = self.gate.take_unissued() else {
            return Ok(None);
        };
        Ok(Some(RegisteredExecution {
            owner: self,
            ticket,
            record,
            seen: receive_mark(evidence)?,
        }))
    }

    pub(super) fn pending(&self) -> Option<PendingExecutionEvidence> {
        self.gate.pending()
    }
}

fn receive_mark(evidence: PendingExecutionEvidence) -> Result<SeenAt, LedgerError> {
    let at = DateTime::<Utc>::from_timestamp(
        evidence.receive.wall_seconds,
        evidence.receive.wall_nanoseconds,
    )
    .ok_or_else(|| LedgerError::InvalidEvidence("invalid receive timestamp".into()))?;
    Ok(SeenAt {
        at,
        clock: None,
        capture: Some(evidence.receive),
    })
}

impl<'receiver, 'ledger> RegisteredExecution<'receiver, 'ledger> {
    /// Use the exact immutable work paired with the gate ticket. None means
    /// no positive changed-write receipt and leaves the observation held.
    pub(super) async fn persist(
        self,
    ) -> Result<Option<ExecutionAcknowledgement<'receiver>>, LedgerError> {
        let owner = self.owner;
        let (_, commit) =
            owner.ledger.record_order_observed(&self.record, &owner.link, self.seen).await?;
        let Some(commit) = commit else {
            return Ok(None);
        };
        if commit.key() != &owner.key
            || commit.owner() != &owner.link.owner
            || commit.generation() != &owner.generation
            || commit.executed_cum() != self.record.executed_qty
            || commit.accounting_version() <= 0
        {
            return Err(LedgerError::Invariant(
                "native commit differs from pending observation".into(),
            ));
        }
        Ok(Some(ExecutionAcknowledgement {
            gate: &owner.gate,
            ticket: self.ticket,
            accounting_version: commit.accounting_version(),
        }))
    }
}

impl ExecutionAcknowledgement<'_> {
    pub(super) fn apply(self) -> bool {
        self.gate.acknowledge(self.ticket)
    }
}
