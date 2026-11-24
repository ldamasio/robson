//! PS-3's reader side of the TIME owner (PLAN §6.6): on-demand samples for
//! O1/O2 and C5, and C5's clock evidence. Nothing here sends: the sampler
//! task runs every cycle, one at a time.
use tokio::sync::{oneshot, Notify};

use super::*;

/// The most callers waiting on one on-demand sample (PLAN §6.7's four read
/// slots); a further caller is refused at once.
const WAITERS: usize = 4;

type Answer = oneshot::Sender<Result<Uuid, TimeError>>;

/// An on-demand request the sampler has not taken yet: every caller that
/// asked before it was taken shares its one cycle.
pub(super) struct Request {
    pub(super) deadline: Instant,
    waiters: Vec<Answer>,
}

impl Request {
    pub(super) fn answer(self, result: Result<Uuid, TimeError>) {
        for waiter in self.waiters {
            let _ = waiter.send(result);
        }
    }
}

#[derive(Default)]
pub(super) struct Requests {
    pending: Mutex<Option<Request>>,
    wake: Notify,
}

impl Requests {
    /// Joins the request the sampler has not taken yet, or makes one: at most
    /// four live waiters (a fifth is refused at once, `Resource`).
    pub(super) fn join(
        &self,
        deadline: Instant,
    ) -> Result<oneshot::Receiver<Result<Uuid, TimeError>>, TimeError> {
        let (answer, receiver) = oneshot::channel();
        let mut slot = self.pending.lock().map_err(|_| TimeError::Native)?;
        let request = slot.get_or_insert_with(|| Request { deadline, waiters: Vec::new() });
        request.waiters.retain(|waiter| !waiter.is_closed());
        if request.waiters.len() >= WAITERS {
            return Err(TimeError::Resource);
        }
        request.deadline = request.deadline.max(deadline);
        request.waiters.push(answer);
        drop(slot);
        self.wake.notify_one();
        Ok(receiver)
    }

    pub(super) fn try_take(&self) -> Option<Request> {
        self.pending.lock().ok().and_then(|mut slot| slot.take())
    }

    /// The next request; cancellation-safe (it stays in its slot until taken).
    pub(super) async fn next(&self) -> Request {
        loop {
            match self.try_take() {
                Some(request) => return request,
                None => self.wake.notified().await,
            }
        }
    }
}

/// C5's clock evidence: the published sample of this epoch, its doubled
/// uncertainty, its age, and whether it still fits a fresh native capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClockVerdict {
    pub(crate) sample: Uuid,
    pub(crate) u2: i128,
    pub(crate) age: Duration,
    pub(crate) valid: bool,
}

/// A reader's handle on the TIME owner's state; it sends nothing itself.
#[derive(Clone)]
pub(crate) struct ClockHandle {
    pub(super) clock: Arc<NativeClock>,
    pub(super) epoch: Uuid,
    pub(super) state: Arc<Mutex<ClockState>>,
    pub(super) cancel: CancellationToken,
    pub(super) requests: Arc<Requests>,
}

impl ClockHandle {
    /// One TIME sample whose request is handed off after this call began:
    /// joins the request the sampler has not taken yet, or makes one. Its
    /// cycle is admitted >= 5 s after the previous on-demand one, by the
    /// budget, within the latest waiter's deadline; this caller waits no
    /// longer than its own `deadline`. A dropped caller drops only its own
    /// answer.
    pub(crate) async fn request_sample(&self, deadline: Instant) -> Result<Uuid, TimeError> {
        let receiver = self.requests.join(deadline)?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(TimeError::Closing),
            answer = timeout_at(deadline, receiver) => match answer {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err(TimeError::Closing),
                Err(_) => Err(TimeError::Expired),
            },
        }
    }

    /// The published sample judged on a fresh native capture of this epoch;
    /// no sample (none yet, invalidated, or closing) is an error.
    pub(crate) fn clock_evidence(&self) -> Result<ClockVerdict, TimeError> {
        let mut state = self.state.lock().map_err(|_| TimeError::Native)?;
        if state.closing || self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        let capture = state.capture_native(&self.clock, self.epoch)?;
        let received = state.last_valid.ok_or(TimeError::Native)?;
        let sample = state.published.as_ref().ok_or(TimeError::Native)?;
        let now = capture.end_tick().map_err(|_| TimeError::Native)?;
        Ok(ClockVerdict {
            sample: sample.id,
            u2: sample.u2,
            age: now.elapsed_since(&received).map_err(|_| TimeError::Native)?,
            valid: sample.arithmetic.fits_capture(&capture) == Ok(true),
        })
    }
}
