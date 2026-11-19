//! Test-only C8 ACCOUNT read under the native ClockOwner and its one
//! SharedBudget, over the synthetic loopback transport. It yields the
//! admitted sample identity only: no balances, ledger or venue authority.
use std::{future::Future, task::Poll};

use robson_connectors::inventory_loopback::{
    self, InventoryRead, PreparedRead, LOOPBACK_BODY_LIMIT, LOOPBACK_UID,
};

use super::{commission::CommissionSigningTime, *};
use crate::venue_budget::state::PendingAccount;

const RECEIPT_LIMIT: Duration = Duration::from_secs(60);

pub(super) type AccountPreparation = JoinHandle<Result<PreparedRead, BinanceRestError>>;

/// What the actual locked first-poll admission saw: the published sample and
/// generation it was judged against and the native poll mark, as produced.
pub(super) struct AccountMark {
    pub(super) generation: u64,
    pub(super) sample: Uuid,
    pub(super) raw: RawClockCapture,
    pub(super) handoff: TickCapture,
}

pub(super) struct AccountReceipt {
    pub(super) sample: Uuid,
    pub(super) send: RawClockCapture,
    pub(super) receive: RawClockCapture,
}

impl ClockOwner {
    pub(super) fn reserve_account(&self, deadline: Instant) -> Result<PendingAccount, TimeError> {
        let now = self.clock.capture().map_err(|_| TimeError::Native)?;
        let remaining =
            deadline.checked_duration_since(Instant::now()).ok_or(TimeError::Expired)?;
        self.budget
            .try_reserve_account(now, after(now, remaining)?)
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
            inventory_loopback::prepare(InventoryRead::Account)
        }));
        jobs::complete(slot, deadline, &self.cancel, |_| TimeError::Transport).await
    }

    pub(super) async fn retire_account(slot: &mut Option<AccountPreparation>) -> bool {
        jobs::retire(slot).await
    }

    /// The commission first-poll contract, shared: same clock-then-budget
    /// guards, same signed admission, its mark returned rather than rebuilt.
    pub(super) fn poll_account_first(
        &self,
        signed: &CommissionSigningTime,
        pending: &mut PendingAccount,
        claim: TickCapture,
        deadline: Instant,
        poll: impl FnOnce() -> Poll<reqwest::Result<reqwest::Response>>,
    ) -> Result<(AccountMark, Poll<reqwest::Result<reqwest::Response>>), BudgetError> {
        if !Arc::ptr_eq(&self.state, &signed.state) {
            return Err(BudgetError::Invalid);
        }
        let mut state = self.state.try_lock().map_err(|_| BudgetError::Deferred)?;
        let (handoff, (generation, sample, raw), result) = self.budget.poll_account_first(
            pending,
            &self.clock,
            self.epoch,
            || {
                let (h, raw) = self.admit_signed(&mut state, signed, claim, deadline)?;
                let sample = state.published.as_ref().map(|s| s.id).ok_or(BudgetError::Deferred)?;
                Ok((h, (state.generation, sample, raw)))
            },
            poll,
        )?;
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
        let work = async {
            if Instant::now() >= deadline {
                return Err(TimeError::Expired);
            }
            if prepared.read() != InventoryRead::Account {
                return Err(TimeError::Venue);
            }
            let request =
                prepared.sign(port, signed.timestamp_ms()).map_err(|_| TimeError::Transport)?;
            let mut sending = std::pin::pin!(request.execute());
            let mut mark = None;
            let mut response = std::future::poll_fn(|cx| {
                if Instant::now() >= deadline {
                    return Poll::Ready(Err(TimeError::Expired));
                }
                let polled = if mark.is_none() {
                    self.poll_account_first(signed, pending, claim, deadline, || {
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
            if response.status() != reqwest::StatusCode::OK {
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
            let uid = inventory_loopback::account_uid(&body).map_err(|_| TimeError::Payload)?;
            if uid != LOOPBACK_UID {
                return Err(TimeError::Venue);
            }
            if Instant::now() >= deadline || self.cancel.is_cancelled() {
                return Err(if self.cancel.is_cancelled() {
                    TimeError::Closing
                } else {
                    TimeError::Expired
                });
            }
            Ok(AccountReceipt {
                sample: mark.sample,
                send: mark.raw,
                receive,
            })
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(TimeError::Closing),
            answer = timeout_at(deadline, work) => answer.map_err(|_| TimeError::Expired)?,
        }
    }
}
