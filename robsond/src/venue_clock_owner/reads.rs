//! The budgeted read owner of a non-trading Spot start (PS-5, PLAN-r4 §6.7):
//! the C8 inventory owner's steps made production. A read is reserved at its
//! fixed weight, `now` taken under the budget lock; prepared on a fresh
//! one-shot transport on one of the reads' two blocking permits (apart from
//! the sampler's); signed with the owner's timestamp; first polled under the
//! clock guard then the budget guard, which hand off its charge; read to EOF
//! within its cap; its receipt checked again at EOF; decoded off the runtime.
//! At most four reads are in flight: a fifth is refused at once, never
//! queued. Every wait ends at the caller's deadline, and dropping a read
//! returns its slot and permits and refunds a charge not yet handed off.
#![allow(dead_code, reason = "no production caller until PS-6")]

use std::{future::Future, pin::Pin, task::Poll};

use robson_connectors::binance::spot::{
    PreparedSpotCommission, PreparedSpotRead, SpotCommission, SpotPreparedRead, SpotReadBody,
    SPOT_COMMISSION_BODY_CAP, SPOT_OPERATED_SYMBOL,
};
use tokio::sync::OwnedSemaphorePermit;

use super::{commission::CommissionSigningTime, *};
use crate::venue_budget::state::{PendingRead, ReadKind};

const REQUEST_BOUND: Duration = Duration::from_secs(10);
const RECEIPT_LIMIT: Duration = Duration::from_secs(60);

/// One read the owner sends: one of the closed set, or the operated pair's
/// commission on its existing prepared transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    Closed(SpotPreparedRead),
    Commission,
}

#[derive(Debug)]
pub(crate) enum ReadBody {
    Closed(SpotReadBody),
    Commission(SpotCommission),
}

/// One completed read: the published sample (id, u2) and generation its first
/// poll was admitted on (a signed read's always names one), the send and EOF
/// brackets and the handoff mark as produced, the status, the decoded body.
#[derive(Debug)]
pub(crate) struct ReadReceipt {
    pub(crate) read: Read,
    pub(crate) generation: u64,
    pub(crate) sample: Option<(Uuid, i128)>,
    pub(crate) send: RawClockCapture,
    pub(crate) handoff: TickCapture,
    pub(crate) receive: RawClockCapture,
    pub(crate) status: u16,
    pub(crate) body: ReadBody,
}

struct Mark {
    generation: u64,
    sample: Option<(Uuid, i128)>,
    raw: RawClockCapture,
    handoff: TickCapture,
}

enum Prepared {
    Read(PreparedSpotRead),
    Commission(PreparedSpotCommission),
}

type Sending = Pin<Box<dyn Future<Output = reqwest::Result<reqwest::Response>> + Send>>;

/// One read's outcome, counted once whatever ends it (`dropped`: the caller
/// dropped it first; `None`: its read counts it), with the bucket gauge
/// refreshed (PLAN §6.11).
struct Metered<'a>(&'a SpotReads, ReadKind, Option<&'static str>);

impl Drop for Metered<'_> {
    fn drop(&mut self) {
        if let Some(outcome) = self.2 {
            metrics::read(self.1, outcome);
        }
        metrics::bucket(self.0.budget.usage(|| self.0.handle.clock.capture().ok()));
    }
}

#[derive(Clone)]
pub(crate) struct SpotReads {
    handle: ClockHandle,
    client: Arc<BinanceRestClient>,
    budget: NonTradingBudget,
    workers: Arc<Semaphore>,
    slots: Arc<Semaphore>,
}

impl Read {
    fn budget(self) -> ReadKind {
        match self {
            Self::Closed(SpotPreparedRead::Account) => ReadKind::Account,
            Self::Closed(SpotPreparedRead::OperatedOpenOrders) => ReadKind::OperatedOpenOrders,
            Self::Closed(SpotPreparedRead::OpenOrderLists) => ReadKind::OpenOrderLists,
            Self::Closed(SpotPreparedRead::ExchangeInfo) => ReadKind::ExchangeInfo,
            Self::Closed(SpotPreparedRead::Kline { .. }) => ReadKind::Kline,
            Self::Commission => ReadKind::Commission,
        }
    }

    fn signed(self) -> bool {
        matches!(self, Self::Commission) || matches!(self, Self::Closed(r) if r.request().signed)
    }

