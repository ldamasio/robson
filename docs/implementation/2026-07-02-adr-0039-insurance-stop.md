# ADR-0039 Insurance Stop — Implementation Guide (Mission 1: core lifecycle)

Milestone: ADR-0039 core. Scope of this mission: exchange port + domain events +
engine actions + executor handling + tests. Reconciliation/startup-recovery
integration and the sizing buffer are explicitly OUT of scope (mission 2).

Read `AGENTS.md` and `docs/adr/ADR-0039-exchange-side-insurance-stop.md` first.

## Non-negotiable invariants

1. All content in English. Conventional commits are NOT your job — do not commit;
   leave the worktree dirty for review.
2. The insurance stop price is ALWAYS the position's current trailing stop
   (chart-derived technical stop). NEVER compute any stop as a percentage of
   entry price (AGENTS.md rule 6).
3. The insurance order is always `reduce_only = true`.
4. Insurance-stop failures must NEVER abort the action batch or block the
   position lifecycle — the software stop remains the primary exit. On failure:
   emit an audit event and continue.
5. Do not touch `calculate_position_size`, reconciliation_worker, or
   startup_recovery in this mission.
6. Mirror existing patterns exactly — this codebase is consistent; when in
   doubt, copy the `PlaceExitOrder`/`ExitOrderPlaced` flow.

## Build/test command (OpenSSL quirk on this workstation)

```
export PKG_CONFIG_PATH=/home/psyctl/rbx/.devbox/nix/profile/default/lib/pkgconfig
cargo test --all
```

Rust/cargo come from the devbox profile already on PATH. All tests must pass.
`cargo fmt` at the end.

## Work items

### 1. `robson-exec/src/ports.rs` — port method

Add to `ExchangePort` (near `place_market_order`, mirroring its doc style):

```rust
async fn place_stop_market_order(
    &self,
    symbol: &Symbol,
    side: OrderSide,
    quantity: Quantity,
    stop_price: Price,
    client_order_id: &str,
) -> Result<OrderResult, ExecError>;
```

Semantics: reduce-only STOP_MARKET protective order; returns the accepted
(unfilled) order. `cancel_order` already exists — reuse it.

### 2. `robson-connectors` — Binance implementation

Find the `impl ExchangePort for` block (grep `place_market_order`) and add
`place_stop_market_order`: USD-M Futures `POST /fapi/v1/order` with
`type=STOP_MARKET`, `stopPrice=<stop_price>`, `reduceOnly=true`,
`newClientOrderId=<client_order_id>`, `quantity`, `side`. Mirror the signing,
error mapping, and response parsing of the existing market-order method.

### 3. `robson-exec/src/stub.rs` — StubExchange

Implement `place_stop_market_order`: record the order (status accepted, not
filled) the same way other stub orders are recorded, return an `OrderResult`
with a generated exchange order id. Keep whatever inspection hooks the stub
already offers so tests can assert the order was placed/cancelled.

### 4. `robson-domain/src/events.rs` — new events

Add four `Event` variants, styled after `ExitOrderPlaced` (doc comments, field
naming, serde):

- `InsuranceStopPlaced { position_id, order_id: String, stop_price: Price, timestamp }`
- `InsuranceStopReplaced { position_id, previous_order_id: String, order_id: String, stop_price: Price, timestamp }`
- `InsuranceStopCancelled { position_id, order_id: String, timestamp }`
- `InsuranceStopFailed { position_id, stop_price: Price, error: String, timestamp }`

Update every exhaustive `match` on `Event`: `event_type()` (snake_case strings:
`insurance_stop_placed`, etc.), `position_id()`, timestamp accessor, and any
other arms the compiler forces. Grep for `ExitOrderPlaced` to find all sites,
including `robson-projector` dispatch (add no-op/audit handling consistent with
how other non-projected events are treated) and `robsond` SSE mapping
(`map_daemon_event`) if it matches exhaustively.

### 5. `robson-engine/src/lib.rs` — actions + emissions

New `EngineAction` variants (doc style of `PlaceExitOrder`):

