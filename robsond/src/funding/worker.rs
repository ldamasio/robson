use std::time::Duration;

use robson_exec::ExchangePort;
use robson_store::Store;
use tokio::time::interval;
use tracing::{debug, error, info};

use super::saga::FundingService;
use crate::error::DaemonResult;

pub struct FundingWorker<E: ExchangePort + 'static, S: Store + 'static> {
    service: FundingService<E, S>,
    poll_interval: Duration,
}

impl<E: ExchangePort + 'static, S: Store + 'static> FundingWorker<E, S> {
    pub fn new(service: FundingService<E, S>) -> Self {
        Self {
            service,
            poll_interval: Duration::from_secs(5),
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
                    match self.service.resume_non_terminal().await {
                        Ok(count) if count > 0 => info!(count, "Funding worker resumed sagas"),
                        Ok(_) => {},
                        Err(error) => error!(%error, "Funding worker poll failed"),
                    }
                }
            }
        }

        info!("Funding worker stopped");
        Ok(())
    }
}
