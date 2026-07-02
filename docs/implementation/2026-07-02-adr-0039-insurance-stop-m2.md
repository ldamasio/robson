# ADR-0039 Insurance Stop — Implementation Guide (Mission 2: reconciliation + recovery)

Prerequisite: mission 1 is merged in this branch (commit `b528f37d`) — read
`docs/implementation/2026-07-02-adr-0039-insurance-stop.md` and the mission-1
diff (`git show b528f37d --stat`) before starting. Also read `AGENTS.md`,
`docs/adr/ADR-0039-exchange-side-insurance-stop.md`, and
`docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`.

## Non-negotiable invariants

1. English only. Do NOT commit — leave the worktree dirty for review.
2. Policy 11: automated reconciled closes accept ONLY `OrderFillRecord` /
   `UserTradeRecord` evidence. Never introduce `Estimated` evidence.
3. Rule 8 / ADR-0022: every insurance order is robsond-authored. The
   `ins-` client-order-id prefix (mission 1) identifies them. An open
   insurance order whose position is not tracked-open is an orphan and MUST
   be cancelled by reconciliation.
4. Insurance-stop maintenance failures must never abort recovery or
   reconciliation scans — log + audit event (`InsuranceStopFailed`), continue.
5. Do not touch `calculate_position_size` (mission 3, not yours).

## Build/test

```
export PKG_CONFIG_PATH=/home/psyctl/rbx/.devbox/nix/profile/default/lib/pkgconfig
cargo test --all        # must be green
cargo +nightly fmt      # repo rustfmt.toml uses unstable options
```

## Work items

### 1. Port: open-orders query

`robson-exec/src/ports.rs` — add to `ExchangePort`:

```rust
/// Query currently open (unfilled) orders for a symbol.
async fn get_open_orders(&self, symbol: &Symbol) -> Result<Vec<OpenOrderRecord>, ExecError>;
```

Define `OpenOrderRecord { exchange_order_id: String, client_order_id: String, order_type: String, reduce_only: bool, stop_price: Option<Price>, side: OrderSide }`
(follow the serde/doc style of `OrderResult`). Implement in:
- `robson-connectors/src/binance_rest.rs`: `GET /fapi/v1/openOrders?symbol=`
  (signed), mirroring existing GET request/signing patterns.
- `robsond/src/binance_exchange.rs` adapter.
- `robson-exec/src/stub.rs`: return stub-recorded stop orders that are still
  open; keep hooks so tests can mark one filled/cancelled.
- The robsond test mocks that implement `ExchangePort` (grep
  `place_stop_market_order` to find all of them) — return `Ok(vec![])` or
  delegate, matching each mock's existing style.

### 2. Reconciliation: insurance-fill close (verify + test)

`robsond/src/reconciliation_worker.rs` already resolves fill evidence from the
insurance order id (`gather_order_fill_evidence`). Add tests proving the
end-to-end path with mission-1 shapes:
- Active tracked position, exchange position gone, insurance order FILLED on
  the stub → reconciled close using `OrderFillRecord` evidence (assert the
  close reason/evidence type and final position state).
- Insurance order still open/NEW and exchange position gone → no automated
  close (existing blocker behavior preserved).

### 3. Reconciliation: orphan insurance orders

In the reconciliation scan (find where UNTRACKED exchange positions are
handled), add an orphan sweep per scanned symbol:
- `get_open_orders(symbol)`; for each order with `client_order_id` starting
  with `"ins-"` and `reduce_only == true` whose exchange order id does not
  match any tracked-open position's `insurance_stop_id` → `cancel_order`,
  `warn!` log, and emit `Event::InsuranceStopCancelled` for audit.
- Tolerate cancel errors (log, continue scan).
- Test: orphaned stub stop order gets cancelled; a stop order belonging to a
  tracked Active position is NOT cancelled.

### 4. Startup recovery: verify/heal the insurance stop

In `robsond/src/startup_recovery.rs`, after `replay_candles` decides the
position STAYS OPEN (the `!should_close_now` branch of `replay_candles`, or
just after it returns `false` in the caller — pick the seam that keeps
`replay_candles` cohesive), heal the protective stop:

- `insurance_stop_id = Some(id)`: query the order
  (`get_order_by_exchange_id`):
  - FILLED during the gap → reconcile-close the position from that
    `OrderFillRecord` evidence (reuse the reconciliation close path — do NOT
    place a new market exit; the position is already closed on the exchange).
  - Cancelled/missing → re-place at the CURRENT post-replay trailing stop via
    the engine/executor path (`PlaceInsuranceStop` action through the
    executor, not a direct exchange call).
  - Still open at a stop price different from the post-replay trailing stop →
    `ReplaceInsuranceStop` at the current trailing stop.
- `insurance_stop_id = None` (positions predating ADR-0039) → place one at the
  current trailing stop.
- All of this tolerates failures per invariant 4.
- Tests: one per bullet above, using the existing startup_recovery test
  harness (grep `mod tests` in startup_recovery.rs for the fixtures).

### 5. Operator visibility

`robsond/src/api.rs` `map_daemon_event`: if it matches event types explicitly,
map the four insurance events into the public SSE stream (mirror how
`TrailingStopUpdated` is exposed). If it already passes unknown events through
or ignores them by design, leave it and note that in your summary.

### 6. Docs alignment (AGENTS.md rule 5)

- `docs/architecture/v3-control-loop.md` and `docs/architecture/v3-runtime-spec.md`:
  wherever they state that no exchange-side protective order exists ("Robson
  manages exits", insurance stop absent), update to describe the implemented
  behavior, referencing ADR-0039. Keep edits surgical — do not rewrite
  sections wholesale.
- `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`: add the orphan
  insurance-order sweep (robsond-authored `ins-` prefix, cancel rule).

## Done criteria

- `cargo test --all` green; `cargo +nightly fmt` clean; no new clippy
  diagnostics in touched crates vs this branch's HEAD.
- No commits. Print a summary of files changed, tests added, and any
  deviations with justification.