    fn cap(self) -> usize {
        match self {
            Self::Closed(read) => read.body_cap(),
            Self::Commission => SPOT_COMMISSION_BODY_CAP,
        }
    }
}

impl SpotReads {
    /// The read owner of a non-trading start, on its TIME owner's handle,
    /// its closed client and its budget. No production caller until PS-6.
    pub(crate) fn new(
        handle: ClockHandle,
        client: Arc<BinanceRestClient>,
        budget: NonTradingBudget,
    ) -> Self {
        let (workers, slots) = (Arc::new(Semaphore::new(2)), Arc::new(Semaphore::new(4)));
        Self { handle, client, budget, workers, slots }
    }

    /// 8k-1 transport B's triple: ACCOUNT, BTCUSDT's open orders, the open
    /// order lists, in that order, all signed under one signing time (one
    /// sample, one generation); the first failure ends it (32 weight at most).
    pub(crate) async fn inventory(&self, deadline: Instant) -> Result<[ReadReceipt; 3], TimeError> {
        let (_slot, deadline) = self.refused(ReadKind::Account, self.slot(deadline, 15))?;
        let signed = self.refused(ReadKind::Account, self.handle.signing_time())?;
        let mut receipts = Vec::with_capacity(3);
        for read in [
            SpotPreparedRead::Account,
            SpotPreparedRead::OperatedOpenOrders,
            SpotPreparedRead::OpenOrderLists,
        ] {
            receipts.push(self.one(Read::Closed(read), Some(&signed), deadline).await?);
        }
        receipts.try_into().map_err(|_| TimeError::Resource)
    }

    /// O1/O2 (8m-5, 8l-5): one on-demand TIME sample handed off after this
    /// call began, then ACCOUNT signed on the sample then published (21).
    pub(crate) async fn account_observation(
        &self,
        deadline: Instant,
    ) -> Result<ReadReceipt, TimeError> {
        let (_slot, deadline) = self.refused(ReadKind::Account, self.slot(deadline, 12))?;
        // Until its read begins, a dropped or failed observation is ACCOUNT's.
        let mut waiting = Metered(self, ReadKind::Account, Some("dropped"));
        let signed = self.handle.request_sample(deadline).await;
        let signed = signed.and_then(|_| self.handle.signing_time());
        waiting.2 = signed.as_ref().err().map(|error| metrics::outcome(Some(error)));
        drop(waiting);
        self.one(Read::Closed(SpotPreparedRead::Account), Some(&signed?), deadline)
            .await
    }

    /// The operated pair's rules (`exchangeInfo`, 20), unsigned.
    pub(crate) async fn rules(&self, deadline: Instant) -> Result<ReadReceipt, TimeError> {
        self.single(Read::Closed(SpotPreparedRead::ExchangeInfo), deadline).await
    }

    /// The operated pair's commission (20), signed.
    pub(crate) async fn commission(&self, deadline: Instant) -> Result<ReadReceipt, TimeError> {
        self.single(Read::Commission, deadline).await
    }

    /// The operated pair's one-minute kline at `start_ms` (2), unsigned; its
    /// status, 200 or not, and rows go to the caller (8p-4).
    pub(crate) async fn kline_at(
        &self,
        start_ms: u64,
        deadline: Instant,
    ) -> Result<ReadReceipt, TimeError> {
        self.single(Read::Closed(SpotPreparedRead::Kline { start_ms }), deadline).await
    }

    /// The EOF receipt check once more, for a caller that must still hold it
    /// after its own step (8k-3's post): the receipt's sample is still
    /// published at its generation, <= 60 s from its handoff, and fits. A busy
    /// clock state is a typed deferral (`Deferred`), never a wait.
    pub(crate) fn still_holds(&self, receipt: &ReadReceipt) -> Result<(), TimeError> {
        let ReadReceipt { generation, sample, send, handoff, .. } = receipt;
        self.receipt(*generation, *sample, send, *handoff, true).map(drop)
    }

    async fn single(&self, read: Read, deadline: Instant) -> Result<ReadReceipt, TimeError> {
        let (_slot, deadline) = self.refused(read.budget(), self.slot(deadline, 10))?;
        let signed = read.signed().then(|| self.handle.signing_time()).transpose();
        let signed = self.refused(read.budget(), signed)?;
        self.one(read, signed.as_ref(), deadline).await
    }

