//! 8k-3a (PLAN-r2 §6.2, §6.5, §6.7): one attempt's capture and its ACCOUNT
//! reading, for the balance scan (8k-3b) and 8l-5's O1/O2. The marks are the
//! gate's invalidity marks, read before the pre statement and again after
//! the post (F4); the interval, `u`, sample and epoch come from the ACCOUNT
//! receipt only. Inert: no production caller yet.
use std::time::Instant;

use robson_connectors::binance::spot::SpotAccount;
use robson_exec::{
    clock_records::RawClockCapture,
    gate::{InvalidityMarks, OperationGate},
    venue_ledger::exact_add,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgConnectOptions;
use uuid::Uuid;

use super::{
    account_u, qualify, snapshot::read_non_trading, AttemptOutcome, BaseVerdict, ClockOutcome,
    IncompleteReason, ObservationInterval, PostOutcome, PreRefusal, PreSnapshot,
    QualificationMarks, ScanScope, UnqualifiedReason,
};
use crate::venue_ledger_store::cash::writer::CashAttempt;

/// C8 steps 1 and 2 of one attempt: the marks, then `v₀`.
#[derive(Debug, Clone)]
pub(crate) struct Capture {
    pub(crate) marks: InvalidityMarks,
    /// `v₀` and the ledger rows: none for the non-trading scope.
    pub(crate) pre: PreSnapshot,
}

impl Capture {
    /// The marks, read under the gate mutex before any I/O, then the
    /// scope's pre statement. A refusal ends the attempt.
    pub(crate) async fn before(
        gate: &OperationGate,
        options: &PgConnectOptions,
        scope: &ScanScope<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<Self, UnqualifiedReason> {
        let marks = gate.invalidity_marks();
        let version = read_non_trading(options, scope, None, deadline).await?;
        Ok(Self {
            marks,
            pre: PreSnapshot { version, ..PreSnapshot::default() },
        })
    }

    /// Steps 6 to 9: the post statement with `v₀`; then `revalidate`, the
    /// ACCOUNT receipt's clock check after the post, which only refuses and
    /// never validates retroactively; then the marks again and [`qualify`].
    pub(crate) async fn still(
        &self,
        gate: &OperationGate,
        options: &PgConnectOptions,
        scope: &ScanScope<'_>,
        deadline: tokio::time::Instant,
        revalidate: impl FnOnce() -> ClockOutcome,
    ) -> AttemptOutcome {
        let post = match read_non_trading(options, scope, Some(self.pre.version), deadline).await {
            Ok(_) => PostOutcome::Matched,
            Err(PreRefusal::Timeout) => PostOutcome::Timeout,
            Err(PreRefusal::Sql) => PostOutcome::Sql,
            // No row at `v₀`, or the scope no longer holds.
            Err(_) => PostOutcome::ScopeMoved,
        };
        let clock = revalidate();
        let after = gate.invalidity_marks();
        qualify(&QualificationMarks { before: self.marks, after, post, clock })
    }
}

/// One ACCOUNT receipt: the published sample it was admitted on and that
/// sample's u2 (`None` when none fit), its send and EOF brackets, and the
/// decoded body (PS-5b's receipt in production, the rig's in tests).
#[derive(Debug, Clone)]
pub(crate) struct AccountReceipt {
    pub(crate) sample: Option<(Uuid, i128)>,
    pub(crate) send: RawClockCapture,
    pub(crate) receive: RawClockCapture,
    pub(crate) body: SpotAccount,
}

/// What a conclusion takes from one ACCOUNT receipt (step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccountReading {
    /// `[send − u, receive + u]` of this receipt's own brackets.
    pub(crate) interval: ObservationInterval,
    /// The same `u`, so the writer recovers send and receive exactly.
    pub(crate) u_ns: i64,
    pub(crate) sample_id: Uuid,
    pub(crate) epoch_id: Uuid,
    /// The quote asset's free + locked from its one row, both ≥ 0; `None`
    /// when the row is missing, repeated or negative (never zero-filled).
    pub(crate) quote_total: Option<Decimal>,
}

impl AccountReading {
    /// The body's UID must be the scope's bound UID (`Scope`, F6); a sample,
    /// this process's epoch on both brackets and a valid interval are
    /// required (`Clock`). No asset but the quote asset is read here (SD7).
    pub(crate) fn from_receipt(
        receipt: &AccountReceipt,
        scope: &ScanScope<'_>,
    ) -> Result<Self, UnqualifiedReason> {
        if receipt.body.uid.to_string() != scope.detector.uid {
            return Err(UnqualifiedReason::Scope);
        }
        let clock = UnqualifiedReason::Clock;
        let (send, receive) = (&receipt.send, &receipt.receive);
        let (sample_id, u2) = receipt.sample.ok_or(clock)?;
        let epoch = scope.detector.expected.process_epoch;
        if send.epoch != epoch || receive.epoch != epoch {
            return Err(clock);
        }
        let interval = ObservationInterval::from_account(send, receive, u2).ok_or(clock)?;
        let u_ns = account_u(u2).and_then(|u| i64::try_from(u).ok()).ok_or(clock)?;
        let mut rows = receipt.body.balances.iter().filter(|row| row.asset == scope.quote_asset);
        let quote_total = match (rows.next(), rows.next()) {
            (Some(row), None) if row.free >= Decimal::ZERO && row.locked >= Decimal::ZERO => {
                exact_add(row.free, row.locked).ok()
            },
            _ => None,
        };
        Ok(Self {
            interval,
            u_ns,
            sample_id,
            epoch_id: send.epoch,
            quote_total,
        })
    }
}

/// The writer's input (§6.5), only from a qualified `Clean` drawn on this
/// capture and reading. Any other verdict comes back unchanged; a Clean on
/// another interval is refused, and one whose quote row is malformed comes
/// back Incomplete. `received` is the floor taken before the ACCOUNT request.
pub(crate) fn cash_attempt(
    verdict: BaseVerdict,
    capture: &Capture,
    reading: &AccountReading,
    received: Instant,
) -> Result<CashAttempt, BaseVerdict> {
    let BaseVerdict::Clean { interval } = verdict else {
        return Err(verdict);
    };
    if interval != reading.interval {
        return Err(BaseVerdict::Unqualified(UnqualifiedReason::Malformed));
    }
    let Some(quote_total) = reading.quote_total else {
        let reason = IncompleteReason::MalformedAccount;
        return Err(BaseVerdict::Incomplete { reason, interval });
    };
    Ok(CashAttempt {
        verdict: BaseVerdict::Clean { interval },
        version: capture.pre.version,
        marks: capture.marks,
        quote_total,
        u_ns: reading.u_ns,
        sample_id: reading.sample_id,
        epoch_id: reading.epoch_id,
        received,
    })
}
