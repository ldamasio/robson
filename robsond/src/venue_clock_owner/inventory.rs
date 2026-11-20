//! Test-only C8 ACCOUNT read under the native ClockOwner and its one
//! SharedBudget, over the synthetic loopback transport. It yields the
//! admitted sample identity only: no balances, ledger or venue authority.
use std::{future::Future, task::Poll};

use robson_connectors::inventory_loopback::{
    self, InventoryBody, InventoryRead, PreparedOwnedOrder, PreparedRead, SignedRead,
    LOOPBACK_BODY_LIMIT, LOOPBACK_UID,
};
use robson_exec::venue::OrderObservation;

use super::{commission::CommissionSigningTime, *};
use crate::venue_budget::state::{PendingAccount, PendingOwnedOrder};

const RECEIPT_LIMIT: Duration = Duration::from_secs(60);

pub(super) type AccountPreparation = JoinHandle<Result<PreparedRead, BinanceRestError>>;
pub(super) type OwnedOrderPreparation = JoinHandle<Result<PreparedOwnedOrder, BinanceRestError>>;

enum Reservation<'a> {
    Inventory(&'a mut PendingAccount),
    OwnedOrder(&'a mut PendingOwnedOrder),
}

struct ReadBody {
    status: u16,
    body: Vec<u8>,
    mark: AccountMark,
    receive: RawClockCapture,
}

pub(super) struct OwnedOrderReceipt {
    pub(super) mark: AccountMark,
    pub(super) receive: RawClockCapture,
    pub(super) observation: OrderObservation,
}

/// What the actual locked first-poll admission saw: the published sample and
/// generation it was judged against and the native poll mark, as produced.
pub(super) struct AccountMark {
    pub(super) generation: u64,
    pub(super) sample: Uuid,
    pub(super) raw: RawClockCapture,
    pub(super) handoff: TickCapture,
}

/// One admitted read: its endpoint, the sample and generation the locked
/// first poll judged it against, its send and EOF brackets, and the typed
/// body decoded only after the EOF check.
pub(super) struct AccountReceipt {
    pub(super) read: InventoryRead,
    pub(super) generation: u64,
    pub(super) sample: Uuid,
    pub(super) send: RawClockCapture,
    /// The native handoff mark, as the locked first poll produced it.
    pub(super) handoff: TickCapture,
    pub(super) receive: RawClockCapture,
    pub(super) body: InventoryBody,
}

/// The test-only composite: ACCOUNT, OPEN_ORDERS, OPEN_ORDER_LIST, all
/// complete under one sample and generation. No financial meaning.
pub(super) struct InventoryTriple {
    pub(super) receipts: [AccountReceipt; 3],
}

impl ClockOwner {
    pub(super) fn reserve_account(&self, deadline: Instant) -> Result<PendingAccount, TimeError> {
        self.reserve_inventory(InventoryRead::Account, deadline)
    }

    pub(super) fn reserve_inventory(
        &self,
        read: InventoryRead,
        deadline: Instant,
    ) -> Result<PendingAccount, TimeError> {
        let now = self.clock.capture().map_err(|_| TimeError::Native)?;
        let remaining =
            deadline.checked_duration_since(Instant::now()).ok_or(TimeError::Expired)?;
        self.budget
            .try_reserve_inventory(read, now, after(now, remaining)?)
            .map_err(TimeError::Budget)
    }

    /// One planned blocking job in the caller's slot, on the owner's one
    /// worker permit. Timeout and cancellation leave it in the slot for the
    /// caller's `jobs::retire`.
    pub(super) async fn prepare_account(
        &self,
        slot: &mut Option<AccountPreparation>,
        deadline: Instant,
    ) -> Result<PreparedRead, TimeError> {
        self.prepare_inventory(InventoryRead::Account, slot, deadline).await
    }

    pub(super) async fn prepare_inventory(
        &self,
        read: InventoryRead,
        slot: &mut Option<AccountPreparation>,
        deadline: Instant,
    ) -> Result<PreparedRead, TimeError> {
        let prepared = self
            .prepare_loopback(slot, deadline, move || inventory_loopback::prepare(read))
            .await?;
        if prepared.read() != read {
            return Err(TimeError::Venue);
        }
        Ok(prepared)
    }

    async fn prepare_loopback<T: Send + 'static>(
        &self,
        slot: &mut Option<JoinHandle<Result<T, BinanceRestError>>>,
        deadline: Instant,
        prepare: impl FnOnce() -> Result<T, BinanceRestError> + Send + 'static,
    ) -> Result<T, TimeError> {
        if slot.is_some() {
            return Err(TimeError::Resource);
        }
        if self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        if Instant::now() >= deadline {
            return Err(TimeError::Expired);
        }
        let permit =
            Arc::clone(&self.workers).try_acquire_owned().map_err(|_| TimeError::Resource)?;
        *slot = Some(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            prepare()
        }));
        jobs::complete(slot, deadline, &self.cancel, |_| TimeError::Transport).await
    }

    pub(super) async fn retire_account(slot: &mut Option<AccountPreparation>) -> bool {
        jobs::retire(slot).await
    }

    /// The commission first-poll contract, shared: same clock-then-budget
    /// guards, same signed admission, its mark returned rather than rebuilt.
    fn poll_read_first(
        &self,
        signed: &CommissionSigningTime,
        pending: &mut Reservation<'_>,
        claim: TickCapture,
        deadline: Instant,
        poll: impl FnOnce() -> Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(AccountMark, Poll<reqwest::Result<reqwest::Response>>), BudgetError> {
        if !Arc::ptr_eq(&self.state, &signed.state) {
            return Err(BudgetError::Invalid);
        }
        let mut state = self.state.try_lock().map_err(|_| BudgetError::Deferred)?;
        let admit = || {
            let (handoff, raw) = self.admit_signed(&mut state, signed, claim, deadline)?;
            let sample = state.published.as_ref().map(|s| s.id).ok_or(BudgetError::Deferred)?;
            Ok((handoff, (state.generation, sample, raw)))
        };
        let (handoff, (generation, sample, raw), result) = match pending {
            Reservation::Inventory(reservation) => {
                self.budget
                    .poll_account_first(reservation, &self.clock, self.epoch, admit, poll)
            },
            Reservation::OwnedOrder(reservation) => self.budget.poll_owned_order_first(
                reservation,
                &self.clock,
                self.epoch,
                admit,
                poll,
            ),
        }?;
        Ok((AccountMark { generation, sample, raw, handoff }, result))
    }

    /// At body EOF, before any decode: the same publication and generation
    /// still hold, the current arithmetic fits a fresh native capture, and
    /// wall and tick agree with the admitted mark. No replacement is borrowed.
    pub(super) fn account_receipt(&self, mark: &AccountMark) -> Result<RawClockCapture, TimeError> {
        let mut state =
            self.state.try_lock().map_err(|_| TimeError::Budget(BudgetError::Deferred))?;
        if state.closing || self.cancel.is_cancelled() {
            return Err(TimeError::Closing);
        }
        if state.generation != mark.generation || mark.raw.epoch != self.epoch {
            return Err(TimeError::Native);
        }
        let now = state.capture_native(&self.clock, self.epoch)?;
        let sample = state
            .published
            .as_ref()
            .filter(|s| s.id == mark.sample)
            .ok_or(TimeError::Native)?;
        let elapsed = now
            .end_tick()
            .and_then(|end| end.elapsed_since(&mark.handoff))
            .map_err(|_| TimeError::Native)?;
        if elapsed > RECEIPT_LIMIT
            || sample.arithmetic.fits_capture(&now) != Ok(true)
            || mark.raw.wall_agrees(&now) != Ok(true)
        {
            return Err(TimeError::Native);
        }
        Ok(now)
    }

    /// One admitted send with the original capture and a bounded body to EOF.
    pub(super) async fn read_account(
        &self,
        signed: &CommissionSigningTime,
        pending: &mut PendingAccount,
        claim: TickCapture,
        prepared: PreparedRead,
        port: u16,
        deadline: Instant,
    ) -> Result<AccountReceipt, TimeError> {
        self.check_read_deadline(deadline)?;
        let read = pending.read();
        if prepared.read() != read {
            return Err(TimeError::Venue);
        }
        let request =
            prepared.sign(port, signed.timestamp_ms()).map_err(|_| TimeError::Transport)?;
        let ReadBody { body, mark, receive, .. } = self
            .receive_loopback(signed, Reservation::Inventory(pending), claim, request, deadline)
            .await?;
        let body = inventory_loopback::decode(read, &body).map_err(|_| TimeError::Payload)?;
        if body.read() != read {
            return Err(TimeError::Venue);
        }
        if let InventoryBody::Account(account) = &body {
            if account.uid != LOOPBACK_UID {
                return Err(TimeError::Venue);
            }
        }
        self.check_read_deadline(deadline)?;
        Ok(AccountReceipt {
            read,
            generation: mark.generation,
            sample: mark.sample,
            send: mark.raw,
            handoff: mark.handoff,
            receive,
            body,
        })
    }

    pub(super) async fn read_owned_order(
        &self,
        signed: &CommissionSigningTime,
        claim: TickCapture,
        slot: &mut Option<OwnedOrderPreparation>,
        port: u16,
        deadline: Instant,
    ) -> Result<OwnedOrderReceipt, TimeError> {
        let now = self.clock.capture().map_err(|_| TimeError::Native)?;
        let remaining =
            deadline.checked_duration_since(Instant::now()).ok_or(TimeError::Expired)?;
        let mut pending = self
            .budget
            .try_reserve_owned_order(now, after(now, remaining)?)
            .map_err(TimeError::Budget)?;
        let prepared = self
            .prepare_loopback(slot, deadline, inventory_loopback::prepare_owned_order)
            .await?;
        let request =
            prepared.sign(port, signed.timestamp_ms()).map_err(|_| TimeError::Transport)?;
        let ReadBody { status, body, mark, receive } = self
            .receive_loopback(
                signed,
                Reservation::OwnedOrder(&mut pending),
                claim,
                request,
                deadline,
            )
            .await?;
        let observation = inventory_loopback::decode_owned_order(status, &body)
            .map_err(|_| TimeError::Payload)?;
        self.check_read_deadline(deadline)?;
        Ok(OwnedOrderReceipt { mark, receive, observation })
    }

    fn check_read_deadline(&self, deadline: Instant) -> Result<(), TimeError> {
        if self.cancel.is_cancelled() {
            Err(TimeError::Closing)
        } else if Instant::now() >= deadline {
            Err(TimeError::Expired)
        } else {
            Ok(())
        }
    }

    async fn receive_loopback(
        &self,
        signed: &CommissionSigningTime,
        mut pending: Reservation<'_>,
        claim: TickCapture,
        request: SignedRead,
        deadline: Instant,
    ) -> Result<ReadBody, TimeError> {
        let work = async {
            self.check_read_deadline(deadline)?;
            let mut sending = std::pin::pin!(request.execute());
            let mut mark = None;
            let mut response = std::future::poll_fn(|cx| {
                if Instant::now() >= deadline {
                    return Poll::Ready(Err(TimeError::Expired));
                }
                let polled = if mark.is_none() {
                    self.poll_read_first(signed, &mut pending, claim, deadline, || {
                        sending.as_mut().poll(cx)
                    })
                    .map(|(admitted, result)| {
                        mark = Some(admitted);
                        result
                    })
                } else {
                    self.poll_commission_again(|| sending.as_mut().poll(cx))
                };
                match polled {
                    Ok(result) => result.map(|outcome| outcome.map_err(|_| TimeError::Transport)),
                    Err(error) => Poll::Ready(Err(TimeError::Budget(error))),
                }
            })
            .await?;
            let mark = mark.ok_or(TimeError::Resource)?;
            let status = response.status().as_u16();
            if matches!(&pending, Reservation::Inventory(_)) && status != 200 {
                return Err(TimeError::Venue);
            }
            let mut body = Vec::new();
            loop {
                if Instant::now() >= deadline {
                    return Err(TimeError::Expired);
                }
                let Some(chunk) = response.chunk().await.map_err(|_| TimeError::Transport)? else {
                    break;
                };
                if body.len().saturating_add(chunk.len()) > LOOPBACK_BODY_LIMIT {
                    return Err(TimeError::Resource);
                }
                body.try_reserve_exact(chunk.len()).map_err(|_| TimeError::Resource)?;
                body.extend_from_slice(&chunk);
            }
            let receive = self.account_receipt(&mark)?;
            if Instant::now() >= deadline {
                return Err(TimeError::Expired);
            }
            Ok(ReadBody { status, body, mark, receive })
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(TimeError::Closing),
            answer = timeout_at(deadline, work) => answer.map_err(|_| TimeError::Expired)?,
        }
    }

    /// Test-only composite: ACCOUNT (UID checked before any suffix), then
    /// OPEN_ORDERS, then OPEN_ORDER_LIST, one request each under the caller's
    /// one deadline and signed sample. The first failure skips the unattempted
    /// suffix, without retry; a successful prefix is never returned. The slot
    /// keeps any in-flight preparation for the caller's `retire_account`.
    pub(super) async fn read_inventory_triple(
        &self,
        signed: &CommissionSigningTime,
        claim: TickCapture,
        slot: &mut Option<AccountPreparation>,
        port: u16,
        deadline: Instant,
    ) -> Result<InventoryTriple, TimeError> {
        let mut receipts = Vec::with_capacity(InventoryRead::ALL.len());
        for read in InventoryRead::ALL {
            let mut pending = self.reserve_inventory(read, deadline)?;
            let prepared = self.prepare_inventory(read, slot, deadline).await?;
            let receipt =
                self.read_account(signed, &mut pending, claim, prepared, port, deadline).await?;
            receipts.push(receipt);
        }
        let receipts: [AccountReceipt; 3] = receipts.try_into().map_err(|_| TimeError::Resource)?;
        Self::check_triple(&receipts)?;
        // Revalidated against the scan anchor before it is called complete.
        self.account_receipt(&AccountMark {
            generation: receipts[0].generation,
            sample: receipts[0].sample,
            raw: receipts[0].send,
            handoff: receipts[0].handoff,
        })?;
        Ok(InventoryTriple { receipts })
    }

    /// Exact endpoints in order, one sample and generation, and distinct raw
    /// chronology: each send bracket ends before its EOF bracket begins, and
    /// each EOF before the next send.
    pub(super) fn check_triple(receipts: &[AccountReceipt; 3]) -> Result<(), TimeError> {
        let first = &receipts[0];
        let mut previous: Option<&RawClockCapture> = None;
        for (receipt, read) in receipts.iter().zip(InventoryRead::ALL) {
            let same = receipt.read == read
                && receipt.body.read() == read
                && receipt.sample == first.sample
                && receipt.generation == first.generation
                && receipt.send.epoch == first.send.epoch
                && receipt.receive.epoch == first.send.epoch
                && receipt.send.domain == first.send.domain
                && receipt.receive.domain == first.send.domain
                && receipt.send.after_ns < receipt.receive.before_ns
                && previous.is_none_or(|p| p.after_ns < receipt.send.before_ns);
            if !same {
                return Err(TimeError::Native);
            }
            previous = Some(&receipt.receive);
        }
        match &first.body {
            InventoryBody::Account(account) if account.uid == LOOPBACK_UID => Ok(()),
            _ => Err(TimeError::Venue),
        }
    }
}
