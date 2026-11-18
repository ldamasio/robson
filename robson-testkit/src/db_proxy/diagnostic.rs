//! Opt-in protocol metadata. Borrowed frames yield only validated metadata;
//! packets, SQL text, parameters and error messages are never retained.
//! Ambiguous response attribution stays explicitly unknown.
use std::{collections::VecDeque, sync::Mutex};

pub const RECORD_LIMIT: usize = 4_096;
const CONNECTION_LIMIT: usize = 32;
const PHASE_LIMIT: usize = 32;

#[derive(Clone, Copy)]
struct Pending {
    phase: Phase,
    category: SqlCategory,
    ordinal: Option<u64>,
}

impl Pending {
    fn unknown() -> Self {
        Self {
            phase: Phase::Unknown,
            category: SqlCategory::Other,
            ordinal: None,
        }
    }
}

struct State {
    identity: Identity,
    next_ordinal: u64,
    pending: VecDeque<Pending>,
    completed: Option<Pending>,
    // Frontend group since the last enqueued Sync; ACKs cannot close it.
    frontend_extended_open: bool,
    // Never reconstruct an association after losing a pipelined boundary.
    // Remaining unknown until close is preferable to an invented ordinal.
    desynchronized: bool,
}

impl State {
    fn lose_attribution(&mut self) {
        self.pending.clear();
        self.completed = None;
        self.desynchronized = true;
        self.identity.attribution_unknown = true;
    }
}

pub(super) struct Connection<'a> {
    trace: &'a Mutex<Option<Trace>>,
    state: Mutex<State>,
}

impl<'a> Connection<'a> {
    pub(super) fn new(
        trace: &'a Mutex<Option<Trace>>,
        id: u64,
        journal: Option<JournalLabel>,
    ) -> Option<Self> {
        let mut held = trace.lock().unwrap();
        let trace_state = held.as_mut()?;
        if trace_state.connections.len() == CONNECTION_LIMIT {
            trace_state.connection_overflow = trace_state.connection_overflow.saturating_add(1);
            return None;
        }
        let identity = Identity {
            connection: id,
            pid: None,
            journal,
            live: true,
            attribution_unknown: false,
        };
        trace_state.connections.push(identity);
        let state = State {
            identity,
            next_ordinal: 0,
            pending: VecDeque::with_capacity(PHASE_LIMIT),
            completed: None,
            frontend_extended_open: false,
            desynchronized: false,
        };
        trace_state.push(Self::record(&state, Pending::unknown(), Event::Connected));
        Some(Self { trace, state: Mutex::new(state) })
    }

    fn record(state: &State, pending: Pending, event: Event) -> Record {
        Record {
            micros: 0,
            identity: state.identity,
            ordinal: pending.ordinal,
            phase: pending.phase,
            category: pending.category,
            event,
            delivery: Delivery::Local,
            sqlstate: None,
            transaction: None,
        }
    }

    fn emit(&self, record: Record) {
        // The only nested order is state -> trace, and neither guard crosses
        // an await. Snapshot takes only trace, never a connection state lock.
        if let Some(trace) = self.trace.lock().unwrap().as_mut() {
            trace.push(record);
        }
    }

