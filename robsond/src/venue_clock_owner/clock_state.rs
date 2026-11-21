//! Bounded active failure episodes and latched visibility.
use super::*;

impl ClockState {
    pub(super) fn begin_closing(&mut self) {
        if !self.closing {
            if let Some(next) = self.generation.checked_add(1) {
                self.generation = next;
            }
        }
        self.closing = true;
        self.published = None;
        metrics::evidence(None);
        // Normal shutdown is not a fabricated native failure or alert.
    }

    /// The publication CAS (C8): a sample persisted by the cycle that began
    /// at `generation` is published only if no fault edge, stop or closing
    /// moved the generation since; it clears the active faults.
    pub(super) fn publish(
        &mut self,
        generation: u64,
        sample: Published,
        received: TickCapture,
    ) -> Result<Uuid, TimeError> {
        if self.closing || self.generation != generation {
            return Err(TimeError::Expired);
        }
        self.last_valid = Some(received);
        self.faults = 0;
        self.active_faults = 0;
        metrics::faults(0, 0);
        metrics::evidence(Some(sample.u2));
        let id = sample.id;
        self.published = Some(sample);
        Ok(id)
    }

    pub(super) fn native_ok(&mut self) {
        self.clear_active(ClockFault::Native);
    }
    pub(super) fn clear_active(&mut self, reason: ClockFault) {
        self.active_faults &= !(reason as u8);
    }

    /// Caller holds the single decision mutex. An old returned HTTP bracket
    /// cannot clear a newer failure: this always performs a fresh actual read.
    pub(super) fn capture_native(
        &mut self,
        clock: &NativeClock,
        epoch: Uuid,
    ) -> Result<RawClockCapture, TimeError> {
        let capture = clock.wall_bracket(epoch).and_then(|capture| {
            if capture.epoch != epoch || capture.domain != self.start.domain() {
                return Err(robson_exec::venue_clock::ClockError::Incomparable);
            }
            capture.end_tick()?.elapsed_since(&self.start)?;
            Ok(capture)
        });
        match capture {
            Ok(capture) => {
                self.native_ok();
                Ok(capture)
            },
            Err(_) => {
                self.invalidate(ClockFault::Native);
                Err(TimeError::Native)
            },
        }
    }

    pub(super) fn invalidate(&mut self, fault: ClockFault) {
        self.published = None;
        metrics::evidence(None);
        let edge = fault == ClockFault::WallStep || self.active_faults & fault as u8 == 0;
        if edge {
            match self.generation.checked_add(1) {
                Some(next) => self.generation = next,
                None => self.closing = true,
            }
        }
        self.active_faults |= fault as u8;
        self.faults |= fault as u8;
        metrics::faults(self.faults, if edge { fault as u8 } else { 0 });
    }
}
