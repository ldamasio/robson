//! One cumulative-order slot for the controlled 8j composition.
use std::sync::Arc;

use rust_decimal::Decimal;
use uuid::Uuid;

use super::OperationGate;
use crate::clock_records::RawClockCapture;

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
    latest: Option<(u64, PendingExecutionEvidence)>,
    pending: bool,
    outstanding: u8,
    exhausted: bool,
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
    pub fn register(&self, evidence: PendingExecutionEvidence) -> Option<PendingExecutionTicket> {
        let mut state = self.gate.lock();
        if state.epoch.map(|epoch| epoch.epoch_seq) != Some(self.epoch_seq)
            || evidence.sample.is_nil()
            || evidence.send.epoch != self.epoch
            || evidence.receive.epoch != self.epoch
            || evidence.send.domain != evidence.receive.domain
            || evidence.send.after_ns >= evidence.receive.before_ns
            || evidence.send.wall_agrees(&evidence.receive) != Ok(true)
            || evidence.executed < Decimal::ZERO
        {
            return None;
        }
        let slot = state.pending_execution.as_mut()?;
        let sequence = match slot.latest {
            Some((sequence, latest)) => {
                if evidence.send.domain != latest.receive.domain
                    || evidence.send.before_ns <= latest.receive.after_ns
                    || evidence.executed < latest.executed
                {
                    return None;
                }
                sequence.checked_add(1)
            },
            None => Some(1),
        };
        // Retain a valid higher observation even when no ticket can be issued.
        // Earlier in-flight receipts must never clear this evidence.
        slot.latest = Some((sequence.unwrap_or(u64::MAX), evidence));
        slot.pending = true;
        slot.exhausted = sequence.is_none();
        let sequence = sequence?;
        if slot.outstanding >= OUTSTANDING_TICKETS {
            return None;
        }
        slot.outstanding += 1;
        Some(PendingExecutionTicket { gate: Arc::clone(&self.gate), sequence })
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
                        && slot.latest.map(|(sequence, _)| sequence) == Some(ticket.sequence) =>
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
        slot.latest.filter(|_| slot.pending).map(|(_, evidence)| evidence)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use rust_decimal_macros::dec;

    use super::*;
    use crate::{gate::EpochView, venue_clock::ClockDomain};

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

        let first = owner.register(evidence(epoch, domain, 100, dec!(0.5))).unwrap();
        let later = evidence(epoch, domain, 102, dec!(0.50));
        let second = owner.register(later).unwrap();

        assert!(!owner.acknowledge(first), "a superseded ticket cannot retire the slot");
        assert_eq!(owner.pending(), Some(later));
        assert!(
            owner.register(evidence(epoch, domain, 104, dec!(0.4))).is_none(),
            "executed never regresses"
        );
        assert!(owner.acknowledge(second));
        assert_eq!(owner.pending(), None);

        // The cleared slot keeps its sequence: the next ticket is new.
        let third = owner.register(evidence(epoch, domain, 106, dec!(0.5))).unwrap();
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
        let first = owner.register(evidence(epoch, domain, 100, dec!(0.25))).unwrap();
        let second = owner.register(evidence(epoch, domain, 102, dec!(0.50))).unwrap();
        let latest = evidence(epoch, domain, 104, dec!(0.75));
        assert!(owner.register(latest).is_none());
        assert!(!owner.acknowledge(first));
        assert!(!owner.acknowledge(second));
        assert_eq!(owner.pending(), Some(latest));
    }
}
