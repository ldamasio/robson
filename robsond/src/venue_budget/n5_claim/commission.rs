//! Owned N5 attempt for the controlled loopback composition. The production
//! startup factory has no constructor for this owner.

use std::{future::poll_fn, task::Poll, time::Duration};

use chrono::{DateTime, Utc};
use robson_connectors::binance::spot::PreparedSpotCommission;
use robson_exec::balance_costs::CostIdentity;
use sha2::{Digest, Sha256};
use tokio::time::{timeout_at, Instant};

use crate::{
    venue_budget::state::PendingCommission,
    venue_clock_http::TimeError,
    venue_clock_owner::{ClockOwner, CommissionPreparation},
};

use super::{
    claim, confirm_once,
    response::{prepare_response, MAX_RESPONSE_BYTES},
    result::{confirm_result_once, publish_result, PreparedSnapshot, Publication, ResultRequest},
    BudgetError, ClaimIdentity, ClaimObservation, ClaimOutcome, ClaimRequest, ClaimResult,
    FrequencyKey, PriorClaim, Trace,
};

const REFRESH_LIMIT: Duration = Duration::from_secs(15);
const WIRE_LIMIT: Duration = Duration::from_secs(10);

struct CommissionOwner<'a> {
    clock: &'a ClockOwner,
    account: CostIdentity,
    signer_binding: uuid::Uuid,
    preparation: Option<CommissionPreparation>,
}

struct OriginalAttempt {
    key: FrequencyKey,
    claim: ClaimObservation,
    prepared: PreparedSpotCommission,
}

#[derive(Debug, PartialEq, Eq)]
enum RefreshOutcome {
    ClaimRefused,
    ClaimUnconfirmed,
    Published,
    ResultRefused,
    ResultUnconfirmed,
}

#[derive(Debug)]
enum RefreshError {
    Time(TimeError),
    Store(BudgetError),
}

impl From<TimeError> for RefreshError {
    fn from(error: TimeError) -> Self {
        Self::Time(error)
    }
}

impl From<BudgetError> for RefreshError {
    fn from(error: BudgetError) -> Self {
        Self::Store(error)
    }
}

fn content_digest(account: &CostIdentity, signer_binding: uuid::Uuid, symbol: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"Robson N5 commission descriptor v1");
    for field in [
        b"GET /api/v3/account/commission".as_slice(),
        b"recvWindow=5000(default)".as_slice(),
        account.tenant().as_bytes(),
        account.venue().as_str().as_bytes(),
        account.scope().as_bytes(),
        &account.uid().to_be_bytes(),
        symbol.as_bytes(),
        signer_binding.as_bytes(),
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    hash.finalize().into()
}

impl CommissionOwner<'_> {
    async fn refresh_registered(
        &mut self,
        pool: &sqlx::PgPool,
        key: FrequencyKey,
        prior: PriorClaim,
        parent_deadline: Instant,
        trace: &Trace,
    ) -> Result<RefreshOutcome, RefreshError> {
        if self.signer_binding.is_nil() || self.account.symbol().as_pair() != key.symbol {
            return Err(BudgetError::Invalid.into());
        }
        let deadline = parent_deadline.min(Instant::now() + REFRESH_LIMIT);
        let prepared = self
            .clock
            .prepare_commission(&self.account, &mut self.preparation, deadline)
            .await?;
        let pending = self.clock.reserve_commission(deadline)?;
        let captured = self.clock.commission_capture()?;
        let audit_at = DateTime::<Utc>::from_timestamp(
            captured.wall_seconds,
            captured.wall_nanoseconds / 1_000 * 1_000,
        )
        .ok_or(BudgetError::Invalid)?;
        let next = ClaimIdentity {
            claim_id: uuid::Uuid::now_v7(),
            owner: self.clock.commission_epoch(),
            digest: content_digest(&self.account, self.signer_binding, &key.symbol),
            captured: captured.end_tick().map_err(|_| BudgetError::Invalid)?,
            audit_at,
        };
        let request = ClaimRequest { key: key.clone(), prior, next };
        let original = match claim(pool, request, deadline, trace).await? {
            ClaimOutcome::Stored(original) => original,
            ClaimOutcome::Uncertain(confirmation) => {
                let Some(original) = confirm_once(pool, confirmation, trace).await? else {
                    return Ok(RefreshOutcome::ClaimUnconfirmed);
                };
                original
            },
            ClaimOutcome::Refused => return Ok(RefreshOutcome::ClaimRefused),
        };
        let attempt = OriginalAttempt { key, claim: original, prepared };
        let wire_deadline = deadline.min(Instant::now() + WIRE_LIMIT);
        let result = attempt.observe(self.clock, &self.account, pending, wire_deadline).await;
        match publish_result(pool, result, deadline, trace).await? {
            Publication::Published | Publication::Replayed => Ok(RefreshOutcome::Published),
            Publication::Refused => Ok(RefreshOutcome::ResultRefused),
            Publication::Uncertain(confirmation) => {
                if confirm_result_once(pool, confirmation, trace).await? {
                    Ok(RefreshOutcome::Published)
                } else {
                    Ok(RefreshOutcome::ResultUnconfirmed)
                }
            },
        }
    }
}

