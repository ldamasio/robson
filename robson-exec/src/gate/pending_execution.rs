//! One cumulative-order slot for the controlled 8j composition.
use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use super::OperationGate;
use crate::{clock_records::RawClockCapture, venue::VenueOrderRecord};

/// Tickets alive at once. A test interleaving cap, never a runtime risk cap.
const OUTSTANDING_TICKETS: u8 = 2;

/// The exact marks carried by one bounded observation. A raw capture alone
/// does not authenticate a sample or authorize a venue request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingExecutionEvidence {
    pub sample: Uuid,
    pub send: RawClockCapture,
    pub receive: RawClockCapture,
    pub executed: Decimal,
}

/// Bound once and never reset: the latest sequence stays after a clear, so a
/// sequence is never issued twice.
#[derive(Debug, Default)]
pub(super) struct PendingExecutionSlot {
    latest: Option<(u64, PendingExecutionEvidence, Arc<VenueOrderRecord>)>,
    pending: bool,
    outstanding: u8,
    exhausted: bool,
    unissued: bool,
}

/// Created once on the actual OperationGate. The daemon's private receiver
/// binds this slot to one immutable native order and journal identity.
pub struct PendingExecutionOwner {
    gate: Arc<OperationGate>,
    epoch: Uuid,
    epoch_seq: i64,
}

/// Move-only. Dropped unacknowledged, it leaves the slot pending.
pub struct PendingExecutionTicket {
    gate: Arc<OperationGate>,
    sequence: u64,
}

pub enum PendingExecutionRegistration {
    Issued(PendingExecutionTicket),
    Retained,
    Refused,
}

impl Drop for PendingExecutionTicket {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if let Some(slot) = state.pending_execution.as_mut() {
            slot.outstanding = slot.outstanding.saturating_sub(1);
        }
    }
}

impl OperationGate {
    /// Isolated composition only. This reserves no venue or trading permit.
    pub fn bind_pending_execution(self: &Arc<Self>, epoch: Uuid) -> Option<PendingExecutionOwner> {
        let mut state = self.lock();
        let epoch_seq = state.epoch?.epoch_seq;
        if epoch.is_nil() || epoch_seq <= 0 || state.pending_execution.is_some() {
            return None;
        }
        state.pending_execution = Some(PendingExecutionSlot::default());
        Some(PendingExecutionOwner { gate: Arc::clone(self), epoch, epoch_seq })
    }
}

impl PendingExecutionOwner {
    /// Synchronously retain the exact newer observation before persistence.
    pub fn register(
        &self,
        evidence: PendingExecutionEvidence,
        record: Arc<VenueOrderRecord>,
    ) -> PendingExecutionRegistration {
        use PendingExecutionRegistration::{Issued, Refused, Retained};

        let mut state = self.gate.lock();
        if state.epoch.map(|epoch| epoch.epoch_seq) != Some(self.epoch_seq)
            || evidence.sample.is_nil()
            || evidence.send.epoch != self.epoch
            || evidence.receive.epoch != self.epoch
            || evidence.send.domain != evidence.receive.domain
            || evidence.send.after_ns >= evidence.receive.before_ns
            || evidence.send.wall_agrees(&evidence.receive) != Ok(true)
            || evidence.executed < Decimal::ZERO
            || record.executed_qty != evidence.executed
        {
            return Refused;
        }
        let Some(slot) = state.pending_execution.as_mut() else {
            return Refused;
        };
        let sequence = match &slot.latest {
            Some((sequence, latest, _)) => {
                if evidence.send.domain != latest.receive.domain
                    || evidence.send.before_ns <= latest.receive.after_ns
                    || evidence.executed < latest.executed
                {
                    return Refused;
                }
                sequence.checked_add(1)
            },
            None => Some(1),
        };
        // Keep the complete work at the same atomic boundary, including when
        // older in-flight tickets prevent issuing a new one immediately.
        slot.latest = Some((sequence.unwrap_or(u64::MAX), evidence, record));
        slot.pending = true;
        slot.exhausted = sequence.is_none();
        slot.unissued = true;
        match self.issue(slot) {
            Some(ticket) => Issued(ticket),
            None => Retained,
        }
    }

    fn issue(&self, slot: &mut PendingExecutionSlot) -> Option<PendingExecutionTicket> {
        if slot.exhausted || !slot.unissued || slot.outstanding >= OUTSTANDING_TICKETS {
            return None;
        }
        let sequence = slot.latest.as_ref()?.0;
        slot.unissued = false;
        slot.outstanding += 1;
        Some(PendingExecutionTicket { gate: Arc::clone(&self.gate), sequence })
    }

    /// Claim only work that was never issued. A dropped issued ticket might
    /// have reached COMMIT and is not retried through this path.
    pub fn take_unissued(
        &self,
    ) -> Option<(PendingExecutionEvidence, Arc<VenueOrderRecord>, PendingExecutionTicket)> {
        let mut state = self.gate.lock();
        if state.epoch.map(|epoch| epoch.epoch_seq) != Some(self.epoch_seq) {
            return None;
        }
        let slot = state.pending_execution.as_mut()?;
        let (_, evidence, record) = slot.latest.as_ref()?;
        let (evidence, record) = (*evidence, Arc::clone(record));
        let ticket = self.issue(slot)?;
        Some((evidence, record, ticket))
    }

