//! Conservative framing boundary; unsupported cycles never borrow a phase.
use super::*;

impl Connection<'_> {
    pub(in crate::db_proxy) fn frontend(
        &self,
        tag: u8,
        category: SqlCategory,
        association_supported: bool,
    ) -> Option<Record> {
        let phase = match tag {
            b'P' => Phase::Parse,
            b'B' => Phase::Bind,
            b'D' => Phase::Describe,
            b'C' => Phase::Close,
            b'E' => Phase::Execute,
            b'Q' => Phase::Simple,
            b'S' => Phase::Sync,
            b'F' => {
                let mut state = self.state.lock().unwrap();
                state.lose_attribution();
                self.emit(Self::record(&state, Pending::unknown(), Event::AttributionLost));
                return None;
            },
            _ => return None,
        };
        let mut state = self.state.lock().unwrap();
        if phase == Phase::Simple && state.frontend_extended_open && !state.desynchronized {
            state.lose_attribution();
            self.emit(Self::record(&state, Pending::unknown(), Event::AttributionLost));
        }
        match phase {
            Phase::Parse | Phase::Bind | Phase::Describe | Phase::Close | Phase::Execute => {
                state.frontend_extended_open = true
            },
            Phase::Sync => state.frontend_extended_open = false,
            _ => {},
        }
        let ordinal = if matches!(phase, Phase::Execute | Phase::Simple) {
            state.next_ordinal = state.next_ordinal.saturating_add(1);
            Some(state.next_ordinal)
        } else {
            None
        };
        if matches!(phase, Phase::Parse | Phase::Bind | Phase::Execute | Phase::Simple)
            && !association_supported
        {
            state.lose_attribution();
        }
        let pending = Pending { phase, category, ordinal };
        if !state.desynchronized {
            if state.pending.len() == PHASE_LIMIT {
                state.lose_attribution();
                if let Some(trace) = self.trace.lock().unwrap().as_mut() {
                    trace.phase_overflow = trace.phase_overflow.saturating_add(1);
                }
                self.emit(Self::record(&state, Pending::unknown(), Event::Overflow));
            } else {
                state.pending.push_back(pending);
            }
        }
        let association = if state.desynchronized {
            Pending::unknown()
        } else {
            pending
        };
        let record = Self::record(&state, association, Event::Sent);
        self.emit(record);
        Some(record)
    }
}
