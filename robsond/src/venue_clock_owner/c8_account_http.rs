#![cfg(all(test, feature = "crash-points"))]
//! Bounded synthetic ACCOUNT listener for the native ClockOwner witness.

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, ensure, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

use super::*;

/// One synthetic signed ACCOUNT answer on its own literal loopback listener.
/// It accepts and counts every connection until finished; only the first is
/// answered, and a second is a failure.
pub(super) struct AccountFixture {
    pub(super) port: u16,
    connections: Arc<AtomicUsize>,
    signing_stamp: Arc<Mutex<Option<i64>>>,
    cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl AccountFixture {
    pub(super) async fn start(deadline: Instant) -> anyhow::Result<Self> {
        use robson_connectors::inventory_loopback::{LOOPBACK_KEY, LOOPBACK_UID};
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let connections = Arc::new(AtomicUsize::new(0));
        let signing_stamp = Arc::new(Mutex::new(None));
        let cancel = CancellationToken::new();
        let (count, stopped) = (Arc::clone(&connections), cancel.clone());
        let observed_stamp = Arc::clone(&signing_stamp);
        let task = tokio::spawn(async move {
            let work = async {
                loop {
                    let (mut socket, _) = tokio::select! {
                        biased;
                        _ = stopped.cancelled() => return Ok(()),
                        next = listener.accept() => next?,
                    };
                    ensure!(
                        count.fetch_add(1, Ordering::SeqCst) == 0,
                        "unexpected second physical ACCOUNT connection"
                    );
                    let exchange = async {
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            ensure!(request.len() < 16384, "bounded loopback head");
                            request.push(socket.read_u8().await?);
                        }
                        let request = std::str::from_utf8(&request)?;
                        let line = request.lines().next().unwrap_or_default();
                        let (stamp, signature) = line
                            .strip_prefix("GET /api/v3/account?timestamp=")
                            .and_then(|line| line.strip_suffix(" HTTP/1.1"))
                            .and_then(|query| query.split_once("&signature="))
                            .context("unexpected signed ACCOUNT request shape")?;
                        let stamp = stamp.parse::<i64>()?;
                        ensure!(
                            stamp > 0
                                && signature.len() == 64
                                && signature
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        );
                        *observed_stamp.lock().map_err(|_| anyhow!("stamp lock poisoned"))? =
                            Some(stamp);
                        let lower = request.to_ascii_lowercase();
                        ensure!(lower.matches("\r\nx-mbx-apikey: ").count() == 1);
                        ensure!(lower.contains(&format!("\r\nx-mbx-apikey: {LOOPBACK_KEY}\r\n")));
                        let body = format!(
                            "{{\"canTrade\":false,\"accountType\":\"SPOT\",\"permissions\":[],\"balances\":[],\"uid\":{LOOPBACK_UID}}}"
                        );
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        socket.write_all(response.as_bytes()).await?;
                        socket.shutdown().await?;
                        Ok::<_, anyhow::Error>(())
                    };
                    tokio::select! {
                        biased;
                        _ = stopped.cancelled() => return Ok(()),
                        result = timeout_at((Instant::now() + Duration::from_secs(3)).min(deadline), exchange) => {
                            result.context("loopback exchange deadline")??;
                        },
                    }
                }
            };
            timeout_at(deadline, work).await.context("loopback fixture ceiling")?
        });
        Ok(Self {
            port,
            connections,
            signing_stamp,
            cancel,
            task,
        })
    }

    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) fn signing_stamp(&self) -> anyhow::Result<i64> {
        self.signing_stamp
            .lock()
            .map_err(|_| anyhow!("stamp lock poisoned"))?
            .context("no physical ACCOUNT stamp")
    }

    pub(super) async fn finish(mut self, deadline: Instant) -> anyhow::Result<()> {
        self.cancel.cancel();
        match timeout_at((Instant::now() + Duration::from_secs(2)).min(deadline), &mut self.task)
            .await
        {
            Ok(result) => result.context("ACCOUNT fixture task")?,
            Err(_) => {
                self.task.abort();
                let retired = timeout_at(deadline, &mut self.task).await;
                return Err(anyhow!("ACCOUNT fixture needed abort; join={retired:?}"));
            },
        }?;
        ensure!(self.connections() <= 1, "second physical ACCOUNT connection observed");
        Ok(())
    }
}
