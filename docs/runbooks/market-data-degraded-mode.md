# Market Data Degraded Mode

**Severity**: High when a risk-open position exists  
**Time to execute**: 10 minutes  
**Required access**: Robson metrics and logs, plus a diagnostic pod on the affected node

## Meaning of `STALE, reconnecting`

The UI status means the futures WebSocket has not delivered a recent market
message. It does not by itself mean that all price protection is offline.

For each risk-open symbol, `robsond` enters REST fallback after 90 seconds of
WS silence. REST prices continue through the same trailing pipeline. The
exchange-side, reduce-only insurance stop remains on Binance and does not
depend on the daemon or its WebSocket connection. If both live price paths
fail, the last insurance stop placed on the exchange remains the bounded-loss
floor, but trailing cannot advance until a price path recovers.

## Confirm the degraded mode

Check these Prometheus series for the affected symbol:

```promql
robsond_market_data_mode{symbol="BTCUSDT"}
robsond_market_data_silent_seconds{symbol="BTCUSDT"}
sum by (outcome) (rate(robsond_market_data_fallback_polls_total{symbol="BTCUSDT"}[5m]))
sum by (endpoint, reason) (increase(robsond_market_data_ws_failures_total{symbol="BTCUSDT"}[30m]))
```

Expected degraded state:

- `robsond_market_data_mode` is `1`.
- REST fallback polls with `outcome="ok"` keep increasing.
- WS failures identify the endpoint and distinguish `connect` from
  `connect_timeout` and `handshake_no_data`.
- Reconnect delays grow to a maximum of 15 minutes while REST polls remain
  healthy. A persistent fallback WARN is emitted at most once per 15 minutes.

If there is no risk-open position for the symbol, REST fallback intentionally
does not poll.

## Test egress from the affected node

Run the checks from a diagnostic pod scheduled on the same node and using the
same egress path as `robsond`. The Python image must have the `websockets`
package available.

```bash
python - <<'PY'
import asyncio
import websockets

URLS = [
    "wss://fstream.binance.com/market/ws/btcusdt@aggTrade",
    "wss://fstream.binance.com/market/ws/btcusdt@markPrice",
    "wss://stream.binance.com:9443/ws/btcusdt@trade",
]

async def check(url):
    try:
        async with websockets.connect(url, open_timeout=10) as ws:
            message = await asyncio.wait_for(ws.recv(), timeout=15)
            print(f"OK {url} {len(message)} bytes")
    except Exception as error:
        print(f"FAIL {url} {type(error).__name__}: {error}")

async def main():
    for url in URLS:
        await check(url)

asyncio.run(main())
PY
```

Also check basic TCP, TLS, and HTTP reachability. An HTTP response does not
prove that the WebSocket upgrade or stream delivery works.

```bash
curl -sv --max-time 10 https://fstream.binance.com/ -o /dev/null
curl -sv --max-time 10 https://fapi.binance.com/ -o /dev/null
```

## Configure endpoint rotation

`ROBSON_MARKET_DATA_WS_ENDPOINTS` is an ordered, comma-separated list of
market-stream base URLs. Entries must use `wss://`, contain no credentials,
query strings, or fragments, and omit the trailing `/ws` path.

```bash
ROBSON_MARKET_DATA_WS_ENDPOINTS=wss://fstream.binance.com/market
```

Append only alternate bases that have been validated and approved for the
deployment. On mainnet, the unset default is
`wss://fstream.binance.com/market`. Do not use the retired
`fstream-auth.binance.com` domain. The current Binance route mapping places
`aggTrade` under `/market`.

After changing the list, restart through the normal deployment workflow and
confirm a log containing `first stream message received` for each symbol.

## Failure modes

| Failure | Expected behavior | Operator action |
| --- | --- | --- |
| Primary WS fails, alternate works | The daemon rotates to the next configured base and logs connected only after its first message | Verify failure counts, then investigate the failed route |
| All WS bases fail, REST is healthy | REST drives trailing and WS retry backoff grows to 15 minutes | Keep the incident open and investigate egress without restarting in a loop |
| WS handshake succeeds but no data arrives | Failure reason is `handshake_no_data`; the endpoint rotates after the watchdog | Verify the routed `/market` URL and test stream delivery from the same node |
| WS and REST both fail | No new trailing input; the last exchange-side insurance stop remains active | Treat as urgent and restore at least one price path |
| Daemon stops | The exchange-side insurance stop remains active | Follow the daemon recovery procedure |

## Validation

- Confirm `robsond_market_data_mode` returns to `0` only after the WS health
  hold-down completes.
- Confirm REST fallback polling stops after recovery.
- Confirm the connected log names the endpoint and follows an actual stream
  message.
- Confirm no endpoint value contains credentials or unapproved relay URLs.

## Related documentation

- [ADR-0044](../adr/ADR-0044-rest-market-data-fallback-for-trailing.md)
- [ADR-0039](../adr/ADR-0039-exchange-side-insurance-stop.md)
- [Binance WebSocket route mapping](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Connect)
- [Binance WebSocket migration notice](https://developers.binance.com/en/docs/products/derivatives-trading-usds-futures/websocket-market-streams/Important-WebSocket-Change-Notice)
