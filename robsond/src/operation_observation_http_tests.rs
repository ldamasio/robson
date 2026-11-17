//! Actual REST/adapter requests against a bounded, joined local TCP listener.
use std::{future::Future, sync::Arc};

use robson_connectors::{
    binance::usdm::BinanceExchangeAdapter, BinanceRestClient, BinanceRestError,
};
use robson_exec::{
    order_journal::OrderJournal,
    venue::{StopRef, TradingPort},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{oneshot, Notify},
    task::JoinHandle,
};
use tracing::instrument::WithSubscriber;

use super::probe::Probe;

type Server = JoinHandle<Result<Vec<String>, std::io::Error>>;
async fn scripted(
    replies: Vec<(u16, String)>,
    hold: bool,
) -> (BinanceRestClient, oneshot::Sender<()>, Server, Arc<Notify>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = BinanceRestClient::with_harness_base_url(
        format!("http://{}", listener.local_addr().unwrap()),
        "key".into(),
        "secret".into(),
    );
    let (stop, mut stopped) = oneshot::channel();
    let arrived = Arc::new(Notify::new());
    let signal = Arc::clone(&arrived);
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in replies {
            let (mut stream, _) = tokio::select! { _ = &mut stopped => return Ok(requests), accepted = listener.accept() => accepted? };
            let mut head = Vec::new();
            let mut buf = [0_u8; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = tokio::select! { _ = &mut stopped => return Ok(requests), n = stream.read(&mut buf) => n? };
                if n == 0 || head.len() + n > 8192 {
                    return Err(std::io::Error::other("incomplete test request"));
                }
                head.extend_from_slice(&buf[..n]);
            }
            let request = String::from_utf8_lossy(&head);
            let first = request.lines().next().unwrap_or_default();
            let mut parts = first.split_whitespace();
            let method = parts.next().unwrap_or_default();
            let target = parts.next().unwrap_or_default();
            let path = target.split('?').next().unwrap_or_default();
            // Keep the paging cursor only, never signature/key/query payload.
            let cursor = target
                .split_once('?')
                .and_then(|(_, query)| query.split('&').find(|part| part.starts_with("fromId=")));
            requests.push(format!(
                "{method} {path}{}",
                cursor.map(|v| format!(" {v}")).unwrap_or_default()
            ));
            signal.notify_one();
            if hold {
                let _ = (&mut stopped).await;
                return Ok(requests);
            }
            let response = format!("HTTP/1.1 {status} Test\r\nLocation: /must-not-follow\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).await?;
        }
        Ok(requests)
    });
    (client, stop, server, arrived)
}
async fn measured<T, E>(probe: &Probe, work: impl Future<Output = Result<T, E>>) -> Result<T, E> {
    robson_exec::observation::reconciliation(async {
        let result = work.await;
        tracing::Span::current().record("outcome", if result.is_ok() { 1_u64 } else { 2_u64 });
        result
    })
    .with_subscriber(probe.dispatch())
    .await
}
fn assert_http(probe: &Probe, server_requests: usize) {
    let actual = probe.snapshot();
    assert_eq!(actual[0], 0, "HTTP-only operation has no database work");
    assert_eq!(
        actual[1], server_requests as u64,
        "real submissions versus independently received requests"
    );
    assert_eq!(
        actual[3],
        2 * server_requests as u64,
        "guardrail weight, not Binance REQUEST_WEIGHT"
    );
}

