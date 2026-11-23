//! One cumulative-order slot for the controlled 8j composition. The slot's
//! state and its read compile in every build (ADR-0056 step 8l-2); the
//! receiver that binds and fills it stays the composition's (`crash-points`)
//! until 8j-2 binds it.
#[cfg(feature = "crash-points")]
use std::sync::Arc;

#[cfg(feature = "crash-points")]
use rust_decimal::Decimal;
#[cfg(feature = "crash-points")]
use uuid::Uuid;

use super::GateState;
#[cfg(feature = "crash-points")]
use super::OperationGate;
#[cfg(feature = "crash-points")]
use crate::{clock_records::RawClockCapture, venue::VenueOrderRecord};

/// Venue evidence with an accounting effect, received and not yet committed
/// (v16 §3.7.3 R5-1), as the gate holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingEvidence {
    /// No receiver is bound, so none can be registered: only
    /// `bind_pending_execution` creates the slot. Publication reads it as
    /// none pending; a spot entry is still refused while unbound (8l-7).
    Unbound,
    /// Bound, and the latest registered evidence was acknowledged.
    Clear,
    /// Registered and unacknowledged: its write may not have committed. A
    /// dropped or superseded ticket and an exhausted sequence leave it here.
    Pending,
}

impl PendingEvidence {
    /// Whether a cash conclusion or a snapshot must refuse to publish.
    pub fn blocks_publication(self) -> bool {
        self == Self::Pending
    }
}

impl GateState {
    pub(super) fn pending_evidence(&self) -> PendingEvidence {
        match &self.pending_execution {
            None => PendingEvidence::Unbound,
            Some(slot) if slot.pending => PendingEvidence::Pending,
            Some(_) => PendingEvidence::Clear,
        }
    }
}

/// Tickets alive at once. A test interleaving cap, never a runtime risk cap.
#[cfg(feature = "crash-points")]
const OUTSTANDING_TICKETS: u8 = 2;

/// The exact marks carried by one bounded observation. A raw capture alone
/// does not authenticate a sample or authorize a venue request.
#[cfg(feature = "crash-points")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingExecutionEvidence {
    pub sample: Uuid,
    pub send: RawClockCapture,
    pub receive: RawClockCapture,
    pub executed: Decimal,
}

/// Bound once and never reset: the latest sequence stays after a clear, so a
/// sequence is never issued twice. `pending` only ever falls by an exact
/// acknowledgement of the latest sequence.
#[derive(Debug, Default)]
#[cfg_attr(
    not(feature = "crash-points"),
    allow(
        dead_code,
        reason = "only the receiver creates it, crash-points until 8j-2"
    )
)]
pub(super) struct PendingExecutionSlot {
    #[cfg(feature = "crash-points")]
    latest: Option<(u64, PendingExecutionEvidence, Arc<VenueOrderRecord>)>,
    pending: bool,
    #[cfg(feature = "crash-points")]
    outstanding: u8,
    #[cfg(feature = "crash-points")]
    exhausted: bool,
    #[cfg(feature = "crash-points")]
    unissued: bool,
}

/// Created once on the actual OperationGate. The daemon's private receiver
/// binds this slot to one immutable native order and journal identity.
#[cfg(feature = "crash-points")]
pub struct PendingExecutionOwner {
    gate: Arc<OperationGate>,
    epoch: Uuid,
    epoch_seq: i64,
}

/// Move-only. Dropped unacknowledged, it leaves the slot pending.
#[cfg(feature = "crash-points")]
pub struct PendingExecutionTicket {
    gate: Arc<OperationGate>,
    sequence: u64,
}

#[cfg(feature = "crash-points")]
pub enum PendingExecutionRegistration {
    Issued(PendingExecutionTicket),
    Retained,
    Refused,
}

#[cfg(feature = "crash-points")]
impl Drop for PendingExecutionTicket {
    fn drop(&mut self) {
        let mut state = self.gate.lock();
        if let Some(slot) = state.pending_execution.as_mut() {
            slot.outstanding = slot.outstanding.saturating_sub(1);
        }
    }
}

#[cfg(feature = "crash-points")]
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