    /// A refusal before a read's own path (no slot, signing time or sample)
    /// is that read's outcome too (PLAN §6.11).
    fn refused<T>(&self, read: ReadKind, result: Result<T, TimeError>) -> Result<T, TimeError> {
        if let Err(error) = &result {
            drop(Metered(self, read, Some(metrics::outcome(Some(error)))));
        }
        result
    }

    /// One of the four slots, refused at once (`Busy`) when none is free,
    /// and the caller's deadline, never later than the composite's default.
    fn slot(
        &self,
        deadline: Instant,
        seconds: u64,
    ) -> Result<(OwnedSemaphorePermit, Instant), TimeError> {
        let slot = Arc::clone(&self.slots).try_acquire_owned().map_err(|_| TimeError::Busy)?;
        Ok((slot, deadline.min(Instant::now() + Duration::from_secs(seconds))))
    }

    /// One physical request, metered, bounded by the caller's deadline and
    /// 10 s from its preparation's start; composites pass their one deadline
    /// to every read, never a reset one.
    async fn one(
        &self,
        read: Read,
        signed: Option<&CommissionSigningTime>,
        deadline: Instant,
    ) -> Result<ReadReceipt, TimeError> {
        let bound = deadline.min(Instant::now() + REQUEST_BOUND);
        let reserved = self.reserve(read.budget(), bound);
        self.reserved(read, signed, bound, reserved).await
    }

    /// `read`'s reservation at its fixed weight, `now` and the deadline
    /// `bound` captured under the budget lock (non-blocking).
    fn reserve(&self, read: ReadKind, bound: Instant) -> Result<PendingRead, BudgetError> {
        let clock = &self.handle.clock;
        self.budget.reserve_read_now(read, || {
            let now = clock.capture().map_err(|_| BudgetError::Invalid)?;
            let left = bound.checked_duration_since(Instant::now()).ok_or(BudgetError::Expired)?;
            Ok((now, after(now, left).map_err(|_| BudgetError::Invalid)?))
        })
    }

