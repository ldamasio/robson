//! Controlled GET witness for one native restart. No live callers.
use robson_exec::{
    gate::PendingExecutionEvidence,
    venue::{OrderObservation, VenueOrderRecord},
};
use robson_testkit::db_proxy::DbProxy;

use super::*;

pub(crate) struct GovernedRestartRead {
    receipt: inventory::OwnedOrderReceipt,
    started: Instant,
}

impl GovernedRestartRead {
    pub(crate) fn after(&self, precheck: Instant, epoch: Uuid) -> Option<&OrderObservation> {
        (self.started > precheck
            && self.receipt.mark.raw.epoch == epoch
            && self.receipt.receive.epoch == epoch)
            .then_some(&self.receipt.observation)
    }

    /// A found record with its original first-poll mark and EOF capture,
    /// never restamped. A raw capture alone authorizes nothing.
    pub(crate) fn execution(&self) -> Option<(&VenueOrderRecord, PendingExecutionEvidence)> {
        let OrderObservation::Found(record) = &self.receipt.observation else {
            return None;
        };
        Some((record, PendingExecutionEvidence {
            sample: self.receipt.mark.sample,
            send: self.receipt.mark.raw,
            receive: self.receipt.receive,
            executed: record.executed_qty,
        }))
    }
}

pub(crate) struct RestartLookup {
    deadline: Instant,
    clock: Arc<NativeClock>,
    owner: Option<ClockOwner>,
    time: Option<http::Fixture>,
    order: Option<c8_account_http::AccountFixture>,
    slot: Option<inventory::OwnedOrderPreparation>,
    sample: Option<Uuid>,
    reads: usize,
    armed: usize,
}

impl RestartLookup {
    pub(crate) fn new(clock: Arc<NativeClock>, deadline: Instant) -> Self {
        Self {
            deadline,
            clock,
            owner: None,
            time: None,
            order: None,
            slot: None,
            sample: None,
            reads: 0,
            armed: 0,
        }
    }

    pub(crate) async fn arm<const N: usize>(
        &mut self,
        pool: &PgPool,
        tenant: Uuid,
        start: &process_epoch::EpochStart,
        input: &bootstrap::BootstrapInput,
        answers: [(u16, String); N],
    ) -> anyhow::Result<()> {
        ensure!(start.predecessor.unclean_reason() == Some("no_marker"));
        ensure!(N > 0 && self.armed == 0, "one scripted lookup per witness");
        self.armed = N;
        let due = self.deadline.min(Instant::now() + Duration::from_secs(2));
        let trace = Trace::default();
        let mut rows = bootstrap::observe_bootstrap(pool, tenant, start, input, due, &trace)
            .await
            .map_err(|e| anyhow!("successor budget bootstrap: {e:?}"))?;
        ensure!(trace.issued() == 6);
        restore::existing_pause(pool, tenant, &mut rows, &self.clock, due, &trace)
            .await
            .map_err(|e| anyhow!("successor budget pause: {e:?}"))?;
        ensure!(trace.issued() == 6);
        let budget =
            SharedBudget::from_rows(rows, true).map_err(|e| anyhow!("successor budget: {e:?}"))?;
        self.time = Some(http::Fixture::start(self.deadline).await?);
        self.order = Some(
            c8_account_http::AccountFixture::restart_order_answers(self.deadline, answers).await?,
        );
        let time = self.time.as_ref().context("TIME listener")?;
        let client = Arc::new(BinanceRestClient::with_harness_base_url(
            time.base_url.clone(),
            String::new(),
            String::new(),
        ));
        let store = ClockSampleStore::from_committed_epoch(pool, tenant, start);
        self.owner = Some(
            ClockOwner::controlled(
                Arc::clone(&self.clock),
                client,
                store,
                budget.clone(),
                tenant,
                start.epoch.epoch_id,
            )
            .map_err(|e| anyhow!("successor clock owner: {e:?}"))?,
        );
        let owner = self.owner.as_mut().context("successor owner")?;
        ensure!(matches!(owner.run_cycle().await, Err(TimeError::Budget(BudgetError::Deferred))));
        ensure!(time.connections() == 0 && budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 0);
        tokio::time::sleep(Duration::from_secs(120)).await;
        ensure!(
            self.clock
                .capture()?
                .elapsed_since(&budget.start().map_err(|e| anyhow!("{e:?}"))?)?
                >= Duration::from_secs(120)
        );
        let sample = owner.run_cycle().await.map_err(|e| anyhow!("successor TIME: {e:?}"))?;
        let durable: Vec<(Uuid, i64)> = sqlx::query_as(
            "SELECT sample_id,server_ms FROM public.clock_samples WHERE tenant_id=$1
             AND process_epoch=$2 AND venue_id='binance:spot' AND numeric_failures=0 LIMIT 2",
        )
        .bind(tenant)
        .bind(start.epoch.epoch_id)
        .fetch_all(pool)
        .await?;
        ensure!(durable == [(sample, time.server_ms()?)]);
        ensure!(time.connections() == 1 && budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == 1);
        self.sample = Some(sample);
        Ok(())
    }