- `PlaceInsuranceStop { position_id, symbol, side: OrderSide, quantity, stop_price }`
- `ReplaceInsuranceStop { position_id, symbol, side: OrderSide, quantity, previous_order_id: String, new_stop_price }`
- `CancelInsuranceStop { position_id, symbol, order_id: String }`

Emission points:

a. `process_entry_fill`: after the `EntryFilled` emit action, push
   `PlaceInsuranceStop` with `side = position.side.exit_action()`,
   `quantity = filled_quantity`, `stop_price = initial_trailing_stop`.

b. `create_update_stop_decision`: after the `UpdateTrailingStop` action — read
   `insurance_stop_id` from the position's `Active` state:
   `Some(id)` → `ReplaceInsuranceStop { previous_order_id: id, new_stop_price: new_stop, .. }`;
   `None` → `PlaceInsuranceStop` at `new_stop` (covers positions opened before
   this feature).

c. `create_exit_decision`: when `Active.insurance_stop_id` is `Some(id)`,
   insert `CancelInsuranceStop` BEFORE `PlaceExitOrder` (never leave both a
   reduce-only stop and a market exit live simultaneously).

State: `PositionState::Active.insurance_stop_id: Option<String>` — the field
exists but is typed for the old design; check its current type (grep
`insurance_stop_id`) and adapt it to `Option<String>` (exchange order id) if it
is not already, fixing all construction sites.

The engine must set/clear `insurance_stop_id` when applying the corresponding
events — find where `ExitOrderPlaced`/fill events mutate position state (grep
in `robson-engine` and `robsond/src/position_manager.rs` around
`ActionResult::OrderPlaced`) and mirror: `InsuranceStopPlaced`/`Replaced` set
the id; `Cancelled` clears it.

### 6. `robson-exec/src/executor.rs` — action handling

Handle the three new actions in `execute_action`, mirroring
`execute_exit_order`'s structure (intent journaling, tracing):

- `PlaceInsuranceStop` / `ReplaceInsuranceStop`: journal an intent; for
  Replace, first `cancel_order(previous_order_id)` (tolerate "unknown order"
  errors — the stop may have just filled; log and continue), then
  `place_stop_market_order` with
  `client_order_id = format!("ins-{}", intent_id.simple())` (36 chars — Binance
  limit). Return `ActionResult::OrderPlaced { order, event: Some(Event::InsuranceStopPlaced/Replaced {..}) }`
  with `order_id` = the EXCHANGE order id from the result.
- `CancelInsuranceStop`: `cancel_order`; tolerate "unknown order" (already
  filled/cancelled) as success-with-log. Return
  `ActionResult::EventEmitted(Event::InsuranceStopCancelled {..})`.
- ANY failure in these paths: `warn!` + return
  `ActionResult::EventEmitted(Event::InsuranceStopFailed {..})` — never
  `OrderFailed` (that aborts the batch in `Executor::execute`).

### 7. Tests (follow CONTRIBUTING.md naming: `test_<behavior>`)

Engine (in `robson-engine/src/lib.rs` tests module, reuse
`create_active_position` helper):
- entry fill decision contains `PlaceInsuranceStop` at the initial technical
  stop with exit side and filled quantity;
- trailing-stop update decision contains `ReplaceInsuranceStop` at the new stop
  when an insurance id is present, `PlaceInsuranceStop` when absent;
- exit decision contains `CancelInsuranceStop` ordered before `PlaceExitOrder`
  when an insurance id is present, and no cancel when absent.

Executor (in `robson-exec`, with `StubExchange`):
- `PlaceInsuranceStop` places a reduce-only stop order on the stub and returns
  `OrderPlaced` with `InsuranceStopPlaced`;
- failure path returns `InsuranceStopFailed` event and does not error;
- `CancelInsuranceStop` cancels on the stub.

Domain: event_type() strings for the four new events.

## Done criteria

- `cargo test --all` green, `cargo fmt` clean, no clippy regressions in touched
  files (`cargo clippy -p robson-engine -p robson-exec -p robson-domain`).
- No commits made. Print a short summary of files changed and any deviations
  from this guide with justification.