    /// [`Self::one`]'s request on a reservation already taken (or refused).
    async fn reserved(
        &self,
        read: Read,
        signed: Option<&CommissionSigningTime>,
        bound: Instant,
        reserved: Result<PendingRead, BudgetError>,
    ) -> Result<ReadReceipt, TimeError> {
        let handle = &self.handle;
        let mut metered = Metered(self, read.budget(), Some("dropped"));
        let work = async {
            let mut pending = reserved.map_err(TimeError::Budget)?;
            // A fresh transport on a blocking permit, which the job keeps until
            // it ends; the job never signs or sends.
            let (permit, client) = (self.worker(bound).await?, Arc::clone(&self.client));
            let job = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                match read {
                    Read::Closed(read) => client.prepare_spot_read(read).map(Prepared::Read),
                    Read::Commission => robson_domain::Symbol::from_pair(SPOT_OPERATED_SYMBOL)
                        .map_err(|_| BinanceRestError::Refused("operated pair".into()))
                        .and_then(|pair| {
                            client.prepare_spot_commission(&pair).map(Prepared::Commission)
                        }),
                }
            });
            let prepared = timeout_at(bound, job).await.map_err(|_| TimeError::Expired)?;
            let prepared =
                prepared.map_err(|_| TimeError::Resource)?.map_err(|_| TimeError::Transport)?;
            let unbound = |_| TimeError::Transport;
            let mut sending: Sending = match (prepared, signed.map(|s| s.timestamp_ms())) {
                (Prepared::Read(read), Some(at)) => {
                    Box::pin(read.sign(at).map_err(unbound)?.send())
                },
                (Prepared::Read(read), None) => Box::pin(read.unsigned().map_err(unbound)?.send()),
                (Prepared::Commission(read), Some(at)) => {
                    Box::pin(read.sign(at).map_err(unbound)?.send())
                },
                (Prepared::Commission(_), None) => return Err(TimeError::Resource),
            };
            // Sent when first polled: that poll admitted under the clock guard
            // then the budget guard, which hand off the charge and observe the
            // head; later polls through the budget's send poll.
            let mut mark = None;
            let mut response = std::future::poll_fn(|cx| {
                let polled = if mark.is_none() {
                    self.admit(signed, &mut pending, bound, || sending.as_mut().poll(cx)).map(
                        |(admitted, result)| {
                            mark = Some(admitted);
                            result
                        },
                    )
                } else {
                    self.budget.poll_send(&handle.clock, handle.epoch, || sending.as_mut().poll(cx))
                };
                match polled {
                    Ok(result) => result.map(|answer| answer.map_err(|_| TimeError::Transport)),
                    // A later poll never waits for a busy budget: it polls again.
                    Err(BudgetError::Deferred) if mark.is_some() => {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    },
                    Err(error) => Poll::Ready(Err(TimeError::Budget(error))),
                }
            })
            .await?;
            let Mark { generation, sample, raw: send, handoff } =
                mark.ok_or(TimeError::Resource)?;
            let status = response.status().as_u16();
            if status != 200 && !matches!(read, Read::Closed(SpotPreparedRead::Kline { .. })) {
                return Err(TimeError::Venue);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| TimeError::Transport)? {
                if body.len().saturating_add(chunk.len()) > read.cap() {
                    return Err(TimeError::Payload);
                }
                body.try_reserve_exact(chunk.len()).map_err(|_| TimeError::Resource)?;
                body.extend_from_slice(&chunk);
            }
            let (receive, sample) =
                self.receipt(generation, sample, &send, handoff, signed.is_some())?;
            // Strict decode on a blocking permit: the read's own decoder within
            // its cap; the commission for the operated pair only.
            let permit = self.worker(bound).await?;
            let body = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                match read {
                    Read::Closed(read) => read.decode(&body).map(ReadBody::Closed).ok(),
                    Read::Commission => serde_json::from_slice::<SpotCommission>(&body)
                        .ok()
                        .filter(|commission| commission.symbol == SPOT_OPERATED_SYMBOL)
                        .map(ReadBody::Commission),
                }
            });
            let body = body.await.map_err(|_| TimeError::Resource)?.ok_or(TimeError::Payload)?;
            // A decode joined past the bound fails the read, as `jobs::complete`
            // does: the outer timeout yields to work that is ready.
            if Instant::now() >= bound {
                return Err(TimeError::Expired);
            }
            Ok(ReadReceipt {
                read,
                generation,
                sample,
                send,
                handoff,
                receive,
                status,
                body,
            })
        };
        let result = tokio::select! {
            biased;
            _ = handle.cancel.cancelled() => Err(TimeError::Closing),
            done = timeout_at(bound, work) => done.unwrap_or(Err(TimeError::Expired)),
        };
        metered.2 = Some(metrics::outcome(result.as_ref().err()));
        result
    }

    /// One of the reads' two blocking permits, waited for until `bound`.
    async fn worker(&self, bound: Instant) -> Result<OwnedSemaphorePermit, TimeError> {
        let acquired = timeout_at(bound, Arc::clone(&self.workers).acquire_owned()).await;
        acquired.map_err(|_| TimeError::Expired)?.map_err(|_| TimeError::Closing)
    }

    /// A signed read's clock admission is the commission's (its sample and
    /// generation still published, ≤ 60 s from signing, the signed window);
    /// an unsigned read's is a fresh capture. Either names the published
    /// sample only when it fits the poll's mark.
    fn admit(
        &self,
        signed: Option<&CommissionSigningTime>,
        pending: &mut PendingRead,
        bound: Instant,
        poll: impl FnOnce() -> Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(Mark, Poll<reqwest::Result<reqwest::Response>>), BudgetError> {
        let handle = &self.handle;
        let mut state = handle.state.try_lock().map_err(|_| BudgetError::Deferred)?;
        let clock_admission = || {
            let (handoff, raw) = match signed {
                Some(signed) => {
                    let claim = signed.capture.end_tick().map_err(|_| BudgetError::Invalid)?;
                    handle.admit_signed(&mut state, signed, claim, bound)?
                },
                None => {
                    let raw = state.capture_native(&handle.clock, handle.epoch);
                    let raw = raw.map_err(|_| BudgetError::Invalid)?;
                    (raw.end_tick().map_err(|_| BudgetError::Invalid)?, raw)
                },
            };
            let fits = |s: &&Published| s.arithmetic.fits_capture(&raw) == Ok(true);
            let sample = state.published.as_ref().filter(fits).map(|s| (s.id, s.u2));
            Ok((handoff, Mark {
                generation: state.generation,
                sample,
                raw,
                handoff,
            }))
        };
        let (_, mark, result) = self.budget.poll_read_first(
            pending,
            &handle.clock,
            handle.epoch,
            clock_admission,
            poll,
        )?;
        Ok((mark, result))
    }

    /// At EOF, before any decode: the sample the first poll was admitted on
    /// is still published at the same generation, ≤ 60 s from the handoff,
    /// fits a fresh capture and agrees with the mark's wall. A signed read
    /// fails without it; an unsigned one keeps no sample.
    fn receipt(
        &self,
        generation: u64,
        sample: Option<(Uuid, i128)>,
        mark: &RawClockCapture,
        handoff: TickCapture,
        signed: bool,
    ) -> Result<(RawClockCapture, Option<(Uuid, i128)>), TimeError> {
        let handle = &self.handle;
        let mut state =
            handle.state.try_lock().map_err(|_| TimeError::Budget(BudgetError::Deferred))?;
        if state.closing || handle.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        let now = state.capture_native(&handle.clock, handle.epoch)?;
        let elapsed = now.end_tick().and_then(|end| end.elapsed_since(&handoff));
        let holds = sample.is_some_and(|(id, _)| {
            state.generation == generation
                && elapsed.is_ok_and(|elapsed| elapsed <= RECEIPT_LIMIT)
                && mark.wall_agrees(&now) == Ok(true)
                && state
                    .published
                    .as_ref()
                    .is_some_and(|s| s.id == id && s.arithmetic.fits_capture(&now) == Ok(true))
        });
        match (holds, signed) {
            (true, _) => Ok((now, sample)),
            (false, false) => Ok((now, None)),
            (false, true) => Err(TimeError::Native),
        }
    }
}