struct HandoffReceipt {
    claim: ClaimObservation,
    at: robson_exec::venue_clock::TickCapture,
}

impl OriginalAttempt {
    async fn observe(
        self,
        clock: &ClockOwner,
        account: &CostIdentity,
        mut pending: PendingCommission,
        deadline: Instant,
    ) -> ResultRequest {
        use std::future::Future;

        let OriginalAttempt { key, claim, prepared } = self;
        let mut result = ResultRequest {
            key,
            claim,
            handoff: None,
            conclusion: ClaimResult::NoSend,
            snapshot: None,
        };
        if clock.commission_cancelled().is_cancelled() || Instant::now() >= deadline {
            return result;
        }
        let Ok(signing) = clock.commission_signing_time() else {
            return result;
        };
        let Ok(signed) = prepared.sign(signing.timestamp_ms()) else {
            return result;
        };
        let mut sending = Box::pin(signed.send());
        let mut receipt = None;
        let request = async {
            let answer = poll_fn(|cx| {
                if clock.commission_cancelled().is_cancelled() || Instant::now() >= deadline {
                    return Poll::Ready(Err(ClaimResult::Unknown));
                }
                let next = if receipt.is_none() {
                    clock
                        .poll_commission_first(
                            &signing,
                            &mut pending,
                            result.claim.identity.captured,
                            deadline,
                            || sending.as_mut().poll(cx),
                        )
                        .map(|(at, next)| {
                            receipt = Some(HandoffReceipt { claim: result.claim.clone(), at });
                            next
                        })
                } else {
                    clock.poll_commission_again(|| sending.as_mut().poll(cx))
                };
                match next {
                    Ok(next) => {
                        next.map(|response| response.map_err(|_| ClaimResult::TransportFailure))
                    },
                    Err(_) => Poll::Ready(Err(ClaimResult::Unknown)),
                }
            })
            .await?;
            let observed = receipt.as_ref().ok_or(ClaimResult::Unknown)?;
            receive_response(answer, clock, account, observed).await
        };
        let conclusion = tokio::select! {
            biased;
            _ = clock.commission_cancelled().cancelled() => Err(ClaimResult::Unknown),
            answer = timeout_at(deadline, request) => answer.unwrap_or(Err(ClaimResult::Unknown)),
        };
        if let Some(receipt) = receipt {
            result.handoff = Some(receipt.at);
            if receipt.claim == result.claim {
                match conclusion {
                    Ok(snapshot) => {
                        result.snapshot = Some(snapshot);
                        result.conclusion = ClaimResult::Success;
                    },
                    Err(conclusion) => result.conclusion = conclusion,
                }
            } else {
                result.conclusion = ClaimResult::Unknown;
            }
        }
        result
    }
}

async fn receive_response(
    mut answer: reqwest::Response,
    clock: &ClockOwner,
    account: &CostIdentity,
    receipt: &HandoffReceipt,
) -> Result<PreparedSnapshot, ClaimResult> {
    if !answer.status().is_success() {
        // A response restriction is installed when the head first returns.
        // This scope requires a successful exact body to qualify any fees.
        return Err(ClaimResult::Unknown);
    }
    let mut body = Vec::new();
    body.try_reserve_exact(MAX_RESPONSE_BYTES)
        .map_err(|_| ClaimResult::InvalidPayload)?;
    while let Some(chunk) = answer.chunk().await.map_err(|_| ClaimResult::TransportFailure)? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(ClaimResult::InvalidPayload);
        }
        body.extend_from_slice(&chunk);
    }
    let original = clock.commission_capture().map_err(|_| ClaimResult::Unknown)?;
    if original.epoch != receipt.claim.identity.owner {
        return Err(ClaimResult::Unknown);
    }
    prepare_response(&body, original, account, receipt.at)
}

#[cfg(all(test, feature = "crash-points"))]
#[path = "commission_tests.rs"]
mod tests;
