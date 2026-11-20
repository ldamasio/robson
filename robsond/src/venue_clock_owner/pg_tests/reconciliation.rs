//! Fixture-only 8k-2 composition (v16 §C8 steps 1-4) over the native
//! ClockOwner, its one SharedBudget and the loopback triple. The production
//! detector modules have no caller; this is their only composition.
//!
//! One attempt: gate marks, pre, signing time, triple, operated subset, keyed
//! post, gate marks, `qualify`; only a qualified attempt is classified.
use std::{future::Future, pin::Pin};

use robson_exec::{
    gate::{EpochView, PendingExecutionEvidence, PendingExecutionOwner},
    order_journal::{OrderJournal, StubJournalBackend},
};
use robson_testkit::db_proxy::DbProxy;
use sqlx::postgres::PgConnectOptions;
use tokio::sync::oneshot;

use super::*;
use crate::balance_reconciliation::{
    self as detector, AttemptOutcome, ClockOutcome, DetectorScope, ObservationInterval,
    PostOutcome, Progress, QualificationMarks, UnqualifiedReason,
};

/// Runs once, after the first completed triple and before its post.
pub(crate) type Hook<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + 'a>>;

/// The fixture budget input and its clock domain, kept for a restart on the
/// same database: a budget start is only comparable within its domain.
pub(crate) struct Reopen(bootstrap::BootstrapInput, Arc<NativeClock>);

/// One native owner on one committed epoch, its SharedBudget and a bounded
/// TIME listener. TIME runs only on the existing 60 s cadence.
pub(crate) struct ReconciliationRig {
    pub(crate) start: process_epoch::EpochStart,
    clock: Arc<NativeClock>,
    owner: Option<ClockOwner>,
    budget: SharedBudget,
    time: Option<http::Fixture>,
    slot: Option<inventory::AccountPreparation>,
    published: Option<Instant>,
    input: bootstrap::BootstrapInput,
}

/// What a fixture scan reads; the detector options are read-only, through
/// the counting proxy.
pub(crate) struct ScanInputs<'a> {
    pub(crate) wire: &'a DbProxy,
    pub(crate) options: &'a PgConnectOptions,
    pub(crate) scope: DetectorScope<'a>,
    pub(crate) gate: &'a robson_exec::gate::OperationGate,
    pub(crate) pending: &'a PendingExecutionOwner,
}

/// The ACCOUNT receipt of a completed triple, its sample's u2, and the last
/// receipt's EOF (the composite end, never the cut source).
#[derive(Debug, Clone, Copy)]
pub(crate) struct AccountSeen {
    pub(crate) send: RawClockCapture,
    pub(crate) receive: RawClockCapture,
    pub(crate) u2: i128,
    pub(crate) composite_end: RawClockCapture,
}

pub(crate) struct Scanned {
    pub(crate) verdict: detector::BaseVerdict,
    pub(crate) outcome: detector::ScanOutcome,
    /// Each attempt's own outcome, in order.
    pub(crate) attempts: Vec<AttemptOutcome>,
    /// Statements on the proxy, handoffs and physical inventory connections
    /// this scan made, measured independently of the cost model.
    pub(crate) sql: u64,
    pub(crate) handoffs: u64,
    pub(crate) connections: usize,
    pub(crate) account: Option<AccountSeen>,
}

/// A fixture clock fault: the owner's publication generation moves, as the
/// accepted C8 rehearsal does for a stale claim.
#[derive(Clone)]
pub(crate) struct ClockTrip(Arc<Mutex<ClockState>>);

impl ClockTrip {
    pub(crate) fn trip(&self) -> anyhow::Result<()> {
        self.0.lock().map_err(|_| anyhow!("clock lock"))?.generation += 1;
        Ok(())
    }
}

/// A loopback listener answering whole triples in connection order.
pub(crate) struct Listener(c8_account_http::AccountFixture);

pub(crate) struct Held {
    pub(crate) seen: oneshot::Receiver<()>,
    pub(crate) release: oneshot::Sender<()>,
}