/// 8p-4's boundary mark on this owner (5a): the closed kline's reservation,
/// taken first under the budget lock with the 10 s bound, for the single read
/// path to send on. Dropped unsent, its charge is refunded.
pub(crate) struct KlineReservation {
    pending: PendingRead,
    bound: Instant,
}

impl SpotReads {
    /// One unsigned kline read at `start_ms` on `reserved` (no second
    /// reservation), on a slot, within the reservation's bound: its HTTP
    /// status and body text.
    async fn kline_on(
        &self,
        reserved: KlineReservation,
        start_ms: u64,
    ) -> Result<(u16, String), crate::boundary::Failure> {
        let read = Read::Closed(SpotPreparedRead::Kline { start_ms });
        let slot = self.refused(ReadKind::Kline, self.slot(reserved.bound, 10));
        let (_slot, bound) = slot.map_err(|e| format!("{e:?}"))?;
        let receipt = self.reserved(read, None, bound, Ok(reserved.pending)).await;
        let receipt = receipt.map_err(|e| format!("{e:?}"))?;
        match receipt.body {
            ReadBody::Closed(SpotReadBody::Kline(rows)) => Ok((receipt.status, rows)),
            _ => Err("the kline read answered another body".into()),
        }
    }
}

/// The binding of `boundary::mark::MarkVenue` (8p-4's seam, 5a): no
/// production caller until 8p-5's completion calls `mark::kline_at`.
impl crate::boundary::mark::MarkVenue for SpotReads {
    type Reserved = KlineReservation;

    /// Only the kline's weight (2); a busy or paused budget refuses at once,
    /// nothing charged.
    fn try_reserve(&self, weight: u32) -> Result<KlineReservation, crate::boundary::Failure> {
        if weight != ReadKind::Kline.weight() {
            return Err(format!("the mark reserves the kline's weight, not {weight}").into());
        }
        let bound = Instant::now() + REQUEST_BOUND;
        let pending = self.reserve(ReadKind::Kline, bound).map_err(TimeError::Budget);
        let pending = self.refused(ReadKind::Kline, pending).map_err(|e| format!("{e:?}"))?;
        Ok(KlineReservation { pending, bound })
    }

    fn read_kline(
        &self,
        reserved: KlineReservation,
        start_ms: u64,
    ) -> impl Future<Output = Result<(u16, String), crate::boundary::Failure>> + Send {
        self.kline_on(reserved, start_ms)
    }
}

#[cfg(all(test, feature = "crash-points"))]
#[path = "reads_tests.rs"]
mod tests;