#[tokio::test]
async fn real_conditional_order_trade_fanout_matches_server_and_production_counter() {
    let algo = r#"{"algoId":42,"clientAlgoId":"ins-1","algoType":"CONDITIONAL","orderType":"STOP_MARKET","symbol":"BTCUSDT","side":"SELL","quantity":"0.1","algoStatus":"TRIGGERED","triggerPrice":"58888","reduceOnly":true,"createTime":1,"updateTime":2,"actualOrderId":"7","triggerTime":1700000000000}"#;
    let order = r#"{"symbol":"BTCUSDT","orderId":7,"clientOrderId":"ins-1","updateTime":1700000000000,"price":"0","avgPrice":"58888","origQty":"0.1","executedQty":"0.04","cumQuote":"2355.52","status":"CANCELED","side":"SELL","type":"MARKET","fills":[]}"#;
    let trades = r#"[{"symbol":"BTCUSDT","id":1,"orderId":7,"side":"SELL","price":"58888","qty":"0.04","quoteQty":"2355.52","commission":"1","commissionAsset":"USDT","realizedPnl":"0","time":1700000000500}]"#;
    let (client, stop, server, _) = scripted(
        vec![
            (200, algo.into()),
            (200, order.into()),
            (200, trades.into()),
        ],
        false,
    )
    .await;
    let venue = BinanceExchangeAdapter::new(Arc::new(client));
    let probe = Probe::new();
    let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
    let result =
        measured(&probe, TradingPort::protection_fill(&venue, &symbol, &StopRef::new("42"))).await;
    let _ = stop.send(());
    let received = server.await;
    let received = received.unwrap().unwrap();
    assert_eq!(received, [
        "GET /fapi/v1/algoOrder",
        "GET /fapi/v1/order",
        "GET /fapi/v1/userTrades"
    ]);
    assert!(result.unwrap().is_some(), "real fee-grade trade result reaches caller");
    assert_http(&probe, received.len());
    assert_eq!(probe.snapshot()[4], 0);
}
#[tokio::test]
async fn real_redirect_and_server_error_are_one_submission_without_hidden_followup() {
    for status in [302, 503] {
        let (client, stop, server, _) =
            scripted(vec![(status, r#"{"code":-1,"msg":"test answer"}"#.into())], false).await;
        let probe = Probe::new();
        let result = measured(&probe, client.get_all_open_positions()).await;
        let _ = stop.send(());
        let received = server.await;
        let received = received.unwrap().unwrap();
        assert!(
            matches!(result, Err(BinanceRestError::ApiError { http_status, .. }) if http_status == status)
        );
        assert_eq!(received, ["GET /fapi/v2/positionRisk"]);
        assert_http(&probe, received.len());
    }
}
#[tokio::test]
async fn real_timeout_and_cancel_preserve_received_prefix_and_owned_cleanup() {
    for cancel in [false, true] {
        let (client, stop, server, arrived) = scripted(vec![(200, "[]".into())], true).await;
        let probe = Probe::new();
        let dispatch = probe.dispatch();
        let task = tokio::spawn(async move {
            robson_exec::observation::reconciliation(async {
                let result = client.get_all_open_positions().await;
                tracing::Span::current()
                    .record("outcome", if result.is_ok() { 1_u64 } else { 2_u64 });
                result
            })
            .with_subscriber(dispatch)
            .await
        });
        let reached =
            tokio::time::timeout(std::time::Duration::from_secs(3), arrived.notified()).await;
        if cancel || reached.is_err() {
            task.abort();
        }
        let result = task.await;
        let _ = stop.send(());
        let received = server.await;
        let received = received.unwrap().unwrap();
        assert!(reached.is_ok(), "real request must reach local server before cancellation");
        if cancel {
            assert!(result.unwrap_err().is_cancelled());
        } else {
            assert!(matches!(result.unwrap(), Err(BinanceRestError::Timeout)));
        }
        assert_http(&probe, received.len());
        assert_eq!(received.len(), 1);
    }
}
#[tokio::test]
async fn real_gate_refusal_reaches_neither_http_boundary_nor_server() {
    let (client, stop, server, _) = scripted(vec![(200, "{}".into())], false).await;
    let journal = OrderJournal::unpersisted();
    client.attach_gate(Arc::clone(journal.gate()));
    journal.gate().begin_closing();
    let probe = Probe::new();
    let result = measured(&probe, client.cancel_algo_order(42)).await;
    let _ = stop.send(());
    let received = server.await;
    let received = received.unwrap().unwrap();
    assert!(matches!(result, Err(BinanceRestError::Refused(_))));
    assert!(received.is_empty());
    assert_http(&probe, 0);
}

#[tokio::test]
async fn existing_spot_pagination_counts_each_real_page_without_inventing_scan_paging() {
    use robson_connectors::binance::spot::{
        SpotRead, SpotRestReader, SpotTradesQuery, MY_TRADES_PAGE_LIMIT,
    };
    let page: Vec<_> = (1..=u64::from(MY_TRADES_PAGE_LIMIT))
        .map(|id| {
            serde_json::json!({
                "symbol":"BTCUSDT", "id":id, "orderId":7, "orderListId":-1,
                "price":"100", "qty":"0.01", "quoteQty":"1", "commission":"0.001",
                "commissionAsset":"USDT", "time":1700000000000_i64, "isBuyer":true,
                "isMaker":false, "isBestMatch":true
            })
        })
        .collect();
    let (client, stop, server, _) = scripted(
        vec![
            (200, serde_json::to_string(&page).unwrap()),
            (200, "[]".into()),
        ],
        false,
    )
    .await;
    let reader = SpotRestReader::new(Arc::new(client));
    let probe = Probe::new();
    let symbol = robson_domain::Symbol::from_pair("BTCUSDT").unwrap();
    let result = measured(
        &probe,
        reader.trades(&symbol, SpotTradesQuery::Order { order_id: 7, from_id: 0 }),
    )
    .await;
    let _ = stop.send(());
    let received = server.await;
    let received = received.unwrap().unwrap();
    let result = result.unwrap();
    assert_eq!(received, [
        "GET /api/v3/myTrades fromId=0",
        "GET /api/v3/myTrades fromId=1001"
    ]);
    assert_eq!(result.pages, received.len() as u32);
    assert_eq!(result.trades.len(), usize::from(MY_TRADES_PAGE_LIMIT));
    assert!(result.next.is_none());
    assert_eq!(result.weight, result.pages * SpotRead::MY_TRADES_BY_ORDER.weight);
    assert_http(&probe, received.len());
    assert_eq!(probe.snapshot()[4], 1, "Spot is outside the fixed measured-reconciliation endpoint classes; incomplete is explicit");
}