fn script(triples: &[[String; 3]]) -> Vec<(&'static str, u16, String)> {
    triples
        .iter()
        .flat_map(|[account, orders, lists]| {
            [
                ("/api/v3/account", 200, account.clone()),
                ("/api/v3/openOrders", 200, orders.clone()),
                ("/api/v3/openOrderList", 200, lists.clone()),
            ]
        })
        .collect()
}

impl Listener {
    pub(crate) async fn triples(
        deadline: Instant,
        triples: &[[String; 3]],
    ) -> anyhow::Result<Self> {
        Ok(Self(
            c8_account_http::AccountFixture::inventory(deadline, script(triples)).await?,
        ))
    }

    /// The same, holding connection `at` after its head until released.
    pub(crate) async fn held(
        deadline: Instant,
        triples: &[[String; 3]],
        at: usize,
    ) -> anyhow::Result<(Self, Held)> {
        let (fixture, seen, release) =
            c8_account_http::AccountFixture::held_inventory(deadline, script(triples), at).await?;
        Ok((Self(fixture), Held { seen, release }))
    }

    pub(crate) fn connections(&self) -> usize {
        self.0.connections()
    }

    pub(crate) async fn finish(self, deadline: Instant) -> anyhow::Result<()> {
        self.0.finish(deadline).await
    }
}

fn time_refusal(error: &TimeError) -> UnqualifiedReason {
    match error {
        TimeError::Native => UnqualifiedReason::Clock,
        TimeError::Expired => UnqualifiedReason::Timeout,
        _ => UnqualifiedReason::Venue,
    }
}

type Attempted = (detector::Attempt, Option<detector::Classification>, Option<AccountSeen>);

impl ReconciliationRig {
    /// A fresh epoch and the fixture budget. It must run before any journal
    /// row exists; TIME waits for `ready`.
    pub(crate) async fn open(pool: &PgPool, tenant: Uuid) -> anyhow::Result<Self> {
        let clock = Arc::new(NativeClock::initialize()?);
        let start = process_epoch::start_epoch(pool, tenant, "8k2-detector-fixture").await?;
        let input = venue_budget::fixture::prepare(
            pool,
            &clock,
            Instant::now() + Duration::from_secs(2),
            &Trace::default(),
        )
        .await
        .map_err(|e| anyhow!("budget fixture: {e:?}"))?;
        Self::on_epoch(pool, tenant, clock, start, input).await
    }

    /// A restart: a new epoch of the same tenant and a new owner, budget and
    /// TIME listener on the same fixture input; no state of the closed rig.
    pub(crate) async fn reopen(pool: &PgPool, tenant: Uuid, input: Reopen) -> anyhow::Result<Self> {
        let Reopen(input, clock) = input;
        let start = process_epoch::start_epoch(pool, tenant, "8k2-detector-restart").await?;
        Self::on_epoch(pool, tenant, clock, start, input).await
    }

