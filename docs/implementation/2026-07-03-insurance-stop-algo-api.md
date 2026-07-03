# Insurance Stop — Binance Algo Order API migration (production bug fix)

Production incident 2026-07-03 02:26 UTC: the first live insurance-stop
placement (ADR-0039) failed with Binance error `-4120 — Order type not
supported for this endpoint. Please use the Algo Order API endpoints
instead.` Binance migrated USD-M conditional orders (STOP_MARKET et al.) to
the Algo Order API effective 2025-12-09; `POST /fapi/v1/order` now rejects
them. The fail-safe worked (position Active, software stop primary), but the
exchange-side protection is not being placed. This mission migrates the whole
insurance-stop lifecycle to the Algo endpoints.

Read `AGENTS.md`, `docs/adr/ADR-0039-exchange-side-insurance-stop.md`, and
the two `docs/implementation/2026-07-02-adr-0039-*.md` guides first.

## Non-negotiable invariants

1. English only. Do NOT commit — leave the worktree dirty for review.
2. Policy 11 unchanged: reconciled closes use `OrderFillRecord` evidence from
   the REAL triggered order, never algo-level estimates.
3. Insurance failures stay audit-only (`InsuranceStopFailed`), never abort a
   batch; the software stop remains the primary exit path.
4. `insurance_stop_id` (String) now stores the **algoId** (stringified).
   Client ids keep the `ins-` prefix, sent as `clientAlgoId`.
5. Do not touch `calculate_position_size` or engine emission logic — this is
   a connector/adapter/port-semantics migration.

## Binance Algo Order API (verified against official docs 2026-07-03)

### Place — POST /fapi/v1/algoOrder (signed)
Params for our use: `algoType=CONDITIONAL`, `symbol`, `side` (BUY/SELL),
`type=STOP_MARKET`, `quantity`, `triggerPrice` (NOT `stopPrice`),
`reduceOnly=true`, `clientAlgoId=<ins-...>`, `newOrderRespType=RESULT`,
`timestamp`. Response (subset): `algoId` (LONG), `clientAlgoId`,
`algoType`, `orderType`, `symbol`, `side`, `quantity`, `algoStatus`
(e.g. `NEW`), `triggerPrice`, `reduceOnly` (BOOLEAN), `createTime`,
`updateTime` (LONG ms).

### Cancel — DELETE /fapi/v1/algoOrder (signed)
Params: `algoId` OR `clientAlgoId`, `timestamp`. Response:
`{algoId, clientAlgoId, code, msg}`. Treat "unknown/already
triggered/cancelled" error codes as tolerated no-ops (log, continue), same
policy as today.

### Query one — GET /fapi/v1/algoOrder (signed)
Params: `algoId` OR `clientAlgoId`, `timestamp`. Response adds:
`algoStatus` (NEW/CANCELED/TRIGGERED/...), **`actualOrderId`** (STRING —
empty if not triggered; the REAL order id once triggered), `actualPrice`,
`actualQty`, `actualType`, `triggerTime`.

### List open — GET /fapi/v1/openAlgoOrders (signed)
Params: `symbol` (optional; omit = all symbols, weight 40 vs 1), `timestamp`.
Response: array with `algoId`, `clientAlgoId`, `orderType`, `algoType`,
`algoStatus`, `side`, `symbol`, `triggerPrice`, `reduceOnly` (BOOLEAN),
`quantity`, `createTime`, `updateTime`.

## Work items

### 1. `robson-connectors/src/binance_rest.rs`

- Rewrite `place_stop_market_order` → `POST /fapi/v1/algoOrder` per spec
  above (rename the `stopPrice` param to `triggerPrice`, add
  `algoType=CONDITIONAL`). New response struct `BinanceAlgoOrderResponse`
  (serde, camelCase) with the fields listed. Keep the method name.
- Add `cancel_algo_order(algo_id: i64)` → `DELETE /fapi/v1/algoOrder`.
- Add `query_algo_order(algo_id: i64)` → `GET /fapi/v1/algoOrder`, struct
  `BinanceAlgoOrderDetail` including `actualOrderId: String` (may be empty),
  `algoStatus`, `actualPrice`, `actualQty`.
- Add `get_open_algo_orders(symbol: &str)` → `GET /fapi/v1/openAlgoOrders`.
- Parse tests for each response (mirror `test_open_order_response_parsing`).

### 2. `robson-exec/src/ports.rs` — port surface

- `place_stop_market_order`: unchanged signature; document that the returned
  `OrderResult.exchange_order_id` carries the **algoId**.
- Add `async fn cancel_stop_market_order(&self, symbol: &Symbol, algo_id: &str) -> Result<(), ExecError>;`
  (insurance cancels must NOT go through `cancel_order`, which targets
  /fapi/v1/order).
- Add `async fn get_stop_order_fill(&self, symbol: &Symbol, algo_id: &str) -> Result<Option<OrderResult>, ExecError>;`
  — queries the algo order; when `actualOrderId` is non-empty, resolves the
  REAL order via the existing by-order-id query and returns its
  `OrderResult` (this is the Policy-11 evidence path). `Ok(None)` when not
  triggered.
- `get_open_orders`: keep the name/signature, but implementations must now
  return open ALGO orders (the insurance sweep/heal is its only consumer —
  verify with grep and note in the doc comment). `OpenOrderRecord`:
  `exchange_order_id` = algoId string, `client_order_id` = clientAlgoId,
  `stop_price` from `triggerPrice`.

### 3. `robsond/src/binance_exchange.rs` — adapter

- `place_stop_market_order`: call the new REST method; success is
  `algoStatus == "NEW"`; map `algoId` → `OrderResult.exchange_order_id`
  (string). Other statuses → Err (audit path), as today.
- Implement `cancel_stop_market_order` (parse algo_id to i64; tolerate
  unknown-order style errors by returning Ok with a warn — match the current
  tolerance policy in the executor, keeping the executor's behavior
  unchanged if it already tolerates).
- Implement `get_stop_order_fill` per the port doc.
- `get_open_orders`: switch to `get_open_algo_orders`.

### 4. Callers

- `robson-exec/src/executor.rs`: insurance cancel paths (replace + cancel)
  call `cancel_stop_market_order` instead of `cancel_order`.
- `robsond/src/reconciliation_worker.rs` `gather_order_fill_evidence`: use
  `get_stop_order_fill` (the stored id is an algoId; querying
  `get_order_by_exchange_id` with it is now wrong).
- `robsond/src/startup_recovery.rs` heal: fill check via
  `get_stop_order_fill`; the open-order check keeps using
  `get_open_orders` (now algo-backed).
- `robson-exec/src/stub.rs`: keep one stop-order model but align semantics:
  ids returned by placement are "algo ids"; `fill_stop_order` should make
  `get_stop_order_fill` return the fill and remove it from open orders;
  `cancel_stop_market_order` removes it. Update the robsond mocks
  (grep `place_stop_market_order`) with the two new methods.

### 5. Tests

- Update/extend existing insurance tests to the new port methods (executor
  cancel path, reconciliation fill evidence, orphan sweep, heal branches) —
  they should keep passing with the stub realigned.
- New: connector parse tests (item 1); adapter status mapping test if the
  existing test structure allows.

## Build/test

```
export PKG_CONFIG_PATH=/home/psyctl/rbx/.devbox/nix/profile/default/lib/pkgconfig
cargo test --all      # must be green
cargo +nightly fmt
```

## Done criteria

Green suite, nightly fmt clean, no new clippy diagnostics in touched crates.
No commits. Print changed files + deviations with justification.