#[cfg(feature = "crash-points")]
impl PendingExecutionOwner {
    /// Synchronously retain the exact newer observation before persistence.
    /// A retained rise moves the invalidity generation in the same critical
    /// section (v16 §3.7.3 R5-1); a refusal retains and moves nothing.
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
        let registration = match self.issue(slot) {
            Some(ticket) => Issued(ticket),
            None => Retained,
        };
        state.invalidate();
        registration
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
    /// The fall never moves the invalidity generation.
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

#[cfg(all(test, feature = "crash-points"))]
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

    fn bound() -> (Arc<OperationGate>, PendingExecutionOwner, Uuid, ClockDomain) {
        let gate = Arc::new(OperationGate::new(Arc::new(AtomicBool::new(false))));
        gate.begin_epoch(EpochView { epoch_seq: 7, unclean_reason: None }, None, Vec::new());
        let epoch = Uuid::from_u128(0xe9);
        let domain = ClockDomain::new(Uuid::from_u128(0xb0), 0, 0).unwrap();
        let owner = gate.bind_pending_execution(epoch).unwrap();
        (gate, owner, epoch, domain)
    }

    /// Each retained rise, issued or not, moves the invalidity generation
    /// under the mutex; the fall and a refusal never move it, and a rise is
    /// not an occurrence: fence and incident generation stay as they were.
    #[test]
    fn pending_rise_moves_the_invalidity_generation() {
        use crate::gate::{InvalidityMarks, PendingEvidence::*};
        let (gate, owner, epoch, domain) = bound();
        let marks = |generation, pending| InvalidityMarks { generation, pending };
        assert_eq!(gate.invalidity_marks(), marks(0, Clear));
        let incident_generation = gate.generation();

        let first = register(&owner, evidence(epoch, domain, 100, dec!(0.25))).unwrap();
        assert_eq!(gate.invalidity_marks(), marks(1, Pending));
        assert!(!gate.fenced() && gate.generation() == incident_generation);
        assert!(owner.acknowledge(first));
        assert_eq!(gate.invalidity_marks(), marks(1, Clear), "the fall never moves it");

        assert!(register(&owner, evidence(epoch, domain, 102, dec!(0.1))).is_none());
        assert_eq!(gate.invalidity_marks(), marks(1, Clear), "a refusal retains nothing");

        let _second = register(&owner, evidence(epoch, domain, 104, dec!(0.5))).unwrap();
        let _third = register(&owner, evidence(epoch, domain, 106, dec!(0.5))).unwrap();
        assert!(register(&owner, evidence(epoch, domain, 108, dec!(0.5))).is_none());
        assert_eq!(gate.invalidity_marks(), marks(4, Pending), "a retained rise moves it too");
    }

    /// The read reports evidence pending whenever its commit is unproven: a
    /// dropped ticket, a superseded one, an exhausted sequence.
    #[test]
    fn pending_evidence_read_fails_closed() {
        let (gate, owner, epoch, domain) = bound();
        drop(register(&owner, evidence(epoch, domain, 100, dec!(0.25))).unwrap());
        assert_eq!(gate.pending_evidence(), PendingEvidence::Pending, "dropped ticket");

        let (gate, owner, epoch, domain) = bound();
        let old = register(&owner, evidence(epoch, domain, 100, dec!(0.25))).unwrap();
        drop(register(&owner, evidence(epoch, domain, 102, dec!(0.5))).unwrap());
        assert!(!owner.acknowledge(old));
        assert_eq!(gate.pending_evidence(), PendingEvidence::Pending, "superseded ticket");

        let (gate, owner, epoch, domain) = bound();
        let last = register(&owner, evidence(epoch, domain, 100, dec!(0.25))).unwrap();
        if let Some((sequence, _, _)) =
            gate.lock().pending_execution.as_mut().and_then(|slot| slot.latest.as_mut())
        {
            *sequence = u64::MAX;
        }
        assert!(register(&owner, evidence(epoch, domain, 102, dec!(0.5))).is_none());
        assert!(!owner.acknowledge(last));
        assert!(owner.take_unissued().is_none());
        assert_eq!(gate.pending_evidence(), PendingEvidence::Pending, "exhausted sequence");
    }
}