    /// Only the issuing gate's current ticket can retire its slot.
    /// The owner calls this after matching the private native commit receipt.
    pub fn acknowledge(&self, ticket: PendingExecutionTicket) -> bool {
        // The lock ends before the ticket drops: its Drop takes the same mutex.
        let acknowledged = Arc::ptr_eq(&self.gate, &ticket.gate) && {
            let mut state = self.gate.lock();
            let current = state.epoch.map(|epoch| epoch.epoch_seq) == Some(self.epoch_seq);
            match state.pending_execution.as_mut() {
                Some(slot)
                    if current
                        && slot.pending
                        && !slot.exhausted
                        && slot.latest.as_ref().map(|(sequence, _, _)| *sequence)
                            == Some(ticket.sequence) =>
                {
                    slot.pending = false;
                    true
                },
                _ => false,
            }
        };
        drop(ticket);
        acknowledged
    }

    pub fn pending(&self) -> Option<PendingExecutionEvidence> {
        let state = self.gate.lock();
        let slot = state.pending_execution.as_ref()?;
        slot.latest.as_ref().filter(|_| slot.pending).map(|(_, evidence, _)| *evidence)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use robson_domain::OrderSide;
    use rust_decimal_macros::dec;

    use super::*;
    use crate::{
        gate::EpochView,
        venue::{OrderPhase, VenueOrderRecord},
        venue_clock::ClockDomain,
    };

    fn register(
        owner: &PendingExecutionOwner,
        e: PendingExecutionEvidence,
    ) -> Option<PendingExecutionTicket> {
        let record = VenueOrderRecord {
            exchange_order_id: "1".into(),
            client_order_id: "local".into(),
            orig_client_order_id: None,
            symbol: "BTCUSDT".into(),
            side: OrderSide::Sell,
            order_type: "MARKET".into(),
            protection_kind: None,
            status: "PARTIALLY_FILLED".into(),
            phase: OrderPhase::Live,
            orig_qty: Decimal::ONE,
            executed_qty: e.executed,
            cumulative_quote_qty: None,
            stop_price: None,
            expiry_reason: None,
            transact_time_ms: None,
            time_ms: None,
            update_time_ms: None,
            working_time_ms: None,
        };
        match owner.register(e, Arc::new(record)) {
            PendingExecutionRegistration::Issued(ticket) => Some(ticket),
            PendingExecutionRegistration::Retained | PendingExecutionRegistration::Refused => None,
        }
    }

    const SECOND: i128 = 1_000_000_000;

    fn capture(epoch: Uuid, domain: ClockDomain, second: i64) -> RawClockCapture {
        let tick = i128::from(second) * SECOND;
        RawClockCapture {
            epoch,
            wall_seconds: second,
            wall_nanoseconds: 0,
            domain,
            before_ns: tick,
            after_ns: tick + 1,
        }
    }

    fn evidence(
        epoch: Uuid,
        domain: ClockDomain,
        send: i64,
        executed: Decimal,
    ) -> PendingExecutionEvidence {
        PendingExecutionEvidence {
            sample: Uuid::from_u128(0x5a),
            send: capture(epoch, domain, send),
            receive: capture(epoch, domain, send + 1),
            executed,
        }
    }

    #[test]
    fn an_equal_quantity_later_mark_supersedes_and_the_old_ticket_cannot_acknowledge() {
        let gate = Arc::new(OperationGate::new(Arc::new(AtomicBool::new(false))));
        gate.begin_epoch(EpochView { epoch_seq: 7, unclean_reason: None }, None, Vec::new());
        let epoch = Uuid::from_u128(0xe9);
        let domain = ClockDomain::new(Uuid::from_u128(0xb0), 0, 0).unwrap();
        let owner = gate.bind_pending_execution(epoch).unwrap();
        assert!(gate.bind_pending_execution(epoch).is_none(), "the slot binds once");

        let first = register(&owner, evidence(epoch, domain, 100, dec!(0.5))).unwrap();
        let later = evidence(epoch, domain, 102, dec!(0.50));
        let second = register(&owner, later).unwrap();

        assert!(!owner.acknowledge(first), "a superseded ticket cannot retire the slot");
        assert_eq!(owner.pending(), Some(later));
        assert!(
            register(&owner, evidence(epoch, domain, 104, dec!(0.4))).is_none(),
            "executed never regresses"
        );
        assert!(owner.acknowledge(second));
        assert_eq!(owner.pending(), None);

        // The cleared slot keeps its sequence: the next ticket is new.
        let third = register(&owner, evidence(epoch, domain, 106, dec!(0.5))).unwrap();
        assert_eq!(third.sequence, 3);
        drop(third);
        assert!(owner.pending().is_some(), "a dropped ticket leaves the slot pending");
    }

    #[test]
    fn a_saturated_slot_retains_the_latest_evidence_without_issuing_a_third_ticket() {
        let gate = Arc::new(OperationGate::new(Arc::new(AtomicBool::new(false))));
        gate.begin_epoch(EpochView { epoch_seq: 7, unclean_reason: None }, None, Vec::new());
        let epoch = Uuid::from_u128(0xe9);
        let domain = ClockDomain::new(Uuid::from_u128(0xb0), 0, 0).unwrap();
        let owner = gate.bind_pending_execution(epoch).unwrap();
        let first = register(&owner, evidence(epoch, domain, 100, dec!(0.25))).unwrap();
        let second = register(&owner, evidence(epoch, domain, 102, dec!(0.50))).unwrap();
        let latest = evidence(epoch, domain, 104, dec!(0.75));
        assert!(register(&owner, latest).is_none());
        assert!(!owner.acknowledge(first));
        assert!(!owner.acknowledge(second));
        assert_eq!(owner.pending(), Some(latest));
    }
}
