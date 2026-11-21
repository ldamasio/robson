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

    /// The same three reads, carrying the physical fixture's operated orders.
    pub(super) async fn attribution(deadline: Instant) -> anyhow::Result<Self> {
        Self::scripted(deadline, None, attribution_script()).await
    }

    /// Scripted signed inventory answers in connection order, any number of
    /// attempts. A held one writes its head, reports it, and waits.
    pub(super) async fn inventory(
        deadline: Instant,
        script: Vec<(&'static str, u16, String)>,
    ) -> anyhow::Result<Self> {
        Self::respond(deadline, None, script).await
    }

    pub(super) async fn held_inventory(
        deadline: Instant,
        script: Vec<(&'static str, u16, String)>,
        at: usize,
    ) -> anyhow::Result<(Self, oneshot::Receiver<()>, oneshot::Sender<()>)> {
        let (head, seen) = oneshot::channel();
        let (release, ready) = oneshot::channel();
        let fixture = Self::respond_at(deadline, Some((head, ready)), at, script).await?;
        Ok((fixture, seen, release))
    }

    pub(super) async fn owned_order(deadline: Instant) -> anyhow::Result<Self> {
        Self::owned_order_answer(deadline, None, 200, owned_order_body()).await
    }

    pub(super) async fn restart_order_answers<const N: usize>(
        deadline: Instant,
        answers: [(u16, String); N],
    ) -> anyhow::Result<Self> {
        const KEY: &str = "/api/v3/order?symbol=BTCUSDT&orderId=417013";
        Self::respond(
            deadline,
            None,
            answers.into_iter().map(|(status, body)| (KEY, status, body)).collect(),
        )
        .await
    }

    pub(super) async fn held_order_not_found(
        deadline: Instant,
    ) -> anyhow::Result<(Self, oneshot::Receiver<()>, oneshot::Sender<()>)> {
        let (head, seen) = oneshot::channel();
        let (release, ready) = oneshot::channel();
        let fixture = Self::owned_order_answer(
            deadline,
            Some((head, ready)),
            400,
            r#"{"code":-2013,"msg":"synthetic unresolved order"}"#.into(),
        )
        .await?;
        Ok((fixture, seen, release))
    }

    async fn owned_order_answer(
        deadline: Instant,
        gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
        status: u16,
        body: String,
    ) -> anyhow::Result<Self> {
        const KEY: &str = "/api/v3/order?symbol=BTCUSDT&orderId=417013";
        Self::respond(deadline, gate, vec![(KEY, status, body)]).await
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
        body_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
        script: Vec<(&'static str, String)>,
    ) -> anyhow::Result<Self> {
        Self::respond(
            deadline,
            body_gate,
            script.into_iter().map(|(path, body)| (path, 200, body)).collect(),
        )
        .await
    }

    async fn respond(
        deadline: Instant,
        body_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
        script: Vec<(&'static str, u16, String)>,
    ) -> anyhow::Result<Self> {
        Self::respond_at(deadline, body_gate, 0, script).await
    }

    /// The body gate holds the exchange at connection index `hold` only.
    async fn respond_at(
        deadline: Instant,
        mut body_gate: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
        hold: usize,
        script: Vec<(&'static str, u16, String)>,
    ) -> anyhow::Result<Self> {
        use robson_connectors::inventory_loopback::{LOOPBACK_BODY_LIMIT, LOOPBACK_KEY};
        ensure!(script
            .iter()
            .all(|(_, _, body)| !body.is_empty() && body.len() <= LOOPBACK_BODY_LIMIT));
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
                    let (path, status, body) =
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
                            .and_then(|line| {
                                let delimiter = if path.contains('?') {
                                    "&timestamp="
                                } else {
                                    "?timestamp="
                                };
                                line.strip_prefix(delimiter)
                            })
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
                        let head = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        socket.write_all(head.as_bytes()).await?;
                        if let Some((sent, ready)) = body_gate.take_if(|_| index == hold) {
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

/// Synthetic ACCOUNT, operated-pair OPEN_ORDERS and account-wide
/// OPEN_ORDER_LIST bodies: full documented keys, an unrelated pair's list kept
/// with its member referenced only by the list. Test identities only; no
/// financial meaning.
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
        format!("[{},{}]", order("BTCUSDT", 12, -1, "c8-b"), order("BTCUSDT", 13, -1, "c8-c"));
    let lists = "[{\"orderListId\":7,\"contingencyType\":\"OCO\",\"listStatusType\":\"EXEC_STARTED\",\"listOrderStatus\":\"EXECUTING\",\"listClientOrderId\":\"c8-l\",\"transactionTime\":3,\"symbol\":\"AAABBB\",\"orders\":[{\"symbol\":\"AAABBB\",\"orderId\":11,\"clientOrderId\":\"c8-a\"}]}]"
        .to_string();
    vec![
        ("/api/v3/account", account),
        ("/api/v3/openOrders?symbol=BTCUSDT", orders),
        ("/api/v3/openOrderList", lists),
    ]
}

/// Attribution variant: the physical fixture's verbatim discovered
/// BTCUSDT/417011 BUY, an operated list member with no durable row, the
/// fixture's authored BTCUSDT/417013 entry, and an unrelated pair's list that
/// the scoped OPEN_ORDERS no longer carries orders for.
/// Test identities only; no financial meaning.
pub(super) fn attribution_script() -> Vec<(&'static str, String)> {
    use robson_connectors::inventory_loopback::LOOPBACK_UID;
    let account = format!(
        "{{\"canTrade\":false,\"accountType\":\"SPOT\",\"permissions\":[\"SPOT\"],\"balances\":[],\"uid\":{LOOPBACK_UID}}}"
    );
    let order = |symbol: &str, id: u64, list: i64, client: &str, kind: &str, side: &str| {
        format!(
            "{{\"symbol\":\"{symbol}\",\"orderId\":{id},\"orderListId\":{list},\"clientOrderId\":\"{client}\",\"price\":\"1.0\",\"origQty\":\"1.0\",\"executedQty\":\"0.0\",\"cummulativeQuoteQty\":\"0.0\",\"status\":\"NEW\",\"timeInForce\":\"GTC\",\"type\":\"{kind}\",\"side\":\"{side}\",\"stopPrice\":\"0.9\",\"time\":1,\"updateTime\":2,\"workingTime\":-1,\"selfTradePreventionMode\":\"EXPIRE_MAKER\"}}"
        )
    };
    let orders = format!(
        "[{},{},{}]",
        order("BTCUSDT", 417011, -1, "physical-recovery-observed-buy", "LIMIT", "BUY"),
        order("BTCUSDT", 417012, 9, "c8-absent-sell", "STOP_LOSS_LIMIT", "SELL"),
        order("BTCUSDT", 417013, -1, "physical-recovery-owned-buy", "MARKET", "BUY"),
    );
    let lists = "[{\"orderListId\":9,\"contingencyType\":\"OCO\",\"listStatusType\":\"EXEC_STARTED\",\"listOrderStatus\":\"EXECUTING\",\"listClientOrderId\":\"c8-operated-list\",\"transactionTime\":3,\"symbol\":\"BTCUSDT\",\"orders\":[{\"symbol\":\"BTCUSDT\",\"orderId\":417012,\"clientOrderId\":\"c8-absent-sell\"}]},{\"orderListId\":7,\"contingencyType\":\"OCO\",\"listStatusType\":\"EXEC_STARTED\",\"listOrderStatus\":\"EXECUTING\",\"listClientOrderId\":\"c8-l\",\"transactionTime\":3,\"symbol\":\"AAABBB\",\"orders\":[{\"symbol\":\"AAABBB\",\"orderId\":11,\"clientOrderId\":\"c8-a\"}]}]"
        .to_string();
    vec![
        ("/api/v3/account", account),
        ("/api/v3/openOrders?symbol=BTCUSDT", orders),
        ("/api/v3/openOrderList", lists),
    ]
}


// A synthetic later execution observation, distinct from the zero durable row.
fn owned_order_body() -> String {
    r#"{"symbol":"BTCUSDT","orderId":417013,"orderListId":-1,"clientOrderId":"physical-recovery-owned-buy","price":"0","origQty":"1","executedQty":"0.25","cummulativeQuoteQty":"25","status":"PARTIALLY_FILLED","timeInForce":"GTC","type":"MARKET","side":"BUY","stopPrice":"0","time":1,"updateTime":2,"workingTime":-1,"selfTradePreventionMode":"EXPIRE_MAKER"}"#.into()
}