    async fn on_epoch(
        pool: &PgPool,
        tenant: Uuid,
        clock: Arc<NativeClock>,
        start: process_epoch::EpochStart,
        input: bootstrap::BootstrapInput,
    ) -> anyhow::Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let trace = Trace::default();
        let mut rows = bootstrap::observe_bootstrap(pool, tenant, &start, &input, deadline, &trace)
            .await
            .map_err(|e| anyhow!("budget bootstrap: {e:?}"))?;
        restore::existing_pause(pool, tenant, &mut rows, &clock, deadline, &trace)
            .await
            .map_err(|e| anyhow!("budget pause: {e:?}"))?;
        let budget = SharedBudget::from_rows(rows, start.predecessor.unclean_reason().is_some())
            .map_err(|e| anyhow!("budget: {e:?}"))?;
        // One TIME answer per 60 s cadence over a fixture of at most 20 minutes.
        let time = http::Fixture::bounded(Instant::now() + Duration::from_secs(1200), 20).await?;
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            time.base_url.clone(),
            String::new(),
            String::new(),
        ));
        let store = ClockSampleStore::from_committed_epoch(pool, tenant, &start);
        let owner = ClockOwner::controlled(
            Arc::clone(&clock),
            client,
            store,
            budget.clone(),
            tenant,
            start.epoch.epoch_id,
        )
        .map_err(|e| anyhow!("owner: {e:?}"))?;
        Ok(Self {
            start,
            clock,
            owner: Some(owner),
            budget,
            time: Some(time),
            slot: None,
            published: None,
            input,
        })
    }

    /// The real 120 s unclean barrier from the budget start, then TIME.
    pub(crate) async fn ready(&mut self) -> anyhow::Result<()> {
        let start = self.budget.start().map_err(|e| anyhow!("start: {e:?}"))?;
        let elapsed = self.clock.capture()?.elapsed_since(&start)?;
        if let Some(rest) = Duration::from_secs(121).checked_sub(elapsed) {
            tokio::time::sleep(rest).await;
        }
        self.refresh().await
    }

    /// A sample at most 40 s old for the next scan: TIME again once the
    /// published one is older, waiting for the 60 s cadence if needed.
    pub(crate) async fn refresh(&mut self) -> anyhow::Result<()> {
        if self.published.is_some_and(|at| at.elapsed() < Duration::from_secs(40)) {
            return Ok(());
        }
        if let Some(rest) =
            self.published.and_then(|at| Duration::from_secs(61).checked_sub(at.elapsed()))
        {
            tokio::time::sleep(rest).await;
        }
        let owner = self.owner.as_mut().context("open owner")?;
        owner.run_cycle().await.map_err(|e| anyhow!("TIME: {e:?}"))?;
        self.published = Some(Instant::now());
        Ok(())
    }

    /// Lets the published sample pass its 60 s age without a new one.
    pub(crate) async fn stale(&mut self) -> anyhow::Result<()> {
        let at = self.published.context("a published sample")?;
        if let Some(rest) = Duration::from_secs(62).checked_sub(at.elapsed()) {
            tokio::time::sleep(rest).await;
        }
        Ok(())
    }

    /// A fresh sample and steady capacity for `triples` whole triples. The
    /// probe reservations are dropped unsent, which cancels them.
    pub(crate) async fn settle(&mut self, triples: usize) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            self.refresh().await?;
            let now = self.clock.capture()?;
            let due = after(now, Duration::from_secs(1)).map_err(|e| anyhow!("{e:?}"))?;
            let probe: Result<Vec<_>, _> = (0..triples)
                .flat_map(|_| InventoryRead::ALL)
                .map(|read| self.budget.try_reserve_inventory(read, now, due))
                .collect();
            if probe.is_ok() {
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "no steady capacity for {triples} triples");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub(crate) fn clock_trip(&self) -> anyhow::Result<ClockTrip> {
        Ok(ClockTrip(Arc::clone(&self.owner.as_ref().context("open owner")?.state)))
    }

    /// A gate on this epoch and its bound pending-execution owner.
    pub(crate) fn gate(&self) -> anyhow::Result<(OrderJournal, PendingExecutionOwner)> {
        let journal = OrderJournal::new(
            Arc::new(StubJournalBackend::default()),
            &VenueId::new(VenueId::BINANCE_SPOT),
        );
        journal.gate().begin_epoch(
            EpochView {
                epoch_seq: self.start.epoch.epoch_seq,
                unclean_reason: self.start.predecessor.unclean_reason(),
            },
            self.start.unclean_generation,
            self.start.open_incidents.clone(),
        );
        let pending = journal
            .gate()
            .bind_pending_execution(self.start.epoch.epoch_id)
            .context("pending-execution owner")?;
        Ok((journal, pending))
    }

    /// Native evidence on this epoch, as an exact lookup would retain it.
    pub(crate) async fn evidence(
        &self,
        executed: rust_decimal::Decimal,
    ) -> anyhow::Result<PendingExecutionEvidence> {
        let owner = self.owner.as_ref().context("open owner")?;
        let capture = || {
            let mut state = owner.state.lock().map_err(|_| anyhow!("clock lock"))?;
            let sample = state.published.as_ref().map(|p| p.id).context("published sample")?;
            let capture = state
                .capture_native(&self.clock, self.start.epoch.epoch_id)
                .map_err(|e| anyhow!("capture: {e:?}"))?;
            Ok::<_, anyhow::Error>((sample, capture))
        };
        let (sample, send) = capture()?;
        tokio::time::sleep(Duration::from_millis(2)).await;
        let (_, receive) = capture()?;
        Ok(PendingExecutionEvidence { sample, send, receive, executed })
    }

    /// At most three attempts, each on a fresh deadline; never a partial.
    pub(crate) async fn scan(
        &mut self,
        inputs: &ScanInputs<'_>,
        listener: &Listener,
        mut between: Option<Hook<'_>>,
    ) -> anyhow::Result<Scanned> {
        let sql = inputs.wire.stats().statements;
        let handoffs = self.budget.handoffs().map_err(|e| anyhow!("{e:?}"))?;
        let connections = listener.connections();
        let mut scan = detector::Scan::default();
        let (mut account, mut attempts) = (None, Vec::new());
        let mut classification;
        loop {
            let deadline = Instant::now() + Duration::from_secs(10);
            let (attempt, classified, seen) =
                self.attempt(inputs, listener.0.port, &mut between, deadline).await?;
            classification = classified;
            account = seen.or(account);
            attempts.push(attempt.outcome);
            if !scan.record(attempt) {
                break;
            }
        }
        let outcome = scan.finish().context("one attempt")?;
        Ok(Scanned {
            verdict: detector::conclude(&outcome, classification),
            outcome,
            attempts,
            sql: inputs.wire.stats().statements - sql,
            handoffs: self.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? - handoffs,
            connections: listener.connections() - connections,
            account,
        })
    }

    async fn attempt(
        &mut self,
        inputs: &ScanInputs<'_>,
        port: u16,
        between: &mut Option<Hook<'_>>,
        deadline: Instant,
    ) -> anyhow::Result<Attempted> {
        use c8_attribution::Refusal;
        let refused = |progress: Progress, reason: UnqualifiedReason| detector::Attempt {
            progress,
            outcome: AttemptOutcome::Unqualified(reason),
        };
        // C8 step 1: the in-memory marks before pre.
        let generation_before = inputs.gate.generation();
        let pending_before = inputs.pending.pending().is_some();
        let mut progress = Progress { pre: true, ..Progress::default() };
        let pre = match detector::read_pre(inputs.options, inputs.scope, deadline).await {
            Ok(pre) => pre,
            Err(refusal) => return Ok((refused(progress, refusal.into()), None, None)),
        };
        let owner = self.owner.as_ref().context("open owner")?;
        let signed = match owner.commission_signing_time() {
            Ok(signed) => signed,
            Err(error) => return Ok((refused(progress, time_refusal(&error)), None, None)),
        };
        let claim = self.clock.capture()?;
        let handoffs = self.budget.handoffs().map_err(|e| anyhow!("{e:?}"))?;
        let statements = inputs.wire.stats().statements;
        let triple = owner
            .read_inventory_triple(&signed, claim, &mut self.slot, port, deadline)
            .await;
        let sent = self.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? - handoffs;
        progress.responses = u8::try_from(sent)?;
        ensure!(inputs.wire.stats().statements == statements, "no SQL during the inventory");
        let triple = match triple {
            Ok(triple) => triple,
            Err(error) => return Ok((refused(progress, time_refusal(&error)), None, None)),
        };
        let [account, orders, lists] = &triple.receipts;
        let u2 = owner
            .state
            .lock()
            .map_err(|_| anyhow!("clock lock"))?
            .published
            .as_ref()
            .filter(|p| p.id == account.sample)
            .map(|p| p.u2);
        let seen = u2.map(|u2| AccountSeen {
            send: account.send,
            receive: account.receive,
            u2,
            composite_end: lists.receive,
        });
        let (
            InventoryBody::Account(body),
            InventoryBody::OpenOrders(open),
            InventoryBody::OpenOrderLists(listed),
        ) = (&account.body, &orders.body, &lists.body)
        else {
            return Ok((refused(progress, UnqualifiedReason::Malformed), None, seen));
        };
        let witness = c8_attribution::Scope {
            expected: inputs.scope.expected,
            uid: inputs.scope.uid,
            version: pre.version,
            operated: inputs.scope.operated,
        };
        let operated = match c8_attribution::select(witness, open, listed) {
            Ok(operated) => operated,
            Err(Refusal::Cap) => {
                return Ok((refused(progress, UnqualifiedReason::Cap), None, seen))
            },
            Err(_) => return Ok((refused(progress, UnqualifiedReason::Malformed), None, seen)),
        };
        if let Some(hook) = between.take() {
            hook.await?;
        }
        // C8 step 3: the keyed attribution snapshot requires version v0.
        progress.post = true;
        let (post, facts) =
            match c8_attribution::snapshot(inputs.options, witness, &operated.orders, deadline)
                .await
            {
                Ok(rows) => match c8_attribution::attribute(witness, &operated.orders, &rows) {
                    Ok(facts) => (PostOutcome::Matched, facts),
                    Err(_) => {
                        return Ok((refused(progress, UnqualifiedReason::Malformed), None, seen))
                    },
                },
                Err(Refusal::Scope) => (PostOutcome::ScopeMoved, Vec::new()),
                Err(Refusal::Timeout) => (PostOutcome::Timeout, Vec::new()),
                Err(Refusal::Sql) => (PostOutcome::Sql, Vec::new()),
                Err(Refusal::Cap) => {
                    return Ok((refused(progress, UnqualifiedReason::Cap), None, seen))
                },
                Err(_) => return Ok((refused(progress, UnqualifiedReason::Malformed), None, seen)),
            };
        // C8 step 2: I and its cut from the ACCOUNT receipt only, whose
        // anchor is revalidated now: never retroactively valid.
        let anchor = inventory::AccountMark {
            generation: account.generation,
            sample: account.sample,
            raw: account.send,
            handoff: account.handoff,
        };
        let interval = u2
            .filter(|_| owner.account_receipt(&anchor).is_ok())
            .and_then(|u2| ObservationInterval::from_account(&account.send, &account.receive, u2));
        let clock = interval.map_or(ClockOutcome::Invalid, ClockOutcome::Valid);
        // C8 step 4: the marks again, after post.
        let marks = QualificationMarks {
            generation_before,
            generation_after: inputs.gate.generation(),
            pending_before,
            pending_after: inputs.pending.pending().is_some(),
            post,
            clock,
        };
        let outcome = detector::qualify(&marks);
        let classification = matches!(outcome, AttemptOutcome::Qualified(_)).then(|| {
            detector::classify(inputs.scope, &pre, body, &c8_attribution::listing(operated, facts))
        });
        Ok((detector::Attempt { progress, outcome }, classification, seen))
    }

    /// Retires the preparation slot, the owner through the real stop, and
    /// the TIME listener; the epoch stays unmarked.
    pub(crate) async fn close(mut self) -> anyhow::Result<Reopen> {
        let asked_at = Instant::now();
        let deadline = asked_at + crate::stop::StopTimings::default().deadline;
        let retired = timeout_at(deadline, ClockOwner::retire_account(&mut self.slot)).await;
        let stopped = match self.owner.take() {
            Some(owner) => {
                let state = Arc::clone(&owner.state);
                let budget = owner.budget.clone();
                let handoffs = budget.handoffs();
                let closing = owner.spawn();
                close_owner(&closing, &state, &budget, handoffs, Some(&self.start), asked_at).await
            },
            None => Ok(()),
        };
        let time = match self.time.take() {
            Some(time) => time.finish(deadline).await,
            None => Ok(()),
        };
        ensure!(matches!(retired, Ok(true)), "inventory preparation not retired");
        stopped?;
        time?;
        Ok(Reopen(self.input, self.clock))
    }
}