    pub(super) fn backend(&self, tag: u8, body: &[u8]) -> Option<Record> {
        let mut state = self.state.lock().unwrap();
        let mut pending = if state.desynchronized {
            Pending::unknown()
        } else {
            state.pending.front().copied().unwrap_or_else(Pending::unknown)
        };
        let mut preparation_invalidated = false;
        let event = match tag {
            b'K' if body.len() == 8 => {
                pending = Pending::unknown();
                // The cancellation secret (last four bytes) is never copied.
                let pid = i32::from_be_bytes(body[..4].try_into().unwrap());
                state.identity.pid = (pid > 0).then_some(pid);
                Event::BackendPid
            },
            b'1' | b'2' | b'3' | b'T' | b'n' => {
                let expected = match tag {
                    b'1' => Phase::Parse,
                    b'2' => Phase::Bind,
                    b'3' => Phase::Close,
                    _ => Phase::Describe,
                };
                if pending.phase == expected {
                    state.pending.pop_front();
                } else if !matches!((tag, pending.phase), (b'T', Phase::Simple | Phase::Execute)) {
                    state.lose_attribution();
                    self.emit(Self::record(&state, Pending::unknown(), Event::AttributionLost));
                }
                // Metadata acknowledgements need no retained event.
                return None;
            },
            b'C' | b'I' | b's' => {
                if matches!(pending.phase, Phase::Execute | Phase::Simple) {
                    if pending.phase == Phase::Execute {
                        state.pending.pop_front();
                        // Multiple completions within one implicit transaction
                        // do not identify which statement caused a deferred
                        // error. Ambiguity is sticky until ReadyForQuery.
                        state.completed = Some(if state.completed.is_none() {
                            pending
                        } else {
                            Pending::unknown()
                        });
                    }
                } else {
                    state.lose_attribution();
                    pending = Pending::unknown();
                }
                Event::Complete
            },
            b'E' => {
                if fatal_response(body) {
                    state.lose_attribution();
                    pending = Pending::unknown();
                } else {
                    preparation_invalidated = matches!(
                        pending.phase,
                        Phase::Parse | Phase::Bind | Phase::Describe | Phase::Close
                    );
                    let simple_error = pending.phase == Phase::Simple;
                    if pending.phase == Phase::Sync {
                        pending = state.completed.unwrap_or_else(Pending::unknown);
                    }
                    if !state.desynchronized && !simple_error {
                        // Extended errors discard every message through Sync,
                        // including an interposed simple Query. If Sync has not
                        // arrived, no future group boundary is proven.
                        let mut failing = true;
                        while state.pending.front().is_some_and(|p| p.phase != Phase::Sync) {
                            let skipped = state.pending.pop_front().unwrap();
                            if !failing {
                                preparation_invalidated |=
                                    matches!(skipped.phase, Phase::Parse | Phase::Bind);
                                self.emit(Self::record(&state, skipped, Event::Skipped));
                            }
                            failing = false;
                        }
                        if state.pending.front().is_none() {
                            state.lose_attribution();
                            pending = Pending::unknown();
                        }
                    }
                }
                Event::Error
            },
            b'Z' => {
                if !state.desynchronized {
                    if matches!(pending.phase, Phase::Sync | Phase::Simple) {
                        state.pending.pop_front();
                        // ReadyForQuery describes the whole transaction/group,
                        // not one execution inside a pipelined group.
                        pending = Pending::unknown();
                    } else {
                        if !state.pending.is_empty() {
                            state.lose_attribution();
                        }
                        pending = Pending::unknown();
                    }
                    state.completed = None;
                }
                Event::Ready
            },
            _ => return None,
        };
        let mut record = Self::record(&state, pending, event);
        record.delivery = Delivery::Received;
        if tag == b'E' {
            record.sqlstate = sqlstate(body);
        }
        if tag == b'Z' {
            record.transaction = body.first().copied().filter(|b| matches!(b, b'I' | b'T' | b'E'));
        }
        self.emit(record);
        if preparation_invalidated && !state.desynchronized {
            // The inherited SQL cache observed requests before acceptance.
            // Keep the rejected phase as historical evidence, then prevent
            // rejected/skipped preparation from naming future executions.
            state.lose_attribution();
            self.emit(Self::record(&state, Pending::unknown(), Event::AttributionLost));
        }
        Some(record)
    }

    pub(super) fn delivery(&self, mut record: Record, delivery: Delivery) {
        record.delivery = delivery;
        self.emit(record);
    }
}

impl Drop for Connection<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.identity.live = false;
        self.emit(Self::record(&state, Pending::unknown(), Event::Closed));
    }
}

#[path = "diagnostic/record.rs"]
mod record;
pub use record::{Delivery, Event, Identity, JournalLabel, Phase, Record, Snapshot, SqlCategory};
#[path = "diagnostic/trace.rs"]
mod trace;
pub(super) use trace::Trace;

mod validation;
pub(super) use validation::journal_label;
use validation::{fatal_response, sqlstate};
mod frontend;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod properties;