    pub(crate) async fn read(&mut self, wire: &DbProxy) -> anyhow::Result<GovernedRestartRead> {
        let started = Instant::now();
        let deadline = self.deadline.min(started + Duration::from_secs(10));
        let owner = self.owner.as_ref().context("successor owner")?;
        let listener = self.order.as_ref().context("order listener")?;
        let signed = owner
            .commission_signing_time()
            .map_err(|e| anyhow!("successor signing: {e:?}"))?;
        let expected_handoffs = 1 + self.reads as u64;
        ensure!(owner.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == expected_handoffs);
        ensure!(listener.connections() == self.reads);
        let counters = || (wire.stats().statements, wire.prepared_statement_parses());
        let pre = counters();
        let receipt = owner
            .read_owned_order(
                &signed,
                self.clock.capture()?,
                &mut self.slot,
                listener.port,
                deadline,
            )
            .await
            .map_err(|e| anyhow!("successor signed order GET: {e:?}"))?;
        self.reads += 1;
        ensure!(counters() == pre && self.slot.is_none(), "GET issues no SQL");
        ensure!(
            listener.connections() == self.reads
                && listener.signing_stamps()?.last() == Some(&signed.timestamp_ms())
        );
        ensure!(owner.budget.handoffs().map_err(|e| anyhow!("{e:?}"))? == expected_handoffs + 1);
        ensure!(
            Some(receipt.mark.sample) == self.sample
                && receipt.mark.raw.epoch == owner.epoch
                && receipt.receive.epoch == owner.epoch
                && receipt.mark.raw.wall_agrees(&receipt.receive)?
                && receipt.receive.before_ns > receipt.mark.raw.after_ns
        );
        Ok(GovernedRestartRead { receipt, started })
    }

    /// Every scripted answer was read. Check actual remaining capacity:
    /// 400 - (1 TIME + 4 per GET), as 20-weight ACCOUNT then unit TIME.
    pub(crate) fn verify_weight(&self) -> anyhow::Result<()> {
        ensure!(self.armed > 0 && self.reads == self.armed);
        let owner = self.owner.as_ref().context("successor owner")?;
        let remaining = 400 - (1 + 4 * self.reads);
        verify_budget_remaining(&owner.budget, &self.clock, remaining / 20, remaining % 20)
    }

    pub(crate) fn reads(&self) -> usize {
        self.reads
    }

    pub(crate) async fn finish(
        mut self,
        start: &process_epoch::EpochStart,
        failed: bool,
    ) -> anyhow::Result<()> {
        let asked_at = Instant::now();
        let deadline = asked_at + crate::stop::StopTimings::default().deadline;
        let preparation = timeout_at(deadline, jobs::retire(&mut self.slot)).await;
        let stopped = if let Some(owner) = self.owner {
            if failed {
                owner.cancel.cancel();
            }
            let state = Arc::clone(&owner.state);
            let budget = owner.budget.clone();
            let handoffs = budget.handoffs();
            let closing = owner.spawn();
            close_owner(&closing, &state, &budget, handoffs, Some(start), asked_at).await
        } else {
            Ok(())
        };
        let order = match self.order {
            Some(listener) => listener.finish(deadline).await,
            None => Ok(()),
        };
        let time = match self.time {
            Some(listener) => listener.finish(deadline).await,
            None => Ok(()),
        };
        ensure!(matches!(preparation, Ok(true)), "restart GET preparation not retired");
        stopped?;
        order?;
        time
    }
}
