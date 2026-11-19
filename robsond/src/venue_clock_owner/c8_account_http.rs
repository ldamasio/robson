#![cfg(all(test, feature = "crash-points"))]
//! Bounded synthetic ACCOUNT listener for the native ClockOwner witness.

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, ensure, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};

use super::*;

/// One synthetic signed ACCOUNT answer on its own literal loopback listener.
/// It accepts and counts every connection until finished; only the first is
/// answered, and a second is a failure.
pub(super) struct AccountFixture {
    pub(super) port: u16,
    connections: Arc<AtomicUsize>,
    signing_stamp: Arc<Mutex<Vec<i64>>>,
    expected: usize,
    cancel: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}

impl AccountFixture {
    pub(super) async fn start(deadline: Instant) -> anyhow::Result<Self> {
        Self::with_gate(deadline, None).await
    }

    pub(super) async fn held_body(
        deadline: Instant,
    ) -> anyhow::Result<(Self, oneshot::Receiver<()>, oneshot::Sender<()>)> {
        let (head, seen) = oneshot::channel();
        let (release, ready) = oneshot::channel();
        let fixture = Self::with_gate(deadline, Some((head, ready))).await?;
        Ok((fixture, seen, release))
    }

    /// The three signed inventory reads, in order, one connection each, each
    /// body nonempty, typed and within the 64 KiB test envelope.
    pub(super) async fn triple(deadline: Instant) -> anyhow::Result<Self> {
        Self::scripted(deadline, None, triple_script()).await
    }

    async fn with_gate(
        deadline: Instant,
        body_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    ) -> anyhow::Result<Self> {
        use robson_connectors::inventory_loopback::LOOPBACK_UID;
        let body = format!(
            "{{\"canTrade\":false,\"accountType\":\"SPOT\",\"permissions\":[],\"balances\":[],\"uid\":{LOOPBACK_UID}}}"
        );
        Self::scripted(deadline, body_gate, vec![("/api/v3/account", body)]).await
    }

    async fn scripted(
        deadline: Instant,
        mut body_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
        script: Vec<(&'static str, String)>,
    ) -> anyhow::Result<Self> {
        use robson_connectors::inventory_loopback::{LOOPBACK_BODY_LIMIT, LOOPBACK_KEY};
        ensure!(script
            .iter()
            .all(|(_, body)| !body.is_empty() && body.len() <= LOOPBACK_BODY_LIMIT));
        let expected = script.len();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let connections = Arc::new(AtomicUsize::new(0));
        let signing_stamp = Arc::new(Mutex::new(Vec::new()));
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
                    let index = count.fetch_add(1, Ordering::SeqCst);
                    let (path, body) =
                        script.get(index).context("unexpected extra physical connection")?;
                    let exchange = async {
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            ensure!(request.len() < 16384, "bounded loopback head");
                            request.push(socket.read_u8().await?);
                        }
                        let request = std::str::from_utf8(&request)?;
                        let line = request.lines().next().unwrap_or_default();
                        let (stamp, signature) = line
                            .strip_prefix("GET ")
                            .and_then(|line| line.strip_prefix(*path))
                            .and_then(|line| line.strip_prefix("?timestamp="))
                            .and_then(|line| line.strip_suffix(" HTTP/1.1"))
                            .and_then(|query| query.split_once("&signature="))
                            .context("unexpected signed inventory request shape")?;
                        let stamp = stamp.parse::<i64>()?;
                        ensure!(
                            stamp > 0
                                && signature.len() == 64
                                && signature
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        );
                        observed_stamp
                            .lock()
                            .map_err(|_| anyhow!("stamp lock poisoned"))?
                            .push(stamp);
                        let lower = request.to_ascii_lowercase();
                        ensure!(lower.matches("\r\nx-mbx-apikey: ").count() == 1);
                        ensure!(lower.contains(&format!("\r\nx-mbx-apikey: {LOOPBACK_KEY}\r\n")));
                        let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        socket.write_all(head.as_bytes()).await?;
                        if let Some((sent, ready)) = body_gate.take() {
                            sent.send(()).map_err(|_| anyhow!("missing body controller"))?;
                            ready.await.context("missing body release")?;
                        }
                        socket.write_all(body.as_bytes()).await?;
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
            expected,
            cancel,
            task,
        })
    }

    /// Every physical stamp, in arrival order.
    pub(super) fn signing_stamps(&self) -> anyhow::Result<Vec<i64>> {
        Ok(self.signing_stamp.lock().map_err(|_| anyhow!("stamp lock poisoned"))?.clone())
    }

    pub(super) fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub(super) fn signing_stamp(&self) -> anyhow::Result<i64> {
        self.signing_stamp
            .lock()
            .map_err(|_| anyhow!("stamp lock poisoned"))?
            .first()
            .copied()
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
        ensure!(self.connections() <= self.expected, "extra physical connection observed");
        Ok(())
    }
}

/// Synthetic ACCOUNT, OPEN_ORDERS and OPEN_ORDER_LIST bodies: full documented
/// keys, an unrelated raw pair kept, a list member referenced by its order.
/// Test identities only; no financial meaning.
pub(super) fn triple_script() -> Vec<(&'static str, String)> {
    use robson_connectors::inventory_loopback::LOOPBACK_UID;
    let account = format!(
        "{{\"canTrade\":false,\"accountType\":\"SPOT\",\"permissions\":[\"SPOT\"],\"balances\":[{{\"asset\":\"AAA\",\"free\":\"1.00\",\"locked\":\"0\"}}],\"uid\":{LOOPBACK_UID}}}"
    );
    let order = |symbol: &str, id: u64, list: i64, client: &str| {
        format!(
            "{{\"symbol\":\"{symbol}\",\"orderId\":{id},\"orderListId\":{list},\"clientOrderId\":\"{client}\",\"price\":\"1.0\",\"origQty\":\"2.0\",\"executedQty\":\"0.0\",\"cummulativeQuoteQty\":\"0.0\",\"status\":\"NEW\",\"timeInForce\":\"GTC\",\"type\":\"STOP_LOSS_LIMIT\",\"side\":\"SELL\",\"stopPrice\":\"0.9\",\"time\":1,\"updateTime\":2,\"workingTime\":-1,\"selfTradePreventionMode\":\"EXPIRE_MAKER\"}}"
        )
    };
    let orders =
        format!("[{},{}]", order("AAABBB", 11, 7, "c8-a"), order("zzzQQQ", 12, -1, "c8-b"));
    let lists = "[{\"orderListId\":7,\"contingencyType\":\"OCO\",\"listStatusType\":\"EXEC_STARTED\",\"listOrderStatus\":\"EXECUTING\",\"listClientOrderId\":\"c8-l\",\"transactionTime\":3,\"symbol\":\"AAABBB\",\"orders\":[{\"symbol\":\"AAABBB\",\"orderId\":11,\"clientOrderId\":\"c8-a\"}]}]"
        .to_string();
    vec![
        ("/api/v3/account", account),
        ("/api/v3/openOrders", orders),
        ("/api/v3/openOrderList", lists),
    ]
}
