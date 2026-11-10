use std::{sync::Arc, time::Duration};

use robson_exec::{gate::OperationKind, order_journal::OrderJournal, ExchangePort};
use robson_store::Store;
use tokio::time::interval;
use tracing::{debug, error, info};

use super::saga::FundingService;
use crate::error::DaemonResult;

pub struct FundingWorker<E: ExchangePort + 'static, S: Store + 'static> {
    service: FundingService<E, S>,
    poll_interval: Duration,
    /// Gives each poll its permit (ADR-0056 step 7).
    order_journal: Option<Arc<OrderJournal>>,
}

impl<E: ExchangePort + 'static, S: Store + 'static> FundingWorker<E, S> {
    pub fn new(service: FundingService<E, S>) -> Self {
        Self {
            service,
            poll_interval: Duration::from_secs(5),
            order_journal: None,
        }
    }

    /// Run each poll under the funding slot's permit from `journal`.
    pub fn with_order_journal(mut self, journal: Arc<OrderJournal>) -> Self {
        self.order_journal = Some(journal);
        self
    }

    /// One poll of the unfinished sagas: it can convert and transfer, so it
    /// holds the funding poll's permit throughout.
    pub(crate) async fn resume(&self) -> DaemonResult<usize> {
        let poll = self.service.resume_non_terminal();
        match &self.order_journal {
            Some(journal) => journal.with_permit(OperationKind::FundingPoll, poll).await?,
            None => poll.await,
        }
    }

    pub async fn run(self, shutdown: tokio_util::sync::CancellationToken) -> DaemonResult<()> {
        info!("Funding worker started");
        let mut ticker = interval(self.poll_interval);
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("Funding worker shutdown requested");
                    break;
                }
                _ = ticker.tick() => {
                    match self.resume().await {
                        Ok(count) if count > 0 => info!(count, "Funding worker resumed sagas"),
                        Ok(_) => {},
                        Err(error) => error!(%error, "Funding worker poll failed"),
                    }
                    if let Err(error) = self.service.refresh_blocked_gauge().await {
                        error!(%error, "Funding worker blocked-saga count failed");
                    }
                }
            }
        }

        info!("Funding worker stopped");
        Ok(())
    }
}
