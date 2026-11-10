# ADR-0056 — Core Risk Policy vs Venue Adapters: Port Contracts, Adapter Relocation, Multi-Venue Execution Model

**Date**: 2026-11-04 (v5; v1–v4 reviewed by Codex gpt-6.1-sol xhigh: 15 + 14 + 9 + 4 findings; v5 APPROVED with one P3 nit applied, see Review trail)
**Status**: ACCEPTED 2026-11-05 (operator: N1-a chosen; single-venue spot on the personal account first; N2–N4 remain open until a second venue is added; N5 must be resolved before step 8 activation, with §2.5's freshness and fee-asset requirements enforced from the first spot admission)
**Deciders**: RBX Systems (operator + architecture)
**Amends**: [ADR-0024](ADR-0024-trading-policy-layer.md) §6 (capital base source becomes venue-neutral and names its valuation),
[ADR-0039](ADR-0039-exchange-side-insurance-stop.md) (the protective stop becomes a venue contract with an explicit lifecycle).
**Conditionally amends, if N1-a is chosen**: ADR-0039's protection-continuity guarantee and ADR-0052's two-layer same-trigger
enforcement claim do **not** hold on balance accounts during a protection transition (§2.2). Risk *numbers* are untouched;
those two *guarantees* are qualified.
**Preserves**: ADR-0021 (level selection), ADR-0022 (unauthored inventory on the operated account is closed, account-wide; no
exception), ADR-0050 (valid-level selection, no-valid-stop semantics, executable risk costing), ADR-0051 (net-from-start
monthly budget, atomic reservation, cost-priced boundary downside), ADR-0049 (the event log stays an audit trail; existing
payloads do not change, except step 6's optional `rules_snapshot` and its new audit-only `residual_inventory_recorded`, §5)
**Related**: issue #154, TD-2026-05-05-001, docs/policies/UNTRACKED-POSITION-RECONCILIATION.md, 2026-10-30 security assessment

---

## Context

On 2026-11-04 Binance removed the `Enable Futures` permission from the production API key and refuses to re-enable it
("not available in your region"). Its futures UI states that futures are available on ADGM accounts only and that Binance
cannot offer derivatives to residents of Brazil (Lei 6.385/1976). CVM has stated that no crypto venue is authorised to
offer derivatives to Brazilian investors and that the BCB SPSAV licence does not cover them. Robson's only execution path,
Binance USDⓈ-M perpetuals, is closed for a Brazilian operator (CPF or CNPJ). The daemon still reads balances and income;
every order fails with `-2015`.

Operator direction: long-only spot as the next execution path; hedge/short adapters outside Binance later (B3 bitcoin
futures is the only regulated derivative venue for Brazilians); and, as a standing goal, one logical position executed
across several operator-selected venues with the capital distributed between them.

The risk core is venue-neutral in substance (level selection, executable span, adverse-fill costing, monthly budget,
hand-span trailing stop never ask which exchange they run on). The leaks are of shape. Survey of 2026-11-04, confirmed line
by line by the Codex reviews:

| # | Leak | Where |
|---|------|-------|
| 1 | `ExchangePort` is modelled on Binance USDⓈ-M with spot methods mixed in (`validate_futures_settings`, `get_futures_balance`, `get_all_open_positions`, `close_position_market`, `universal_transfer`, `get_transfer_history`, `get_spot_*`, `place_spot_market_order`) | `robson-exec/src/ports.rs` |
| 2 | Production adapters live in the daemon; the daemon is built on the concrete type (`Daemon<BinanceExchangeAdapter, _>`, `new_binance*`) | `robsond/src/binance_exchange.rs`, `binance_ohlcv.rs`, `daemon.rs:359-442` |
| 3 | Market data is constructed on the futures WebSocket client outside any factory; `MarketData` events, detectors and the price cache are keyed by symbol only | `robsond/src/market_data.rs:37,53,355`, `event_bus.rs:203`, `position_manager.rs:174` |
| 4 | The position manager imports a quantisation helper from the adapter; the helper hard-codes a `0.001` step and checks only the minimum; ordinary market orders already prefer metadata with a fallback quantity path | `robsond/src/position_manager.rs:58,2225`, `binance_exchange.rs:40-50,87,391,556` |
| 5 | `binance_position_id` is the name of the persisted event field, the entity field (independent serde derive), a legacy SQL column and the API/frontend field; the active projector's `EntryFilled` payload does not carry it | `robson-domain/src/events.rs:251`, `entities.rs:42,73`, `migrations/20240101000005_add_binance_position_id.sql`, `frontend/src/lib/api/robson.ts:59`, `robson-projector/src/types.rs:191` |
| 6 | The domain `Exchange` enum carries endpoints; its serialized value is the credential identity (AES-GCM AAD, store queries, SQL CHECK) | `robson-domain/src/credentials.rs:90,149`, `robson-store/src/credential_store.rs:459`, `migrations/20240101000004_exchange_credentials_multi_tenant.sql:20` |
| 7 | Binance wire encodings in the port layer (`as_binance_str`) | `robson-exec/src/ports.rs:74,603` |
| 8 | Execution costs default to the Binance futures taker rate and are read as one scalar; accounting adds scalar fees to quote P&L although the port allows other fee assets | `robson-domain/src/value_objects.rs:302`, `executable_stop.rs:345`, `robsond/src/position_manager.rs:4124` |
| 9 | Reconciliation, startup recovery, live-risk filtering, orphan-stop recognition and `/status` sync all start from `get_all_open_positions()` and `reduce_only` | `robsond/src/reconciliation_worker.rs:379,413,580`, `daemon.rs:1064`, `position_manager.rs:420,3699`, `api.rs:1408` |
| 10 | The executor's stop replacement (place-then-cancel) and close (`reduce_only = true`) rely on derivatives semantics | `robson-exec/src/executor.rs:440,552` |
| 11 | The funding saga uses spot balances, prices, symbol availability, order lookup by client id and base/quote orders, then refreshes capital from the futures wallet; the worker is constructed from DB/config presence | `robsond/src/funding/saga.rs:70,160,249,303,389`, `daemon.rs:733` |
| 12 | The intent journal is in-memory (`RwLock<HashMap>`), recreated at daemon construction; crash recovery today relies on venue observation, not on journal evidence | `robson-exec/src/intent.rs:134` |
| 13 | The connector maps the futures `balance` field straight into `wallet_balance` (unrealized P&L not included); the month-boundary helper subtracts entry-to-stop distance clamped at zero, not ADR-0051 §3's cost-priced downside from the boundary mark | `robson-connectors/src/binance_rest.rs:709`, `robsond/src/daemon.rs:1004` |

None of these touch a risk number. All of them make a second venue expensive, and 9–13 would make a spot venue *unsafe*
if the split simply made the futures-only methods optional.

### Target picture

```text
            operator (API / UI)
                  │  arm LogicalOperation{symbol, side, legs: [(venue, risk weight)]}
                  ▼
   ┌──────────────────────────────┐
   │  risk core (venue-neutral)   │  ADR-0021/0050/0051/0052: levels, span, costing,
   │  admission · budget · stops  │  atomic reservation, hand-span trailing stop
   └──────────────┬───────────────┘
                  │ per leg: plan {qty, executable stop, cost envelope, rules snapshot}
      ┌───────────┼─────────────────┐
      ▼           ▼                 ▼
 Venue binance:usdm   Venue binance:spot     Venue b3:btc-fut (later)
 ┌──────────────┐   ┌──────────────┐        ┌──────────────┐
 │ Trading      │   │ Trading      │        │ Trading      │   required
 │ Account      │   │ Account      │        │ Account      │   required
 │ PositionAcct │   │ Inventory    │        │ PositionAcct │   exactly one, by account kind
 │ Settlement   │   │ Settlement   │        │ Settlement   │   required (from fills if no income API)
 │ Funding      │   │ —            │        │ —            │   optional
 │ MarketData   │   │ MarketData   │        │ MarketData   │   required, venue-scoped feed
 └──────────────┘   └──────────────┘        └──────────────┘
```

---

## Decision

### 1. Vocabulary

- **Venue** = exchange × market kind × account scope. `VenueId` is a stable string (`binance:usdm`, `binance:spot`,
  `b3:btc-fut`) used in config, logs, `/status`, allocations, market-data events (step 3) and, later, on positions.
- **Market kind**: `PerpUsdM` (position account, funding, reduce-only) or `Spot` (balance account, holdings, protection
  is an order on the book). Dated futures later.
- **Account kind**: `PositionAccount` (the venue reports positions) or `BalanceAccount` (the venue reports holdings only;
  Robson derives its governed position from its own fills).
- **Execution account**: the account a venue trades on. On balance accounts it is a **dedicated account scope**: a
  Binance sub-account on the corporate account, a separate account on a venue without sub-accounts, or, during Fase S,
  the exclusive personal spot wallet specified in §8. It holds only Robson-governed inventory and quote cash; separate
  API keys do not isolate shared inventory and do not qualify. ADR-0022 applies unchanged across the entire operated
  scope: any unauthored holding of any asset that appears there is closed per
  `docs/policies/UNTRACKED-POSITION-RECONCILIATION.md`. The operator's own holdings never live in an execution scope;
  sharing one is out of scope (would require a policy amendment).
- **Governed holding**: base-asset inventory bought by a leg, net of base-asset commission, as evidenced by fills.
- **Leg**: the execution of a logical operation on one venue. Today every position is a single leg on `binance:usdm`.

### 2. Port contracts (replace `ExchangePort`)

All traits are `#[async_trait]` with `Send + Sync` supertraits; optional capabilities are borrowed accessors on the venue
object. A runtime-selected venue is a sized dispatch enum in `robsond` (`VenueRuntime { Usdm(..), Spot(..), Stub(..) }`)
forwarding to the generic paths; `Executor<E: Venue, S: Store>` stays generic. The legacy `ExchangePort` is kept for one
release as a **forwarding compatibility layer** (same method names, implemented over the new traits), not as an alias.

```text
TradingPort                      required
  trading_rules / trading_rules_preview                      (issue #154 contracts, qualified by step 6: §5)
  trading_rules_fresh / last_validated_rules / rules_refresh_due / refresh_trading_rules     (step 6: §5)
  place_market_order(symbol, side, qty, client_id, intent: EntryOrReduce) -> OrderResult
  place_protection(symbol, close_side, qty, trigger, client_id) -> PlacedStop { stop: StopRef, acceptance: OrderResult }
  cancel_protection(symbol, StopRef) -> StopCancelOutcome
      StopCancelOutcome = { Cancelled{filled_before_cancel: Decimal, fills}, AlreadyGone{final_state: OrderLookup}, Unknown }
  replace_protection(symbol, old: StopRef, new: StopParams) -> ReplaceOutcome
      ReplaceOutcome = { Replaced{new: PlacedStop, old_filled: Decimal}, OldKeptNewRejected{reason},
                         OldCancelledNewRejected{old_filled, reason},
                         TriggerAlreadyCrossed{old: OrderLookup, residual: Decimal}, Unknown{new: Option<PlacedStop>} }
      TriggerAlreadyCrossed is a venue rejection of the new trigger, not execution evidence; `old` is the old stop's
      final state with authoritative cumulative fills and `residual = governed − old_filled`.
      A replacement reports a rejection only when every attempt on record for the successor's client id was
      authoritatively refused; any other placement failure is Unknown{new: None}. Unknown{new: Some(..)} means the
      successor is accepted and the old stop's state is what is unknown
  order_by_client_id(symbol, client_id) -> OrderLookup
      OrderLookup = { Absent{finality: VenueFinalityRule}, Open{filled_qty}, PartiallyFilled{..},
                      Terminal{filled_qty, cancelled_qty, fills}, Unknown }
      Absent is returned only under the venue's finality rule (below), never because time has passed; a raw not-found
      response is Unknown
  order_by_exchange_id / protection_fill(symbol, StopRef) / open_protective_orders
  user_trades_since(symbol, since, limit) / price / health

AccountPort                      required
  cash(asset) -> Cash { free, locked, as_of }
  valuation(symbol) -> Mark { price, as_of }
  execution_costs(symbol) -> ExecutionCostEnvelope { maker_bps, taker_bps, gap_bps, fee_asset_policy,
                                                     funding: Option<FundingTerms>, as_of, source }
  capabilities() -> VenueCapabilities

PositionAccountPort              required when account_kind = PositionAccount
  validate_position_settings(symbol, expected_leverage = 1)  (One-way)
  balance() / open_positions()
  position_snapshot() -> { wallet_balance, unrealized_pnl, positions: Vec<ExchangePosition>, as_of }   built from those two
  close_position_market(symbol, side, qty, client_id)

InventoryPort                    required when account_kind = BalanceAccount
  account_observation() -> { holdings: Vec<(Asset, Holding{free, locked})>   every nonzero holding, all assets,
                             open_orders: Vec<AccountOpenOrder{symbol, order}>, independent of configured symbols
                             as_of }
  governed_inventory(symbol) -> GovernedInventory{qty, fill_ids}            derived from authoritative fills

SettlementPort                   required (every venue)
  settlement_since(since, limit) -> Vec<SettlementRecord>  realized pnl, commission (amount, asset), funding
                                                         where applicable; derived from authoritative fills and
                                                         account records when the venue has no income endpoint

FundingPort                      optional (spot→derivatives conversion feature, position accounts only)
  spot_balances / spot_price / spot_symbol_is_trading / place_spot_market_order / spot_order
  transfer / transfer_history

TradeFeedPort, OhlcvPort         required; built by the same factory, one port per venue. TradeFeedPort is one connection
                                 attempt, because the venue-neutral reconnect loop, watchdog and REST fallback stay in
                                 robsond and must see each attempt. The market data manager stamps its VenueId on MarketData
                                 (FeedKey { venue, symbol }); PriceUpdate and Candle stay venue-free. The older
                                 MarketDataPort is not used by the daemon

trait Venue: TradingPort + AccountPort {
  fn id(&self) -> &VenueId;
  fn position_account(&self) -> Option<&dyn PositionAccountPort>;
  fn inventory(&self) -> Option<&dyn InventoryPort>;
  fn settlement(&self) -> &dyn SettlementPort;
  fn funding(&self) -> Option<&dyn FundingPort>;
}
```

`VenueCapabilities` is data: `market_kind`, `account_kind`, `supports_short`, `reduce_only_wire_flag`,
`protection: ProtectionKind`, `replacement: ReplacementProtocol`, `has_funding`, `order_finality: VenueFinalityRule`. The executor and the daemon branch on
capabilities, never on the venue id. The conformance suite (§10) checks that flags and accessors agree.

**Order finality.** The one rule is `VenueFinalityRule::EveryAttemptRejected`: a not-found is `Absent` only when every
submit attempt on record for the client id ended in an authoritative venue rejection. Elapsed time is not an input: a
signed-request window bounds when a request can still be accepted, not when an accepted order becomes visible to a
lookup, and no venue used here documents a visibility bound. A not-found therefore stays `Unknown` for as long as an
attempt is unresolved, and may stay `Unknown` indefinitely (see step 7, §8).

The evidence is a `SubmitAttemptLedger` kept by the adapter. An attempt is on record before its request can be sent and
starts unresolved; its only transition is an authoritative rejection, so an accepted, indeterminate, in-flight or dropped
attempt blocks `Absent` for that client id whatever happened to other attempts with it. A lookup reads the evidence after
its not-found responses have returned. The ledger is bounded: a new client id at capacity drops all evidence and every
not-found reads `Unknown` for the rest of that instance's life, because per-entry eviction would let a rejected retry
look like a fresh record. What counts as an authoritative rejection is per venue and is a narrow allow-list. For
`binance:usdm` it is HTTP 400 with `-2010`, `-2019`, `-2021` or `-2022`, excluding every other status and the codes
Binance documents as execution-status-unknown. This reads the codes' documented meanings; Binance gives no explicit
finality guarantee, and the HTTP condition is an inference that fails safe to `Unknown`.

The adapter's ledger is in memory and speaks for one process. With the order journal attached (step 7, §8) the finality
rule reads the durable attempt rows and the journal's memory instead: at least one row, all rejected, nothing unlanded in
memory, then the venue's not-found. A send with no row reads `Unknown` for ever, and so does a failed read. A daemon with
no database has no journal and keeps the in-memory limitation, and no production path acts on `Absent` yet.

**2.1 Protection kind.** An eligible venue must provide exchange-resident protection that triggers **market** execution at
the resolved executable trigger: `ProtectionKind::MarketOnTrigger`. Binance USDⓈ-M: conditional `STOP_MARKET`. Binance
spot: `STOP_LOSS` (market on trigger per Binance's trading API documentation; `exchangeInfo.orderTypes` for BTCUSDT lists
it on 2026-11-04, which establishes symbol availability, not account authorisation: both are verified at venue
activation and recorded with a timestamp). A venue whose only protection is stop-limit is ineligible under
ADR-0039/0052.

**2.2 Protection continuity on balance accounts (operator decision N1).** On a balance account the open stop **locks**
the inventory, so a second full-quantity stop cannot coexist with it, and no Binance spot primitive replaces a stop
atomically: `cancelReplace(STOP_ON_FAILURE)` only guarantees that a *failed cancel* prevents the new placement; a
successful cancel followed by a rejected placement (HTTP 409) leaves no stop. The same holds for a close: the stop must
be cancelled before the governed quantity can be sold. Therefore every replacement and close on a balance account is a
**non-atomic transition** during which the inventory has no exchange-resident protection. This ADR puts two options to
the operator and recommends the first, with the residual risk stated in full:

- **N1-a (recommended)**: amend ADR-0039 with a *non-atomic transition exception for balance accounts*. In the
  successful, uninterrupted path the exposure lasts from the accepted cancel until the successor protection is accepted
  or the residual inventory is authoritatively sold (one venue round-trip). **There is no guaranteed maximum duration**
  during a rejection, an unknown execution outcome, venue API unavailability or daemon downtime. The mitigations are
  best-effort, not bounds: immediate retry of the placement with the same `client_id`, a market **flatten** of the
  governed residual after `ROBSON_PROTECTION_RETRY_MAX` (proposed 3) failures or when the venue is unreachable, and
  re-protect-or-flatten as the **first** recovery action on that leg. Issuing a flatten is not evidence of flattening;
  the transition completes only on confirmed protection or authoritative fills. **During the exposure, loss may exceed
  the priced execution envelope and the per-trade, global monthly and per-venue caps**: a long spot leg can lose the
  entire value of its governed holding. ADR-0052's two-layer same-trigger enforcement does not hold during the
  transition. Admission on the venue is blocked while it has unresolved unprotected inventory. Required mechanics: the
  transition intent is persisted **before** the cancel is sent (§8 step 7); an `unprotected_inventory` gauge per venue
  (quantity, notional, age) is exported and alerted on while nonzero, in addition to the completed-transition event
  with its duration (ADR-0044 visibility). `ReplacementProtocol::CancelThenPlace`.
- **N1-b**: keep ADR-0039 as is; Binance spot (and every balance account) is ineligible. Consequence: no spot path for a
  Brazilian operator; the direction of this ADR is reduced to the port split and to position-account venues (B3).

**Arbitration (both account kinds).** Protection and close transitions on the same governed inventory are serialized
(one in-flight transition per leg). The residual quantity to sell or re-protect is computed **after** the cancel from
the stop's final lifecycle state and cumulative fills (`StopCancelOutcome`, `ReplaceOutcome`), never from a lookup taken
before the cancel (the stop can fill in between). An `Unknown` outcome reconciles the original request to a final state
before any further submission; the leg stays pending with its reservation held. Typed transitions cover partial
cancellation with partial fill, a replacement whose trigger is already crossed, and a partially executed flatten
(residual re-protected or re-flattened). `TriggerAlreadyCrossed` is **not** execution evidence: the old stop may have
zero or partial fills (cancel accepted, then price crossed the new trigger before placement). The executor establishes
the old stop's final state and authoritative cumulative fills, computes the residual, and if it is positive enters the
governed market-flatten path immediately, retaining any still-live protection until its cancellation is required; a
zero residual completes the exit only on authoritative fills, with unsettled costs still charged; unknown outcomes and
unexecutable residuals stay visibly unresolved. Reusing a
`client_id` does not establish an earlier attempt's outcome: a rejected retry after an unresolved attempt is still
`Unknown`. A `Terminal` outcome releases only the liabilities
proven extinguished: filled inventory and unsettled costs stay charged until settlement evidence arrives.

**Exit order on position accounts (step 6).** `binance:usdm` keeps the order it runs in production. The tick's exit cancels the
protective stop, then submits the market exit (the 2026-07-02 ADR-0039 implementation guide). `DELETE` and `/panic` submit only
the exit and never cancel: a failed one keeps the stop, and a successful one leaves it live until the next sweep. So §5's "protection is retained until the successor is accepted" holds for
stop replacement, which is place-then-cancel, and **not** for the tick's market exit, and this qualifies ADR-0039 and ADR-0052's
availability language for that window. Nothing has shown that Binance accepts a reduce-only market exit beside an open reduce-only
stop, and an exit-first design with a cancel-then-resubmit fallback was tried and withdrawn (Review trail, v11). The cost: a tick
exit that fails leaves the leg with no exchange stop until the heal re-places it, and §5 states that window. On this venue the
remainder is not computed from the stop's final state as above: the reduce-only bound caps the order and its fill closes the leg
(§5). Balance accounts also cancel first, for their own reason: the stop locks the inventory the exit sells.

**2.3 Reconciliation is never optional.** The reconciliation worker, startup recovery, the live-risk filter, orphan-stop
recognition and `/status` sync consume an `AccountObservation` produced by the venue: `Positions(snapshot)` for position
accounts, `Inventory(account_observation)` for balance accounts, in both cases **account-wide** (every symbol, every
asset, every open order in the operated account scope, independent of configured or tracked symbols). Governed inventory
is identified through authoritative fills; everything else is unauthored and is closed (ADR-0022, policy unchanged).
A residual that cannot be executed (below the lot or notional minimum) is a visible unresolved liability with durable
evidence, priced conservatively at the venue mark, never declared closed. A discrepancy between governed inventory and
Robson's leg quantity blocks new legs on that venue and surfaces in `/status`. The existing gap (matching by symbol/side
rather than order-level authorship) stays a follow-up.

**What step 6 delivers on `binance:usdm`, narrower than the above.** A fill for a leg's exit or stop closes the leg, as it did
before this step, and what the venue still holds is unauthored inventory, closed under ADR-0022 on the next scan. So on this venue
a leg can be declared closed with a remainder on the account; the remainder itself is never declared closed, and one below the
lot fails every scan visibly until the operator clears it. There is no operator API. ADR-0022's definition of authored inventory
rests on entry traceability and this remainder is traceable, so this is an extension of the sweep, recorded in the UNTRACKED policy's
implementation boundary. A residual on a leg that stays open (a rounded
stop, a stop below the lot, an exit that is not executable) is durable evidence and blocks new admissions, but it is **not priced at
the mark in this step**: neither the budget nor latent risk reads it. It surfaces as `residual_quantity` on the position summary and
as the admission refusal, not in the `/status` slots, and `/positions?month=` under PostgreSQL reads the projection and omits it. Pricing it, and what a remainder means on a balance account, are step 8
obligations (Implementation Notes).

**2.4 Capital quantities.** `AccountPort`/`PositionAccountPort` expose **wallet balance and unrealized P&L
separately** (the connector's `wallet_balance` mapping is left as is; a new field is added). The core derives once:
- `cash.available`: cap for new entries (ADR-0024 capital cap);
- **governed equity**: position accounts: wallet balance + unrealized P&L; balance accounts: quote cash (free + locked)
  + Σ governed inventory × `Mark`;
- **persisted monthly budget basis** (ADR-0024 §6, ADR-0051 §3): governed equity at the boundary **minus the cost-priced
  downside of every carried leg from the boundary mark through its executable stop and costs**, persisted per leg, reset
  atomically. This replaces the current entry-to-stop clamp (`daemon.rs:1004`) and is its own behaviour-changing step
  (§8 step 5); until then the basis keeps today's derivation.
Numeraire is the venue's quote asset; this ADR admits USDT-quoted venues only (N4). A `Mark` or `Cash` older than
`ROBSON_VALUATION_MAX_AGE` (proposed 60 s) marks the basis invalid, as an unreachable venue does today.

**2.5 Execution costs drive sizing.** `ExecutionCostEnvelope` replaces the single `RiskConfig` taker rate per leg:
admission, latent risk and the executable worst-loss per unit (ADR-0050 §3) read the venue envelope; a missing or stale
envelope (`as_of` older than `ROBSON_COSTS_MAX_AGE`, proposed 24 h) fails admission closed. `fee_asset_policy` states
whether commissions settle in quote, in base (the governed quantity is net of it) or in a third asset (converted at the
venue mark at fill time, recorded with provenance). Settlement evidence is mandatory on every venue; rebates are negative
commission.

### 3. Adapter relocation and construction by configuration

- `robsond/src/{binance_exchange,binance_ohlcv}.rs` (step 1) and the futures WebSocket wiring in `market_data.rs`
  (endpoints, subscription and wire decoding, step 3) move to `robson-connectors/src/binance/usdm/` (mechanical;
  `normalize_market_quantity` moves unchanged).
- `Daemon<E: Venue + 'static, S: Store + 'static>`; `Daemon::with_venue(config, venue, market_data, ohlcv)`. A
  `VenueFactory` in `robsond` is the only place that names a concrete adapter; it builds the venue, its feeds and its
  credential scope from `ROBSON_VENUE` (`binance_usdm` today, `binance_spot` next, `stub` for tests). **New** funding sagas and the
  funding endpoints that create them are constructed only when the selected route exposes `venue.funding()`. Every funding
  saga persists its original credential/account route; at startup a **recovery worker is constructed per durable
  unfinished saga route**, even when that route is disabled for new execution and even when `ROBSON_VENUE` now selects a
  venue without funding. A saga whose original credentials are missing stays visibly blocked with its liabilities
  retained. Governed execution inventory is excluded from conversion and eligible conversion quantities are reserved
  against concurrent execution. Test: switch to a venue without funding while an older saga is unfinished.
- `/readyz` keeps `binance_api` and adds `venue: { id, kind, eligible, reasons[] }` (additive; readiness consumers that
  only read `status` see no change). `eligible` means execution and accounting eligibility: rules, costs, valuation and
  account observation fresh; today's adapter `health_check` (a public ping) is not sufficient.
- **Feed isolation** (prerequisite for a second venue): `MarketData` events, detector subscriptions and the price cache
  are keyed by `(VenueId, Symbol)`; the existing watchdog, its 90 s default and its fallback-exit hysteresis
  (`market_data.rs:68,253`) are preserved unchanged by the relocation and become per venue; a leg's technical analysis
  uses its own venue's OHLCV. No feed timing changes in this ADR's mechanical steps. A reconnect resync from a REST
  snapshot anchored at the last applied tick is required, but is staged below because it does change feed behaviour.

**What step 3 delivers and what is staged.** Each staged item has an owner and blocks that owner's step:

- **Funding route.** Steps 1–4 have no migration, so a saga's route is not yet persisted. Step 3 runs one worker and treats
  every unfinished saga's route as the implied `binance:usdm` on this process's credentials, with `FundingRouteMode`:
  `Selected` (as before), `Recovery` (the selected venue has no funding; finishes started sagas on the implied route,
  expires unstarted quotes, never touches the selected venue's capital) and `Blocked` (the route's credentials are
  unusable; writes nothing, liabilities retained). Evidence for `Blocked` is `blocked_reason` on the funding views and the
  gauge `robsond_funding_sagas_blocked`. **Owner of durable per-saga routing and per-route workers: step 8** (moved from step 7 on 2026-11-08), whose legacy
  mapping must also account for sagas a simulator created earlier on the same database.
- **Simulated venues.** A factory-built stub has no funding surface, so it can neither create nor execute a persisted saga.
- **Conversion exclusion.** Excluding governed inventory and reserving conversion quantities is not delivered. It is
  unreachable while every real selectable venue has funding, but `Recovery` on a spot venue would act on the execution
  wallet: `current_spot_usdt()` transfers all free spot USDT, which there is quote cash. **Owner: step 8, before
  activation**, covering recovery work too; refusing recovery while spot is selected is the acceptable alternative.
- **`eligible`.** `/readyz.venue` computes only valuation freshness today and reports `rules`, `costs` and
  `account_observation` freshness as untracked reasons, so it reads `false`. Step 6 adds `rules_refresh_due`, which can
  supply the rules input; `/readyz` does not read it yet. **Owner: the steps producing that evidence, and step 8 activation
  requires all of it.**
- **Reconnect resync and feed properties.** Today a reconnect takes no snapshot; REST polling starts only after the
  watchdog's 90 s of silence with a risk-open position, and subscriptions follow the configured symbols rather than the
  active `(venue, symbol)` set. Step 8 inherits this behaviour. **Owner: a separate reviewed change, before a second
  venue**, together with the §10 feed property tests.

### 4. Domain cleanup with stable encodings

- **Credential identity is not the venue.** `robson_domain::Exchange` keeps its serialized values (`"binance"`,
  `"binance_testnet"`): they are the AES-GCM AAD, the store query key and the SQL CHECK. A venue is `(Exchange,
  MarketKind, profile)`; `binance:spot` reuses the `"binance"` credential. `base_url()` leaves the domain for
  `robson_connectors::binance::Endpoints`. A new exchange adds a credential value through an explicit migration with
  old-ciphertext compatibility tests. `Endpoints::base_url` moved unchanged and still has no production caller: live hosts
  come from `BinanceRestClient`, `BinanceWebSocketClient` and the trade feed. Its mapping is asymmetric (`Binance` → spot
  host, `BinanceTestnet` → USDⓈ-M testnet host), so it is not a host table and step 8 must not use it as one.
- `as_binance_str` leaves `ports.rs`; wire encodings belong to adapters (private `robson_connectors::binance::wire`).
- `binance_position_id` → `exchange_position_id` in Rust only, on **both** the event payload and the independently
  serialized `Position`, each with `#[serde(rename = "binance_position_id")]`. Compatibility contract: canonical
  serialization before and after the refactor is equal (fixtures with null and non-null values; a missing optional field
  reserializes as `null` before and after, which is the existing behaviour); historical payloads deserialize; the legacy
  SQL column, the active projector payload (which does not carry the field) and the API/frontend field are unchanged.
  The position-summary API does not serve this field today (`PositionSummary` has none, so the frontend normalizer yields
  null). The event payload reaches the event log and `/events/history`, which returns raw payloads, so the wire name stays
  observable there. The log text naming the field also stays, because log output is observable.
- Sizing vocabulary: the "margin cap" in `size_entry` is named the **capital cap** (`notional ≤ available_quote × (1 −
  headroom)`, leverage constant 1); numbers unchanged. `RiskConfig::leverage()` is validated only by position accounts.
  As implemented, the rename covers `size_entry` and its engine caller. Configured and observable names stay:
  `margin_headroom_bps`, `ROBSON_MARGIN_HEADROOM_BPS`, the rejection text pinned by the fixtures and the log text. So do
  the RiskGate initial-margin check and `calculate_margin_required`, which are not this cap, and prose docs that describe
  USDⓈ-M sizing. `RiskConfig::leverage()` has no production caller, but the executor passes `RiskConfig::LEVERAGE` to
  `validate_futures_settings` at entry and exit unconditionally, and a venue without a position account gets
  `ExecError::Config`. **Owner: step 8 gates that call to position accounts.**

### 5. Quantisation and metadata rules (behaviour-changing step)

`normalize_market_quantity` hard-codes a `0.001` step and checks only `minQty`; `SymbolTradingRules` aligns to the real
step and validates min, max and alignment; they differ. Step 6 replaces the helper with
`robson_exec::quantize::market_quantity(rules, qty)` and fixes **two different rules**:
- **Admission** requires fresh validated metadata (`ExecutableSpan` flows fail closed, as today); the rules snapshot used
  at arm is persisted with the leg.
- **Risk reduction (exit, protection)** never waits for fresh metadata: it uses the leg's persisted rules snapshot (or the
  last validated cache), submits, and on a rules rejection refreshes once and resubmits; protection is retained until the
  successor is accepted (position accounts) or handled per 2.2 (balance accounts); a rounded residual below the lot
  minimum is recorded as residual inventory, never declared closed. Today's fallback quantity path
  (`binance_exchange.rs:391`) is replaced by this rule, not removed silently.
The domain stop resolver's adverse tick rounding is reused; ADR-0052's persisted span is never recomputed.

**What step 6 delivers, and where it reads this section narrowly:**
- **Snapshot.** Taken at admission (the entry signal), not at arm, and under `ExecutableSpan` only: `LegacyUncapped`
  admissions need fresh metadata but persist none. It is an optional `rules_snapshot` on `entry_signal_received`, read back
  from `event_log` at restart as the first non-null one by stream sequence. No migration.
- **Source order for risk reduction.** Last validated cache, then the snapshot. The cache goes first because it is never older
  than the snapshot taken from it. Holding either never waits: a refresh past the TTL goes on the side, one in flight per
  symbol.
- **The one exception to "never waits".** A leg holding neither, which after this step means one admitted before it or one whose
  snapshot could not be read, does the rules lookup on whatever path it is on, including the tick, as before this step. The lookup
  and the rejection refresh are each bounded end to end at 15 s. If the lookup fails, a decided exit (close, panic, recovery, or a
  tick that already had rules) goes out with the quantity as given. A tick cannot decide without rules, because the `ExecutableSpan`
  trigger derives from them: that leg fails closed for that tick, the other legs are processed, and the tick returns the first error.
  After an attempted fetch that did not succeed, a symbol with nothing cached retries after 30 s instead of 5 minutes, counted from
  the attempt's start even if the bound dropped it, so such a leg cannot evaluate its software stop for up to 30 s after the venue
  recovers (§10 states the request cost).
- **Not executable under held rules.** Those rules may be stale, so the order is submitted raw and the venue decides. Only a
  venue refusal confirmed by rules that also say so ends as not executable.
- **Maximum.** Not checked locally on risk reduction, so stale rules cannot stop an order reaching the venue; a venue refusal
  is a rules rejection like any other.
- **Rules rejection on USDⓈ-M.** HTTP 400 with `-1111`, `-4004`, `-4005`, `-4013`, `-4014`, `-4023` or `-4164`. This is a
  reading of Binance's error table, not a guarantee, and only `-1111` has repository evidence (2026-07-04). It is separate
  from the step 2 finality list: a rejected attempt stays unresolved in the ledger. It takes at most one `exchangeInfo`,
  skipped within 30 s of the last refresh, and one resubmit under the same client id; the reduce-only flag bounds what a
  misreading can do. A second rejection returns to the caller, and the leg retries after 30 s.
- **A fill closes its leg, and a remainder is unauthored inventory.** An exit fill, or a stop fill the daemon reaches through an
  exit, startup recovery, reverse reconciliation or the tick's overtaken path, closes the leg whatever quantity it executed, and no venue
  read decides it. A stop fill reached by none of these leaves the leg `Active` (see "A stop gone from the venue"). For a market exit this rests on a reading with no venue evidence: a `FILLED` reduce-only
  order short of its request means the venue capped it because the position was smaller, or rounding after a lot rule rose, leaving
  under one lot. `PARTIALLY_FILLED` and `EXPIRED` on a market reduce return the error they returned before this step, and the exit
  retries. A stop is different: one that executes part and then expires or is cancelled counts as terminal, and startup recovery and
  `/reconcile-close` (on operator evidence) can close a leg on it while the venue still holds most of the leg; that remainder is not
  dust and goes to the sweep at market. The worker, the startup gate and the tick's overtaken path do not close on it while the venue
  shows the position.
- **How long a remainder sits.** The worker closes it under ADR-0022 with no grace period, normally on its next scan
  (`ROBSON_RECONCILIATION_INTERVAL_SECS`, default 60 s), and it has no stop until then. **There is no upper bound**: a failed positions
  read skips the pass for as long as it fails, passes are serial and a slow request delays them, a venue refusal or a remainder below
  the lot retries every scan until the operator clears it, and downtime extends it. Nothing blocks a new entry meanwhile: a new
  `Entering`, `Active` or `Exiting` leg on the same symbol and side makes the remainder count as tracked (§2.3's symbol/side gap), so it
  is not swept until that leg closes. A failed UNTRACKED close no longer ends the rest of the scan.
- **Evidence of a short exit.** An exit fill short of the leg always writes `residual_inventory_recorded` first, on the tick, close
  and panic paths, because `ExitOrderPlaced` carries the requested quantity and this is the only durable record of what filled.
- **A triggered stop counts as filled only with terminal, complete evidence.** `NEW`, `PARTIALLY_FILLED` and unknown statuses are not
  fills, and a terminal order's summed trades must equal its cumulative executed quantity; otherwise the caller retries on a later
  pass. This also tightens startup recovery, the tick's overtaken path and the worker's missing-position path. There is no bound if
  they never match: the lookup reads one page of 1000 trades, so an order beyond it, or past the venue's trade history, stays unresolved
  on a flat venue where the previous code accepted the partial evidence, until user-trade evidence or `/reconcile-close` resolves it.
  **Remaining gap:** an order not found (`-2013`) still falls through to its trades, which cannot prove it terminal or complete.
- **What a close books.** Every close books the leg's recorded quantity at the last order's fill, and that order's fee. This is an
  estimate when a leg closes through more than one order: the earlier fill is priced at the later fill's price and its fee is not
  counted. It is kept over pricing only what the last order executed, which would drop the earlier quantity from the month's net and
  understate a loss. If the recorded quantity really was larger than the venue held, the difference is booked at the last fill too,
  as the exit and panic paths did before this step. Only reverse reconciliation prices a fill above the leg at what filled, and its
  evidence keeps the real filled quantity. Per-fill accounting is a step 8 obligation.
- **Residual on an open leg.** `residual_inventory_recorded` is audit-only: the projector ignores it, and its `fill_price` and `fee`
  are evidence nobody reads yet. On a leg that stays open (a rounded stop, a stop below the lot, an exit that is not executable) the
  latest residual stands and only closing the leg resolves it: an accepted stop does not clear it, even one covering the whole leg. An
  unchanged value writes nothing. A stop below the lot minimum retries every 5 minutes and writes durable events once.
- **Admission block.** A residual on any open leg refuses new admissions through its own check and reason (409). It never fails
  the budget snapshot and is no input to the budget, latent risk or MonthlyHalt. It does not cover a remainder after a close.
- **MonthlyHalt.** With latent risk unresolved, the halt fires only on a valid capital base with realized losses alone exhausting the
  budget (`min(consumed_active, consumed_nfs)`). Below that it defers as before this step. An invalid base never fires it, with latent
  risk resolved or not (ADR-0051 §1, amended 2026-11-07, after this step).
- **Stop healing.** Any `Active` leg with no stop on record is re-placed through the existing heal path, whichever caller or error
  left it so. The first attempt waits the 30 s window and needs a tick, so the exposure after a failed tick exit is at least 30 s and
  has no upper bound without ticks on the symbol. A leg with nothing held and a failed lookup waits for metadata; a leg below the lot
  retries every 5 minutes; a leg suspected closed on the venue is not healed, rechecked when a heal comes due. After placement, and again after a replacement cancels its
  predecessor, a leg no longer `Active` gets nothing recorded and the new stop is withdrawn, best-effort. A window remains between the
  last check and applying the event; a stop that survives it is an orphan for the next sweep and, until then, could reduce a new entry
  on that symbol and side if it triggers. The sweep also visits the symbols of legs closed in the last 24 h by `closed_at`, so such a stop,
  or the one a successful `DELETE` or `/panic` leaves, normally goes within a scan. It does not on a flat symbol with no open leg when
  `get_open_orders` keeps failing past those 24 h, when the close is backdated past them (`closed_at` is the fill's time, so a restart
  can reconcile an old fill), or for a leg in `Error`. Untyped exit failures retry at tick cadence as before this step; only `RulesRejected`
  and `ReduceOnlyRejected` have a 30 s window. **Known corner:** a persistent untyped refusal while price stays past the stop submits
  at tick cadence and alternates heal and cancel, about 2 stop events per 30 s, until the operator acts.
- **A stop gone from the venue.** A stop can stop being live on a leg the venue still holds: a rounded stop fills and leaves a remainder
  with no crossing tick, a stop executes part and expires, the operator cancels it, or the sweep cancels a successor not yet recorded.
  The leg stays `Active`, so nothing closes it, and a recorded id would keep the heal from noticing. The orphan sweep already reads the
  open protective orders of every symbol with an `Active` leg, so no request is added: on a successful read, a leg the venue holds whose
  recorded stop is not among them, and whose id is unchanged on a re-read, is scheduled for the heal, with a warning and the stale gauge.
  Exposure lasts up to one scan, plus the 30 s window, plus the next tick, and with no feed nothing is re-placed. Not verified on a venue:
  that a reduce-only stop larger than what is held is accepted (if refused, `InsuranceStopFailed` repeats every 30 s and only the software
  stop protects the leg), and that the read returns every live conditional stop (if it hid one, the cost is one replacement per leg per
  scan, which stacks only if a predecessor's cancel also fails). The scan's two reads are not atomic, so a stop filling between them
  schedules a heal on a leg about to be flat; the venue may refuse the new stop, and then it retries every 30 s until the leg closes. A
  worker lookup that closed the leg on the dead stop was built and withdrawn (Review trail, v11).
- **A superseded stop stays evidence of a whole fill.** A leg remembers its last 3 displaced stop ids, restored at restart from the stream
  it already reads, because a stop can fill after being displaced, by this heal, by startup recovery or by a trailing advance. The
  missing-position worker and the startup gate try the recorded id, then those; a displaced stop counts only if it filled the leg's whole
  quantity, and it keeps its `ins-` authorship, so the close is `InsuranceStop` and counts in the budget. An older partial is ignored, so it
  cannot finalize a later close at its own price and month, and the path falls through to user trades. That costs up to 3 ids × 3 requests,
  only when the recorded id has no fill. The tick's overtaken path looks up the recorded stop only, so it costs per mark what it did before
  this step. Three further displacements before the fill push an id off the list and the close falls to user trades, outside the budget; a
  failed append of the event loses the id at restart (step 7). A fill that left the leg open stays out of the month's net until the leg
  closes and the whole leg is booked at the later fill.
- **Restart.** An event-log query error aborts startup with a retryable exit; exchange stops stay live meanwhile. A residual row that
  cannot be read, or is not the variant its type says for this leg, sets `residual_unknown`, which blocks admissions the same way until
  the leg closes; such a rules row counts as no snapshot. A residual going A→B→A without an order id hits event-log idempotency, so a
  restart shows B instead of A; both block.
- **The old fallback.** The path cited above now lives in `usdm/exchange.rs` and is gone, together with its 2-decimal
  trigger rounding. With no cache, no snapshot and `exchangeInfo` down, a legacy trigger whose 2-decimal result happened to
  be tick-aligned is no longer placed and retries at 30 s. New `ExecutableSpan` legs cannot reach this.
- **Accepted limit: a real partial is not in the month's net while the leg stays open**, and the close only approximates it, as above.
  The income ledger is a conditional safety net: a `REALIZED_PNL` or `COMMISSION` row matches when exactly one position on the symbol
  has `entry_filled_at` or `closed_at` within 120 s of it, so a partial near the entry or the close raises nothing. Otherwise it stays
  unmatched, alarms after the grace period and makes `transfer_explains_delta` return `None`, which stops transfer-confirmed
  `capital_base` recalibration for that window; acknowledging the row does not lift that (ADR-0045 §2). Trading is unaffected.
- **Crash between a fill and its events, or a failed durable append** (reasoned, not tested; **owner: step 7**). A residual's admission
  block can be lost at restart. The leg keeps its recorded quantity and stays managed.

### 6. Long-only readiness

Arming `Side::Short` on a venue with `supports_short = false` is rejected at admission with a typed error
(`VenueUnsupported { side }`) before any order. No other core change.

### 7. Multi-venue execution model (constraints fixed here; implemented by a later ADR)

The operator arms **one logical operation** (`symbol`, `side`, selected venues, risk weights). The entry pipeline fans out
into one **leg per venue**; a leg is a full position with its own fills, protection, reconciliation and close.
Prerequisites, all durable: `LogicalOperationId`, `LegId`, `VenueId` and account scope on every position; venue-scoped
exchange order ids, settlement ids and checkpoints; mapping of every pre-existing position to `binance:usdm`; feed
isolation (§3); durable intent journal (§8 step 7).

- **Weights allocate planned risk, not quantity.** With `R` the requested risk before quantization and normalized
  nonnegative weights, leg *i* is derived from `R × wᵢ`: its own quantity from its own venue price, executable stop
  (levels from its own feed), cost envelope, cash cap and rules. Its final cost-priced charge is `aᵢ`; the operation
  reserves `A = Σ aᵢ`. Unused slack is not redistributed. Quantities are never transferred between venues.
- **One governed admission reserves the aggregate atomically** before anything is externally visible (ADR-0051 §1).
  With `aᵢ` the final cost-priced charge of leg *i* after quantization and `A = Σ aᵢ` as above:

  ```text
  A = Σ aᵢ > 0
  A ≤ 0.01 × capital_base
  consumed + latent_before + A ≤ monthly_budget                    (global, ADR-0051 net-from-start)
  aᵢ ≤ 0.01 × Bᵢ                                                  (per-leg cap on the venue's own basis)
  local_consumedᵢ = max(0, −settlement_complete_local_netᵢ)       (signed net, ADR-0051 semantics)
  local_consumedᵢ + local_latent_beforeᵢ + aᵢ ≤ λ × Bᵢ            (per-venue monthly cap, N2)
  ```

  Global and local reservations are committed together or not at all. Reservations are held through unknown
  acknowledgements, partial fills and unsettled closes; a failed leg's reservation is released only after
  `OrderLookup::Absent` (under the venue's finality rule) or `Terminal` evidence that no liability remains (`Unknown`
  never releases). Failed weights are not redistributed; a recovered venue is not entered later without the operator's
  retry policy (N3).
- **Budget: global 4 % plus a per-venue monthly cap λ (N2).** These are enforced **admission-budget** constraints;
  their realized-loss implications depend on the priced execution envelope, and N1-a exposure can exceed every stated
  cap. Within the envelope the global −4 % on the consolidated basis is the outer bound. The per-venue cap `λ × Bᵢ` uses the same signed-net definition as ADR-0051, so at `λ = 4 %` it is exactly a
  per-venue ADR-0051 budget (no fungibility); larger λ keeps fungibility. Proposed default `λ = 8 %`. Without λ no local
  survival statement exists (one venue could absorb the whole global allowance); the floor below is a guarantee only
  while λ is enforced, within the execution envelope (N1-a exposure excluded), without external cashflows, and under the
  prices, rules and costs assumed at evaluation.
- **Viability floor per venue** (operator requirement). Let `Cₛ = Bᵢ × (1 − λ)^K` be the stressed bankroll after `K`
  worst months at the local cap. Let `q*` be the **single minimum feasible quantity** on the venue for the symbol: the
  smallest lot-aligned quantity such that (i) the entry market order passes the lot and notional minimums (Binance spot
  evaluates the notional on a 5-minute average price, so the reference is an envelope, not the entry price), (ii) the
  **net protectable quantity** after the commission policy (base-asset commission reduces the bought quantity;
  quantization of the protection rounds down) still passes the lot minimum, and (iii) the protection/exit order passes
  the notional minimum **at the executable stop region**, not at entry. Any remainder that cannot be protected is a
  reserved liability priced at the conservative mark. A venue is viable for the symbol only if both hold:

  ```text
  0.01 × Cₛ              ≥  q* × worst_loss_per_unit(d_max, costs)     (risk capacity)
  Cₛ × (1 − headroom)     ≥  q* × entry_price_envelope                   (stressed cash)
  ```

  `d_max` is the maximum admissible executable distance (ADR-0050 §5 bound; conservative input), `worst_loss_per_unit`
  the ADR-0050 §3 costing (adverse fill at `(1 − d_max) × (1 − gap)`, taker per fill, long-side loss ratio 0.10184955
  with repository defaults). Thresholds are computed at full precision and rounded **up** (ceiling) to the venue's cash precision; the table
  shows two-decimal ceilings. **Illustrations** (conditional
  arithmetic with repository fee defaults, entry 77,500 USDT, `d_max` 10 %, loss per BTC 7,893.34 USDT; not accepted
  floors; metadata in `docs/evidence/2026-11-04-binance-exchangeinfo-btcusdt.md`):

  | Venue | `q*` and why | Planned loss | Floor, λ = 4 %, K = 3 (×0.884736) | Floor, λ = 8 %, K = 3 (×0.778688) |
  |---|---|---:|---:|---:|
  | Binance USDⓈ-M BTCUSDT | 0.001 BTC (lot min; 77.5 ≥ 50 USDT) | 7.893 USDT | ≥ 892.17 USDT | ≥ 1,013.68 USDT |
  | Binance spot BTCUSDT, quote-fee policy | 0.00008 BTC (7 lots = 5.425 USDT at entry but 4.88 USDT at the stop region, failing the 5 USDT exit check; 8 lots = 5.58 USDT there, conditional on that reference envelope) | 0.631 USDT | ≥ 71.38 USDT | ≥ 81.10 USDT |
  | Binance spot BTCUSDT, base-fee policy | not shown: 8 lots net of a 5 bps base commission round down to 7 protectable lots; `q*` must be solved with the commission precision and the chosen policy (N5) | — | — | — |

  `K` is `ROBSON_VENUE_VIABILITY_MONTHS` (proposed 3). The floor is checked when an allocation is saved, at every month
  boundary and at admission. It states bankroll sufficiency, not uninterrupted trading: re-applying a rolling K-month
  floor can suspend a venue before K months elapse; a venue below it is flagged in `/status` and excluded from new legs,
  never from exits.
- **Costs are visible where the weights are edited**: the API returns each venue's envelope next to its weight.
- **Out of scope here**: cross-venue routing by liquidity, synthetic hedges, transfer automation.

### 8. Sequencing and rollback boundaries

Eight PRs, each Codex-reviewed before merge, during the empty-book window. Mechanical steps (1, 3) change no timing,
threshold or budget; any such change is its own step.

**Priority order decided 2026-11-05 ("Fase S", Binance spot first).** The operator's priority is to execute a long on
Binance spot as soon as it is safe, keeping the USDⓈ-M adapter intact as `binance:usdm`. The steps are therefore merged
in the order **1 → 2 → 3 → 4 → 6 → 7 → 8**; step 5 (cost-priced boundary downside, carried-basis migration) is deferred
until after step 8 and before any second venue, with today's basis derivation kept meanwhile (§2.4 already allows
this). The rollback boundaries of the table are unchanged by the reordering: step 6 and step 7 still require the stated
empty-book conditions, and step 8 depends on steps 2, 3, 6 and 7.

**Interim accounting contract while step 5 is deferred.** Step 8 must establish and persist the interim capital basis
from the operated spot wallet (quote cash plus governed inventory at the venue mark, §2.4), preserving the active
monthly model and the consumed budget without any intra-month budget increase. Until step 5 ships, **carrying
risk-bearing or unsettled legs across a UTC month boundary is unsupported**: admissions quiesce before the boundary and
the book must drain and settle beforehand. If that cannot be verified at the boundary, accounting is invalid and new
admissions stay blocked while protection and recovery continue. Supporting carried legs requires step 5 or a separately
reviewed interim accounting contract. (Reason: today's boundary helper clamps entry-to-stop at zero; with a trailing
stop above entry it would reserve no downside for a spot leg marked at the boundary, and a normal stop fill could exceed
the monthly allowance.) The deferred step 5 still requires its own verified empty-book window at deployment time; the
original pre-spot window does not carry over.

**Execution account during Fase S.** Binance does not offer sub-accounts to the operator's personal account, so the
execution account is the **spot wallet** of that account, which the spot API observes in isolation from the Funding and
Earn wallets. The operator moves any non-Robson holdings out of the spot wallet before activation; ADR-0022 applies
unchanged to whatever remains there. N6 (dedicated sub-account on the corporate account) applies as soon as the PJ
account exists.

| Step | Content | Behaviour | Rollback boundary |
|------|---------|-----------|-------------------|
| 1 | Mechanical relocation of the two adapters into `robson-connectors` (the futures WS wiring moved in step 3); `ExchangePort` unchanged | preserving | revert alone while nothing else imports the new paths; afterwards revert together with its dependants |
| 2 | New port contracts (§2) + USDⓈ-M adapter implementing them; legacy `ExchangePort` kept as a forwarding compatibility layer; separate `unrealized_pnl` field added, `wallet_balance` untouched | preserving | revert 2 (and 3 if merged) |
| 3 | `VenueFactory`, `with_venue`, `TradeFeedPort` and the WS wiring relocation, feed injection with `(VenueId, Symbol)` keys, additive `/readyz.venue`, funding route modes (§3 lists what is staged) | preserving for execution with `ROBSON_VENUE` unset and usable credentials; additive for readiness consumers; stub paths lose their funding surface | revert alone |
| 4 | Rust naming and endpoint cleanup with stable event, `Position` and credential encodings; fixtures | preserving | revert alone |
| 5 | Accounting: governed equity derived once; month-boundary basis uses ADR-0051 §3 cost-priced downside; **migration** for persisted carried bases with compatible settlement attribution and replay handling; atomic reset | **changing** (basis, persisted state version) | once new accounting state is written, rollback needs demonstrated reader/writer compatibility of the older binary with that state, or a verified book free of risk-bearing and unsettled liabilities; keeping `capital_base` alone is insufficient |
| 6 | Quantisation and metadata rules (§5) with before/after fixtures per order kind | **changing** | revert alone only with a verified empty book (the old helper is incompatible with spot lots) **and no `residual_inventory_recorded` row in `event_log`**, since an older binary's projector returns `MissingHandler` on it and its event enum cannot read it; the additive `rules_snapshot` needs no such condition |
| 7 | Durable intent journal in the store (migration): intents and results persisted in order relative to reservations, stable venue-scoped client ids, durable attempt evidence (§2), explicit unresolved states, transition intents (§2.2); **durable venue/account routing on every risk-bearing and unsettled leg**, with the legacy mapping of every existing position to `binance:usdm`; process-kill tests at every external-call/persistence boundary | preserving (adds evidence and identity) | revert alone with an empty book; after step 8 only together with step 8 |
| 8 | Spot venue acceptance behind `ROBSON_VENUE=binance_spot`: `InventoryPort`, protection protocol per N1, settlement from fills, conformance suite green; the long-only entry gate is a separate ADR | new venue | disabling a venue stops new admissions on it but **keeps managing its open and unsettled legs** through the step-7 routing; a binary rollback requires a verified empty book on that venue |

**Step 7 is delivered in parts, and this is what is implemented so far (operational rollout pending).** Step 7's scope is
the venue-independent core; routing, booking and everything else that needs the spot design moved to step 8 by operator
decision on 2026-11-08 (`Fence and manual recovery`, `Core now`). The parts land in the order harness, journal, cancels,
stop, persistence, fence, ownership races. The row above is the original statement.

*Harness (PR H).* Named crash points, a fake venue, a database proxy, CI step. No production behaviour.

*Order journal, part 1 (store and rules).* Migration 31 adds `order_intents` (one row per logical order, unique per venue,
account scope and client id), `submit_attempts` and `journal_decisions`, and the view `order_intents_blocking_rollback`.
`robson_exec::order_journal` holds the rules and `robsond::order_journal_store` the PostgreSQL side.

- **Attempt rules.** An attempt goes `started → rejected` and nothing else, and an upsert lets `rejected` absorb `started`, so
  replays and lost acknowledgements converge. A further attempt on a client id commits together with every earlier attempt on
  it, including attempts known only in memory; a failed or uncertain commit refuses that send, and the id is closed for the
  rest of the process. A later process cannot use a client id that landed. An id whose row never landed and whose process
  died cannot be detected as used: that case rests on ids never being minted twice.
- **Write classes.** Strict: the intent and first attempt commit before the request, waiting at most 500 ms. A known failure is
  a proven no-send; a timeout or a commit with no answer is uncertain and no request goes out. Bounded: the commit is tried for
  at most 500 ms on the journal's own pool of 2 connections and the request goes out anyway, setting the latch if the row did
  not land. A client id already spent is refused on either class. **"Known failure" means the `COMMIT` was never sent**, since a
  transaction that never sent it cannot be durable: a statement error before it is known, which is broader than "before the
  statement was sent"; every timeout stays uncertain.
- **Results and retry.** A result write never fails its caller. It is registered in the retry set before the wait, so a future
  dropped mid-commit stays there, counted on the gauge and flushed; the set holds at most 1,024 intents, and one retry pass stops
  starting commits after 2 s, so it holds its loop for at most 2.5 s. Overflow discards that intent's history and sets the latch.
- **Discarded ids stay refused while they can be kept.** An id that leaves memory either has a durable row, which the unique index
  holds for good, or was never sent, except one sent with no row and no room in the retry set. Those go to a set of at most 4,096 ids
  (about 400 KB) kept for the life of the process, and are refused through any handle. **A full journal never stops a bounded first
  send.** With both full the send goes out and its id is not kept, so its non-reuse rests on the id being fresh, the same reliance as
  a process that died with no row. Every bounded path mints a fresh id per intent. The loss is logged at ERROR and counted, and the
  count only grows until restart.
- **Cancellation of a read or a write.** A connection whose read or write did not finish is closed, not returned to the pool, so a
  dropped future cannot leave the two connections waiting on replies that will never come. An interrupted cancel intent is uncertain, never
  `not_sent`: `not_sent` needs a ticket as proof, and a cancel has none.
- **Query bounds (guardrails §1.1).** Every write is one explicit transaction ending in a simple-query `COMMIT`; nothing commits
  implicitly. Reviewed bounds per handled operation, `BEGIN` and `COMMIT` included: an order 9 (13 with one quantisation retry, 22 with a
  refused retry replaced by a fresh intent), a cancel 9, a stop placement with its predecessor's cancel 31, a cancel-first exit 31, a
  placement that withdraws both stops 40.
  They are above the guardrails' default review threshold of 15 on purpose: the writes are small and each pair is one order's
  durability. Each operation's count is kept and the bound (40) is checked when it ends. These are charged counts: a commit is charged
  what it plans to run, so one that fails before sending anything is charged in full. They are not a measure of executed SQL.
  From PR S an operation is a permit's whole span: an entry and its initial stop are one operation, 18. An operation over several legs
  (one main-loop event on a symbol with several legs, a reconciliation scan, `/panic`, the startup phase) is charged the sum of its orders,
  so it can pass 40; the over-bound counter then counts it too, which marks a loop of venue work for review rather than an error.

**Rollback across this schema is boundary B:** with the daemon stopped, `SELECT * FROM order_intents_blocking_rollback` must be
empty. It lists every intent with no definite outcome that is not proven unsent and has no disposition. Read on a running process
it is a snapshot, not an authorization. An older binary ignores the new tables, which is why ignoring them needs this check first.

**A rollback image must keep migration 31.** SQLx's migrator rejects an applied migration missing from its embedded set
(`VersionMissing`), and `robsond db migrate` runs in the deployment's Sync hook. So once migration 31 is applied, pinning straight back to
`sha-179898cf` is rejected by that image's migration hook. Rollback means an image that keeps migration 31, or a compatible
migration-job image.

Part 1 changes nothing at runtime: nothing calls the journal yet.

*Order journal, part 2 (disposition and metrics).* **`POST /recovery/disposition`** (authenticated; a verified Google identity is
recorded as `verified`, the legacy token as `asserted`) records, per outstanding intent, `terminal_evidence` or `accepted_risk`,
plus the accounting treatment of what remains (`unbooked_accepted` in the core). It writes no venue outcome and no event, books
nothing and affects no admission. Terminal evidence names the order (for a cancellation, the venue reference of the order it cancelled, never its local `cxl-` id, which
never reached the venue), states a terminal status consistent with its filled quantity,
which cannot be negative or more than the order asked for, gives its source and a past observation time, and **attaches the venue's
own record of the order**. Without a record, what the operator has is an assertion, and the disposition to make is `accepted_risk`.
The endpoint does not check the record against the venue: it is what the named operator attached. So an observation that the account
is empty never satisfies B. A disposition is not a venue outcome and does not discharge liabilities; the recorded treatment does.
Each request appends to `journal_decisions` and the latest stands, so a retry after a lost acknowledgement adds a second decision
and changes nothing else.

Metrics, defined here and published from part 3 on: `robsond_journal_latch`, `robsond_journal_retry_set_depth`,
`robsond_journal_non_reuse_ids_lost`, `robsond_journal_statements` (charged, not executed),
`robsond_journal_operation_statements_max`, `robsond_journal_operations_over_statement_bound`.
`robsond_journal_non_reuse_ids_lost` is process-local, resets on restart, and counts sends authorized without their id being kept, not
a verified inventory of orders with no row.

*Order journal, part 3 (wiring).* Every order path writes through the journal: entry (strict); exit, protective stop, replacement,
withdrawal, software-exit and orphan cancels, and the UNTRACKED sweep close (bounded). Funding conversions are not journaled and are
never evidence for `Absent`. The daemon checks the journal at startup, which also warms its pool: it refuses to start when the journal's
tables are missing or when the whole check, probes and retries included, has not finished within 10 s, and a stalled probe's connection is
closed. On a slow database this is new: a boot that used to carry on now exits with an error if the journal cannot answer in that time, and
the pod restarts.

- **Outcome classes are recorded only.** An intent ends as a success, a rejection (every attempt authoritatively refused), not sent,
  or uncertain. **Between this PR and the fence PR, uncertainty is journaled but does not fence, exactly as today:** no incident
  opens, no admission closes, and the latch shows only on its metric. A failed result write never discards a venue result and never
  prevents protection. Event persistence after a venue result can still abort the flow as before; that is a later PR.
- **Tickets and the re-arm backoff.** The attempt ledger reports a send ticket just before a request leaves. The backoff reset moved
  from before the executor to after it, on `Ok` and `Err` alike, and only when the entry holds a ticket. An adapter refusing locally
  before the send is therefore a proven no-send too.
- **A proven no-send (design D15, amended).** ADR-0052 makes admission evidence on an `Armed` leg something to recover explicitly, and
  the engine refuses a second admission against it, so a re-arm could never admit. On a leg with evidence (`ExecutableSpan`, the
  production default) the attempt ends at once through `mark_entry_exhausted`, reason `entry_not_sent`, audited and with no re-arm;
  the evidence is neither cleared nor re-admitted against, and the operator re-arms. On a leg with none (legacy policy) the detector
  goes back under the governed backoff. A refusal by the account's futures settings is the same case: its safety event stands and a leg with
  evidence ends the same way, while a legacy leg keeps today's behaviour there, with no exhaustion and no re-arm. If the exhaustion cannot be
  appended, this process still holds the record and re-arms nothing, but that is a same-process statement: a restart can restore the detector,
  and the engine's evidence check then rejects the next signal and makes it terminal. Across a restart what holds is that no second admission
  happens. An uncertain commit is not a no-send: no re-arm and no reset.
- **A refused resubmission does not wait.** A rules rejection earns one resubmission under the same id, and the completeness rule can
  refuse it. The refused id stays closed, and when the first submission ended in that rules rejection and only that first request left, the
  refreshed order goes out once under a fresh id in the same call, as a bounded first attempt, so a cancel-first exit is not left with no stop
  and no exit until a later tick. One fallback, no loop: if it fails or is refused, the ordinary error path applies. Exit, initial stop,
  replacement and sweep close share this through `submit_reduction`; a cancel has no resubmission, and an entry stays strict and has no
  fallback. The refused intent ends uncertain and the fresh one carries the venue's answer.
- **Cancels.** Each cancel is its own bounded intent. Their classification is the next part.

Trade-off: an entry is refused rather than sent when the journal cannot record it, and a slow database adds up to 500 ms per write to
an order's path. After an entry fill, the result write plus the stop's pre-send write can add 1 s before the stop's request; a cancel-first
exit likewise. "Does not wait" above means no later tick or backoff, not zero journal waiting: the fallback adds another bounded pre-send write and
another outcome write, so up to 2 s of journal waits can pass after the cancel acknowledgement before the fresh request, and 2.5 s in total for a
replaced retry. These waits sit under existing locks: detector handling and the API's close and panic hold the manager write lock across
them, so they can delay unrelated protective work. Nothing acts on uncertainty or the latch yet. `applied_at` and `uncertain_generation`
exist but are not written. In J an entry's operation ended before its stop was placed; PR S makes them one operation. Not verified: behaviour on a
real venue or in the cluster.

*Cancels (PR C).* Every production stop cancel (replacement, withdrawal, software-exit, orphan) goes through one journaled entry that
classifies the answer with one venue-neutral, exhaustive `robson_exec::cancel::classify_cancel`. `ports::StopCancelOutcome` now mirrors
the venue enum variant for variant, payload included (`Unknown` is `Indeterminate` there), `legacy_cancel_outcome` maps one to one and
folds nothing into an error, and `StubExchange` can answer every variant.

| Result | Class | `venue_outcome` | `cancel_class` |
|---|---|---|---|
| `Cancelled { .. }` | definite | `success` | `cancelled` |
| `AlreadyGone { Terminal { .. } }` | definite | `gone_terminal` | `gone_terminal` |
| `AlreadyGone` with `Absent`, `Open`, `PartiallyFilled` or `Unknown` | uncertain | NULL, with `uncertain_at` | `gone_unresolved` |
| `Indeterminate` | uncertain | NULL, with `uncertain_at` | `indeterminate` |
| any `Err` | uncertain | NULL, with `uncertain_at` | `error` |

Both definite classes write the venue's fills to the cancel intent's `venue_evidence` (`filled_before_cancel` or `filled_qty`, and
`fills`). No migration: migration 31 already created `cancel_class` and the `gone_terminal` value, so the design's 5 SQL lines were spent in J.

- **No error string or broad variant can produce a definite class.** `is_unknown_order_error` is deleted. The adapter's `-2011`/`-2013` and
  text detection only chooses between two uncertain classes. USD-M does no lookup on a cancel, so **every USD-M "already gone" is uncertain.**
- **A cancel is its own intent.** A replacement's placement is recorded as a success before the cancel runs, and an uncertain cancel leaves it so.
- **No in-operation resolution.** A later close of the leg, trades-only evidence included, never settles a cancel. Step 8 must supply typed terminal
  evidence before spot activation.

| Path | Definite | Uncertain |
|---|---|---|
| Replacement | success | placement stays a success, the double stop is tolerated and logged, as before |
| Withdrawal | nothing logged | logged, the next stop is still withdrawn, as before |
| Software-exit | `Cancelled`: `InsuranceStopCancelled`. `GoneTerminal`: keeps the stop id, `Skipped`, no event | `GoneUnresolved`: keeps the stop id, `Skipped`. `Indeterminate` or `Err`: `InsuranceStopFailed`, keeps the stop id. No class stops the exit behind it; a failed append or application of the `Cancelled` event does, as it did before this PR, and the failure event's append is awaited |
| Orphan | `InsuranceStopOrphanCancelled`, as before | `GoneUnresolved`: the same report, retained from before. It does not establish that the order is gone, since `Open` and `PartiallyFilled` allow a live one, and its outcome stays uncertain in the journal. `Indeterminate` or `Err`: logged, no event, as before |

**One change in emitted events.** A software-exit cancel answered by `ExecError::OrderRejected`, or by `ExecError::Exchange` with "order" in its
text, used to read as cancelled: it emitted `InsuranceStopCancelled` and cleared the stop id. It now emits `InsuranceStopFailed` and keeps the id, as
every other cancel error already did. **Retaining the id changes what follows it:** reverse reconciliation reads the kept stop's fill before broader trade evidence;
restart recovery takes the branch that checks that order's fill and open state, and may keep it, where an absent id takes the placement branch; and the stale-stop retry,
which runs if the leg is still `Active` when it comes due, because the exit behind the cancel failed, replaces the stop against the kept id, one more cancel than the plain
re-placement of a cleared id.

**A cancel that keeps failing repeats, and that is the accepted cost.** While the id stays on the leg, the software-exit cancel goes out once before each exit attempt, with its
own journal intent and, for `Indeterminate` or `Err`, its own `InsuranceStopFailed` each time. Errors the old `is_unknown_order_error` did not match already did this, and the
broad errors now behave the same way. Generic exit failures have no backoff, so that can be tick cadence; the exit backoffs bound it where they apply. Cancels are not rate-limited,
because a retried cancel can be what lets the next exit through: a stop left live by a timed-out cancel can be removed on the next tick, and a reduction outranks saving a request.

**A pending stop heal keeps its deadline.** An `InsuranceStopFailed` no longer changes a deadline on record, overdue included; only a leg with none gets a new 30 s. So repeated
failures, and a batch whose bookkeeping runs after its deadline has passed, cannot keep a heal from coming due. `ProtectionNotExecutable` still overrides with its long window, and
the heal pushes its own window before acting, so a failed heal still backs off and this does not become a heal on every tick. As a result, a failure during the long window no
longer shortens it to 30 s.

**An outstanding heal is rebuilt at a restart.** The retry window lives in memory, `InsuranceStopFailed` is audit-only, and a kept id hides the leg from missing-stop scheduling. So the
projection-recovery path, in its existing stream query per recovered leg, schedules the heal for an `Active` leg whose latest event among `insurance_stop_placed`, `insurance_stop_replaced`,
`insurance_stop_cancelled` and `insurance_stop_failed` is the failure, at now + 30 s, keeping any entry already there. It runs as any other heal, a `ReplaceInsuranceStop` against the kept id,
and a successful startup-recovery placement clears it. A failed open-orders read schedules nothing, so one read failure cannot start replacing stops on healthy legs. The rebuild is conservative: a failure that is still the latest of those events schedules a replacement even when startup finds the stop healthy (an `already_protected` audit does not count), and it neither keeps elapsed backoff nor makes the replacement exactly-once, so a crash before its event is durable can repeat the replacement on the next boot, and a crash loop inside the 30 s keeps postponing it. Limits: `InsuranceStopFailed`
is a best-effort append, so a failure event that was never written cannot be rebuilt; and the branch of `restore_positions` that finds legs already in the store does not rebuild it, which production
cannot reach, since every daemon starts on a fresh in-memory store whose replay is empty.

**Between this PR and the fence PR, uncertainty is journaled but does not fence, exactly as today:** an uncertain cancel opens no incident
and closes no admission, and the orphan sweep has no veto. The cost the design accepts arrives with the fence, not here: on USD-M, a software-exit cancel attempted after the
exchange stop has filled is uncertain and will fence until an operator release even if the close then succeeds. How often that happens is
unmeasured, and nothing trades on USD-M today. Uncertain cancels block boundary B until they carry a disposition, as they did under J; a
`gone_terminal` one no longer does. Not verified: behaviour on a real venue or in the cluster.

*Stop (PR S).* Permits, process epochs and the two-phase stop, in three parts, each deployable alone in this order: the gate, the
epochs, the stop. Parts 1 and 2 are below. What the design leaves to the fence PR stays there: nothing sets `fenced`, no generation, no
incident, `unclean_start` is recorded and not acted on, old intents are not stamped, and the orphan sweep has no veto.

*Part 1, the gate.* `robson_exec::gate`.

- **Operation permits.** Every operation that can mutate the venue runs under a permit taken at its entry, before any venue request and
  before the manager lock, held in a task-local scope for the whole operation, its result handling and protective follow-ups included. The
  permit is the operation's entry in `operations_in_flight`, J's registry, so an entry and its initial stop are now one operation and the J
  boundary above is closed. A nested operation runs under the permit it is reached from and never takes a second slot or another class: an
  `Immediate` arm's entry and stop, `/reconcile-close` into MonthlyHalt into `panic_close_all`, an entry's protection batch, the startup scan
  inside the startup phase, the reconciliation scan's orphan cancels and UNTRACKED closes. A permit must be completed. One dropped before
  its work returned, a cancelled or panicked future, counts in `robsond_aborted_operations_total` and sets the latch, whatever its rows say.
  An operation that returned an error has completed. An API operation runs in its own task, which owns the permit, and the handler only
  awaits its answer: a client that disconnects drops the handler, which hyper does on a closed connection, and the operation runs on to
  completion; one that panics answers 500, aborted.
- **Slots.** One reserved slot each for the main loop (one event), a reconciliation scan, a funding poll and the startup phase; 16 for the
  ordinary API class (arm, injected signal, approval, funding execute, a spot USDT recovery that executes); 8 for the protective class (cancel
  or close, `/panic`, `POST /monthly-halt`, `/reconcile-close`), which ordinary requests never take. That is 28 of the registry's 32 entries;
  the other 4 take operations without a permit, which in production only a path the choke point counts can be. A full class answers 503
  `busy` at once with nothing done, counted in `robsond_api_slot_busy_total{class}`. Slots guarantee allocation, not latency: holders still
  queue on the manager lock. The route table is `OperationKind::class`. Routes with no venue mutation take no slot: `/funding/quote`,
  `/capital/refresh`, `/entry/preview`, income acknowledgement, `/recovery/disposition` and a recovery dry run.
- **Choke point.** `BinanceRestClient::post_signed` and `delete_signed`, and every mutating method of `StubExchange`, call
  `OperationGate::mutation` before anything is built or sent. With a permit of that gate it passes in every phase. Without one it passes only
  while the daemon is open, counted in `robsond_unpermitted_mutations_total` and setting the latch; once the daemon is stopping it is refused
  and nothing is sent. `Venue::attach_gate` has no default, so every venue says how its mutations reach the gate; the daemon attaches it to
  the selected venue and to the funding recovery route.
- **Send start.** `SubmitAttemptLedger::submit`, and the stub, run `begin_send` immediately before the request is first polled, with no
  await between. Under the gate's mutex, with the journal's memory taken inside it (the lock order is the manager lock, a stream's lock, the
  gate, the journal's memory), an entry is refused unless the daemon is open and unfenced: a typed no-send that registers no attempt and
  leaves no ticket. Anything else is counted in `sends_started` and gets its ticket. This is J's ticket, now issued under the gate: one
  ticket, not two.
- **One bound per request (D19).** Each REST request has one 10 s bound for the send and the body together, where it had 10 s for each. A
  timeout is uncertain.

What part 1 changes in production: more than 16 ordinary or 8 protective API requests at once now get 503 `busy`; a client that disconnects
mid-operation no longer cancels it, where today the dropped handler takes the operation with it, possibly between an entry's fill and its
stop or between the legs of a `/panic`; a panicking operation sets the latch; a request whose head and body together take more than 10 s
now times out. The phase never leaves `Open` in this part.

*Part 2, epochs.* Migration 32 adds `process_epochs`, with the design's columns plus the tenant: a predecessor is the latest epoch of the same
tenant. At start, after the journal's readiness check and before any venue request, the capital read included, one transaction reads the
previous epoch and inserts this one. The start is refused when that does not land within 10 s, as with a database down at start today, or
when `epoch_seq` would reach 2^31, the high half of the fence PR's generation; that row is rolled back. A predecessor with no marker, or one
older than version 2, makes the start unclean: logged, and `robsond_unclean_start{reason}` is 1 for `no_predecessor`, `no_marker` or
`marker_version`. Only the fence PR writes version 2, so every start is unclean until then, and that is all this part does with it.
Without a database, or in a build without PostgreSQL, there is no epoch, and only the stub starts: any other venue, `binance_usdm` whether
named in `ROBSON_VENUE` or selected from credentials, is refused there, before any request to it. The simulator mode is
`ROBSON_VENUE=stub`, or an unset `ROBSON_VENUE` that resolves to the stub; no checked-in workflow ran a real venue without a database.

**Rollback across parts 1 and 2 is boundary B,** with the daemon stopped, and an image that keeps migrations 31 and 32: SQLx's migrator
rejects an applied migration missing from its embedded set, so pinning back to `sha-b06aecb1` or `sha-23501535` is refused by that image's
migration hook. `process_epochs` is additive and an older binary ignores it. Not verified: behaviour on a real venue or in the cluster.

Migrations: none in steps 1–4 and 6; step 5 (carried bases) and step 7 (intent journal, leg routing) ship with explicit
migrations and persisted-state versions. Step 6 adds one optional event field and one event type, registered in bronze
contract 1.2.0; `rbx-data` vendors its own registry copy (1.1.3) and must re-vendor before any export runs. Every rollback
names its transitive dependency prefix: a step is reverted only
together with every later merged step that imports it, and only at a persisted-state version its binary can read.

### 9. Not decided here

The long-only bottom-confirmation entry gate (separate ADR); B3 adapter; multi-venue implementation (§7 constraints
only).

### 10. Acceptance criteria (measurable)

- **Conformance suite** in `robson-testkit`, run against `StubExchange`, the USDⓈ-M adapter and (step 8) the spot
  adapter: flags agree with accessors; protection is `MarketOnTrigger`; replacement property per protocol
  (`PlaceThenCancel`: the old stop is never absent while the new one is unaccepted; `CancelThenPlace`: the transition
  intent is persisted before the cancel, the gauge is nonzero for the whole exposure, and the transition ends only in
  confirmed protection or authoritative closure); arbitration property under concurrent fills (**protected residual
  inventory or authoritative closure**, never a sell of a stale quantity), with crossed-trigger cases at zero old
  fills, partial old fills and partial flattening; finality property (`Absent` only under the
  venue's finality rule).
- **Admission property** (money, per proposal). With `R` the requested risk before quantization and normalized
  nonnegative operator weights, each leg is derived from `R × wᵢ`, its final post-quantization charge `aᵢ` is computed,
  and `A = Σ aᵢ` is reserved; unused slack is not redistributed. Asserted on the same charges committed atomically with
  the global reservation: `A ≤ min(0.01 × capital_base, monthly_budget − consumed − latent_before)`,
  `latent_after = latent_before + A`, `aᵢ ≤ 0.01 × Bᵢ`, `local_consumedᵢ + local_latent_beforeᵢ + aᵢ ≤ λ × Bᵢ`;
  property-tested under concurrent proposals and concurrent settlement.
- **Request and query budget**, measured per handled operation (admission, close, protection transition,
  reconciliation pass, recovery of one leg) by instrumentation exported in `/metrics` and asserted in tests: DB queries
  and **weighted** exchange requests per operation, alert above 15 unless a reviewed operation-specific bound is recorded
  (guardrails §3). Steady-state ceilings per venue: rules refresh ≤ 1 per symbol per 5 min **plus** one per rules
  rejection (§5); costs ≤ 1 per symbol per 24 h; valuation from the feed, REST fallback ≤ 12 per symbol per minute;
  account observation ≤ 6 per minute; order lookups ≤ pending intents per minute; no REST call on a market tick. An
  aggregate weighted-request budget across venues and symbols respects each exchange's shared limits; protective
  operations (re-protect, flatten, reconciliation of an `Unknown`) have priority and an explicit emergency budget over
  the steady-state ceilings, recorded per incident. Degraded mode (feed silent) keeps the same ceilings. Step 6 bounds
  its own requests (§5) by construction and tests, but apart from the counter below delivers no `/metrics` instrumentation and no
  aggregate or protective-priority budget, so it does not establish this criterion. During an outage a symbol with nothing cached costs up to
  2 `exchangeInfo` per minute, above the rules ceiling here for those symbols only. The wider sweep (§5) adds 1 open-orders request per
  symbol with a leg closed in the last 24 h and nothing else to visit it, plus one store read (two in the first 24 h of a month), counted
  by `robsond_reconciliation_closed_symbol_visits_total`, a logical total: the guardrail's measure per handled operation and its
  threshold alert are not met. **Owner of both: before step 8 activation.**
- **Concurrency and memory bounds**: financial admission depends **only** on the monetary constraints of §7 (slots
  report guaranteed full-cap opportunities, `policy.rs:253`, and never cap legs or admissions). Execution is bounded by
  worker concurrency (one in-flight transition per leg, `ROBSON_EXEC_WORKERS` proposed 4) and bounded queues; recovery
  work is durable and paginated, scheduled independently of any financial count; event-bus channels keep their bounded
  capacity; subscriptions equal the active `(venue, symbol)` set and are torn down on close; caches, tasks and recovery
  entries are profiled for cleanup at the supported cardinality (legs × venues) and the profile is part of the review.
- **Feed properties**: reconnect snapshot anchoring, fallback-exit hysteresis (existing), and property tests for
  duplicate and reordered delivery per venue.
- **Durable recovery evidence**: every recovery decision emits an event (observed fills, remaining inventory, protection
  evidence, retained liabilities, unresolved failures); `/status` exposes per-venue eligibility, staleness and inventory
  discrepancies (mandatory, not optional).
- **Failure tests**: process kill at each external-call/persistence boundary (step 7), database loss during result
  persistence, concurrent settlement/admission, reordered feed delivery, unresolved order outcome; old-ciphertext
  decryption after step 4; replay fixtures; the DB-gated tests; security gates unchanged (gitleaks, osv-scanner,
  cargo-audit, SHA-pinned actions); allocation and arming mutations frozen during a request (guardrails §4).

---

## Failure modes (external dependencies and this service)

| Dependency / event | What happens | Degraded mode | Visibility |
|---|---|---|---|
| Database dies | no admission, no new legs; already-installed protection remains on the exchange; inventory in an N1-a transition may remain unprotected without a duration or loss bound; no cancellation starts without a durable intent, and unfinished transitions retain liabilities and alerts and resume from that intent on recovery; intent results cannot be persisted → in-flight operations enter `Unknown`; recovery looks them up, a lookup may stay `Unknown`, and then the reservation stays held until step 7's explicit resolution path | read-only `/status` from memory; arms refused | `/readyz.database=false`; `unprotected_inventory` gauge; recovery events |
| Journal database down or slow (step 7, PR J) | an entry's strict commit that fails is refused, no request; a leg with admission evidence ends its attempt, a legacy leg re-arms under its backoff. One that times out is uncertain, no request and no re-arm. Reductions, stops and cancels wait at most 500 ms per write and go out anyway, also with the journal's memories full | entries refused, protection and reductions continue; unlanded rows retry through a bounded set | `robsond_journal_latch`, `robsond_journal_retry_set_depth`, warnings in the logs; no fence yet |
| Cancel outcome unknown (step 7, PR C) | an error, an indeterminate answer or an "already gone" with no final state is uncertain on the cancel's own intent; the protective action behind it proceeds | journaled only until the fence PR | `order_intents.cancel_class` and `uncertain_at`; boundary B |
| API slot class full (step 7, PR S) | 503 `busy` at once, nothing done; the other classes keep their slots | the caller retries | `robsond_api_slot_busy_total{class}` |
| Venue mutation without a permit (step 7, PR S) | while open: sent, counted, latch set; while stopping: refused, nothing sent | — | `robsond_unpermitted_mutations_total`, `robsond_journal_latch` |
| Operation dropped mid-flight: a panic, a cancelled internal future (step 7, PR S) | each intent it left open is sealed: one whose send may have started ends uncertain, one proven unsent ends not sent, both through the retry set; one with nothing to record is dropped. The permit counts as aborted and sets the latch | — | `robsond_aborted_operations_total`, `robsond_journal_latch` |
| Database down or refusing the epoch at start, or none configured for a venue other than the stub (step 7, PR S) | no start, before any venue request | the pod restarts | the error in the log |
| Venue REST dies | rules/costs/valuation stale → basis invalid, admission fails closed; risk reduction proceeds on the last validated rules or the persisted snapshot, with one refresh and resubmit per rejection; a leg holding neither waits for the lookup, then a decided exit goes out as given and a tick fails closed (§5) | reads from cache until TTL; protection on the venue, except a leg whose exit failed, until the heal | per-venue `eligible=false` + reasons; alert |
| Venue feed silent | watchdog after `ROBSON_FEED_SILENCE_MAX`; REST snapshot resync; trailing advances continue on REST ticks within the request budget | bounded REST polling | silent-feed metric, event |
| Daemon dies mid fan-out | accepted legs are protected on their venues except for a leg killed between entry fill and protection placement, which recovery protects or flattens first | startup recovery reads the durable journal and each venue's observation; completes, protects or releases per evidence | recovery events per leg |
| Daemon dies, venue unreachable or placement rejected inside a balance-account transition (N1-a) | the leg is unprotected for the whole interval, with no maximum duration; loss can exceed the priced envelope and every cap | retry, flatten, then re-protect-or-flatten as the first recovery action; admission on the venue blocked | `unprotected_inventory` gauge and alert while nonzero; transition event with duration; recovery event |
| Daemon dies mid replacement (position account) | old stop still live (place-then-cancel) | recovery heals from `open_protective_orders` | recovery event |
| Unknown order outcome (timeout) | reservation held; `order_by_client_id` before any retry; `Unknown` keeps the leg pending, possibly indefinitely (time does not resolve it) | no blind retry | event, `/status` pending count |
| Stale fees / rules / valuation | admission fails closed; risk reduction does not wait while it holds rules (§5) | — | `/status` per-venue staleness |
| Tick exit fails (position account) | the stop was cancelled first, so the leg has no exchange stop until the heal: at least 30 s, longer without ticks or held rules (§5); the software stop keeps evaluating | heal re-places the stop | `robsond_insurance_stop_stale_positions` gauge |
| Fill leaves a remainder on the account (position account) | the leg closes; the remainder is unauthored, without a stop, until a scan closes it, normally the next, with no upper bound (§5) | a remainder below the lot fails each scan until the operator clears it | UNTRACKED warning and rogue-position event each scan; `residual_inventory_recorded` for a short market exit only |
| Event log unreadable at restart | query error: startup aborts with a retryable exit, exchange stops stay live | unreadable or mismatched residual row: admissions blocked until the leg closes (§5) | warning or error in the logs, no durable event |
| Incomplete settlement | commission/pnl missing → month accounting flagged invalid, new legs blocked (ADR-0051) | — | `unmatched_income_count`, event |
| Unauthored inventory or position on an execution account | closed (ADR-0022); new legs blocked until the account is clean, a block `binance:usdm` does not have yet (§5) | — | `/status` per-venue discrepancy, event |

## Consequences

- Positive: a second venue is an adapter plus a factory arm; the risk numbers and the ADR-0050/0051 policies are
  untouched; spot long-only becomes a small ADR once N1 is decided; the regulatory single point of failure of 2026-11-04 is structurally addressed; failure modes
  are written down per venue.
- Negative / trade-offs: eight refactor PRs with no operator-visible feature before step 8; optional ports add branching;
  the consolidated basis needs every venue reachable and valued at month start; spot inventory accounting is new surface;
  N1-a accepts an unprotected interval without a guaranteed maximum duration, which position accounts did not have before step 6 (§5:
  a failed tick exit, a remainder and a stop gone from the venue can each go unprotected with no upper bound), and
  qualifies the ADR-0039 and ADR-0052 guarantees on balance accounts. A leg whose order outcome stays `Unknown` holds its
  global and per-venue admission capacity indefinitely: elapsed time, a restart or a month rollover does not release the
  reservation, only evidence or step 7's explicit resolution path does. During Fase S, a book that cannot be verified as
  drained also keeps admissions blocked across the month boundary (§8).
- Left on the table: cross-venue netting and routing, transfer automation, order-level authorship verification in
  reconciliation, shared execution accounts.

## Alternatives and decision matrix

| Option | Second venue cost | Safety of spot | Risk-ADR impact | Chosen |
|---|---|---|---|---|
| A. Keep `ExchangePort`, stub futures methods in spot adapters | low | unsafe (reconciliation and replacement assume a position account) | none on paper, broken in practice | no |
| B. Capability-split ports + required observation/settlement + durable journal (this ADR) | medium | safe by contract and conformance, modulo N1 | none on numbers; ADR-0039/0052 guarantees qualified on balance accounts under N1-a | **yes** |
| C. One crate per adapter now | medium+ | as B | none | later |
| D. Rename the persisted field | — | — | breaks ADR-0049 replay | no |

Budget model for multi-venue:

| Option | −4 % consolidated admission budget | Local monthly admission budget bounded (realized loss within the execution envelope; N1-a exposure excluded) | Fungibility | Chosen |
|---|---|---|---|---|
| Global only | yes | no (one venue can absorb the whole allowance) | full | no |
| Per-venue 4 % × Bᵢ, signed net (Σ = 4 % × ΣBᵢ for any weights) | yes | yes (4 %) | none | = N2 with λ = 4 % |
| Global + per-leg 1 % × Bᵢ, no monthly local cap | yes | no | high | no (v2's choice; the survival claim did not hold) |
| Global + per-venue monthly cap λ × Bᵢ + floor derived from λ | yes | yes (λ) | partial (λ > 4 %) | **yes**, λ per N2 |

---

## Needs Input (operator)

- **N1** — **decided 2026-11-05: N1-a.** Non-atomic transition exception to ADR-0039 with the exposure stated in §2.2
  (no guaranteed maximum duration; loss may exceed the envelope and the caps).
- **N2** Per-venue monthly cap λ (proposed 8 %; 4 % = pure per-venue budget) and `K` (proposed 3).
- **N3** Failed-leg policy (never redistribute, proposed) and retry authority for a venue that recovers mid-operation.
- **N4** Numeraire beyond USDT-quoted venues (USDC, BRL on B3): conversion source and staleness rule.
- **N5** Cost envelope source: venue API (Binance exposes commission rates) with a 24 h staleness rule, or configuration
  with an `as_of` date. **Required before step 8 activation**; recommendation: venue API with the 24 h rule, configuration
  only as an explicit, dated override.
- **N6** — **decided 2026-11-05 for Fase S**: the personal account's spot wallet is the execution account (see §8),
  with non-Robson holdings moved out beforehand; a dedicated sub-account on the corporate account replaces it once the
  PJ account exists. A separate API key on a shared wallet is not an option.

## Implementation Notes

- Step 1: `robsond/src/{binance_exchange,binance_ohlcv,daemon,main}.rs` → `robson-connectors/src/binance/usdm/*`.
- Step 2: `robson-exec/src/{ports,executor,stub}.rs`, `robson-connectors/src/binance/usdm/exchange.rs`, `robson-testkit`.
- Step 3: `robsond/src/{venue,daemon,main,config,api,market_data,event_bus,position_manager}.rs`, `robsond/src/funding/`,
  `robson-connectors/src/binance/usdm/market_data.rs`, `robson-exec/src/ports.rs` (`TradeFeedPort`).
- Step 4: `robson-domain/src/{credentials,entities,events,value_objects}.rs`, `robson-engine/src/lib.rs`,
  `robson-store/src/{credential_store,memory}.rs`, `robson-connectors/src/binance/*`, `robson-exec/src/ports.rs`,
  `robsond/src/{config,event_bus,position_manager,sse}.rs`, `robson-projector/src/types.rs`, golden fixtures.
- Step 5: `robsond/src/daemon.rs` (boundary), `robson-domain` (carried basis), migration for persisted carried bases
  and settlement attribution, persisted-state version bump.
- Step 6: `robson-exec/src/{quantize,executor,error,ports,venue,compat,stub}.rs`, `robson-engine/src/lib.rs`,
  `robson-domain/src/{entities,events,trading_rules}.rs`, `robson-store/src/memory.rs`, `robson-eventlog`, `robson-projector`,
  `robson-store/src/postgres.rs` (a close exactly at month start belongs to that month),
  `robsond/src/{position_manager,daemon,api,error,metrics,reconciliation_worker,startup_recovery,venue}.rs`, usdm adapter; bronze
  contract 1.2.0; before/after fixtures.
- Step 7: delivered in parts (§8). PR J: `robson-exec/src/{order_journal,executor,venue,stub,error}.rs`,
  `robsond/src/{order_journal_store,api,auth,daemon,metrics,position_manager,reconciliation_worker,venue}.rs`, migration 31; the
  attempt rows are read from the database rather than reloaded. PR C: `robson-exec/src/{cancel,compat,executor,order_journal,ports,stub}.rs`,
  `robsond/src/{daemon,position_manager,reconciliation_worker}.rs`, no migration. PR S: `robson-exec/src/{gate,order_journal,venue,stub}.rs`,
  `robson-connectors/src/binance_rest.rs` and the USD-M adapter, `robsond/src/{api,daemon,process_epoch,metrics,reconciliation_worker,
  position_manager,venue,error}.rs` and `funding/worker.rs`, `robson-testkit`, migration 32. What follows here is the original statement of the step, with its
  open obligations. `robson-exec/src/intent.rs` → store-backed journal; migration (journal tables, leg routing columns, legacy
  mapping); process-kill tests. Before any caller acts on `order_by_client_id` or `replace_protection`, this step must:
  persist each attempt durably before its request is sent and each rejection when observed, reload them on startup, treat
  an attempt without a persisted rejection as unresolved, and keep the truncated state across restarts. It must also give
  the permanently `Unknown` cases an explicit resolution path that never infers absence from time, including an attempt
  whose outcome never becomes known and a successor refused locally before anything was sent (`NoRecord` →
  `Unknown{new: None}`, old stop still live, nothing for a lookup to find); until resolved, the leg stays pending with its
  reservation held and visible. Step 6 removed that local refusal from USDⓈ-M `place_protection`; the rule stands for any
  venue that keeps one. This step also owns a crash between a fill and its events, a failed durable append of a
  residual event (§5), and two ownership races that exist before step 6 and need the per-leg transition coordination of §2.2: the worker
  classifies a remainder as UNTRACKED, a new entry on that symbol and side fills, and the worker reduces inventory the new leg owns; and
  the orphan sweep, working from a stale covered set, cancels a stop accepted after that set was read, recorded or not, including an initial
  stop on a new leg, so the leg can record a cancelled stop until the next sweep schedules the heal (§5). This step must revalidate ownership
  in coordination with cancellation and test a stop recorded before the cancel. Step 6's wider sweep adds some exposure to the second, and its
  refresh and resubmit lengthen the first's window.
- Step 8: `robson-connectors/src/binance/spot/*`; conformance suite; `ROBSON_VENUE=binance_spot`; executor's
  `validate_futures_settings` gated to position accounts (§4). It must also, before activation: replace the same-id resubmit, since
  spot has no reduce-only bound (§5); decide what a fill short of the leg and a remainder mean on a balance account, because step 6's
  answer on position accounts relies on a reduce-only bound and on ADR-0022 closing the rest; price a residual at the mark (§2.3);
  account per fill, so that a partial counts in the month's net and a close stops booking the whole leg at its last fill (§5); and
  design for dust. A fee charged in the base asset can leave inventory below the lot; how often has not been measured. Review trail
  v11 records the two USDⓈ-M designs withdrawn in step 6 and why.
- Evidence: `docs/evidence/2026-11-04-binance-exchangeinfo-btcusdt.md` (retained in this PR): timestamped verbatim
  extracts with source URLs and SHA-256 of the raw responses; spot `orderTypes` includes `STOP_LOSS`, `LOT_SIZE` step
  0.00001, `NOTIONAL` min 5 USDT (applyMinToMarket, avgPriceMins 5); futures `LOT_SIZE` step 0.001, `MIN_NOTIONAL` 50
  USDT, `orderTypes` includes `STOP_MARKET`. Symbol availability only; account authorisation per order type is an
  activation gate (§2.1). The DB-gated test annotations reproduce from source (68 when written, 76 after step 3); security workflows are unchanged.

## Review trail

- v1 (2026-11-04): Codex gpt-6.1-sol xhigh, REQUEST_CHANGES, 15 findings (P0: stop-limit protection; P1: replacement/close
  semantics, optional reconciliation, equity definition, costs, credential identity, quantisation equivalence, funding,
  atomic reservation, leg identity, viability floor, sequencing; P2: serialization claims, per-venue budget rationale).
- v2 (2026-11-04): Codex, REQUEST_CHANGES, 14 findings (P0: `cancelReplace` is not atomic; P0: cancel-then-sell exposes
  inventory; P1: ADR-0022 contradiction, spot floor arithmetic, local stress premise, unrealized P&L at the boundary,
  volatile intent journal, client-id lookup, funding gating, metadata rules for exits, sequencing accuracy, measurable
  acceptance; P2 serialization; Low evidence).
- v3: both P0s are made an explicit operator decision (N1) with the exact exposure stated; ADR-0022 preserved via
  dedicated execution accounts; floor arithmetic redone on a single feasible quantity; survival derived from an enforced
  λ (N2); unrealized P&L separated from the basis change (step 5); durable journal (step 7); client-id lookup in
  `TradingPort`; funding gated and routed; separate admission vs risk-reduction metadata rules; eight-step sequencing with
  honest rollback boundaries; measurable acceptance criteria; serialization contract narrowed.
- v3 (2026-11-04): Codex, REQUEST_CHANGES, 9 findings (P0: "bounded window" still false under failure; P1: arbitration
  from final outcomes and finality rule, account-wide inventory enumeration, spot `q*` not shown feasible at the stop
  region and net of base commission, slot-count ceiling and guardrail bounds, funding worker vs saga recovery conflict,
  migration/rollback inconsistencies; P2: local-cap semantics (signed net, proposed reservation), survival
  qualifications; Low: evidence absent). Arithmetic of 892.17 / 62.45 USDT reproduced by the reviewer.
- v4: N1-a restated without a duration bound and with the loss statement; transition intent persisted before cancel;
  `unprotected_inventory` gauge; residual from final outcomes; `VenueFinalityRule`; account-wide `account_observation`;
  `q*` solved at the stop region with 8 lots (71.37 / 81.09 USDT) and base-fee case deferred to N5; slot ceiling removed,
  per-operation query budgets and aggregate limits added; funding recovery workers per saga route; step 5 migration and
  step 7 leg routing; signed-net local cap with the proposed reservation; evidence file retained.
- v4 (2026-11-04): Codex, REQUEST_CHANGES, 4 findings (P1: `TriggerAlreadyCrossed` treated as execution evidence; P2:
  database row and budget wording not yet qualified by N1-a, §10 asserting pre-quantization weights, display rounding
  71.37/81.09 vs ceilings 71.38/81.10). Resolved 3, 5, 6, 7, 9 of round 3.
- v5: crossed trigger carries the old stop's final state and residual and routes to flatten; admission property on
  post-quantization charges; database row, budget statement and matrix qualified; ceilings corrected.
- v5 (2026-11-04): Codex, **APPROVE** (architecture acceptable at the ADR level with N1–N6 decided by the operator;
  implementation acceptance gates and rollout pending). One P3 nit (§7 `R × wᵢ` vs `A = Σ aᵢ` notation) applied.
- v6 (2026-11-05): operator decisions recorded (N1-a, Fase S order, personal spot wallet as execution scope). Codex
  REQUEST_CHANGES on the reorder: P1 interim accounting contract for the deferred step 5 (added to §8: persisted interim
  basis, no carried legs across a month boundary until step 5, quiesce and drain); P3 "dedicated account scope" wording
  in §1. Both applied.
- v7 (2026-11-05): Codex confirmed the round-6 fixes; P2: N5 (cost source) must be resolved before step 8 activation.
  Applied to the status line and N5.
- v8 (2026-11-06): §2 aligned with step 2 as implemented, after Codex rounds 1–4 on `refactor/venue-step2-ports` (APPROVE
  on round 4): the time-based finality wording is replaced by `EveryAttemptRejected` and `SubmitAttemptLedger`; result
  shapes (`PlacedStop`, `reason`, `Unknown{new}`, `final_state`, `AccountOpenOrder`) and cursor-less signatures recorded;
  step 7 obligations added. No risk number or sequencing changed. Codex REQUEST_CHANGES on the alignment, 2 P2: the
  failure-mode table still promised resolution by lookup, and Consequences did not state that `Unknown` legs can hold
  admission capacity indefinitely. Both applied; Codex **APPROVE** on the result.
- v9 (2026-11-06): §2, §3 and §8 aligned with step 3 as implemented after Codex rounds 1–2 on
  `refactor/venue-step3-factory` (APPROVE on round 2). Codex required the delivered contracts recorded and every omission
  staged with an owner: durable saga routing (step 7), conversion exclusion and `eligible` inputs (step 8), reconnect resync
  and feed properties (separate change before a second venue). No risk number or operator decision changed. Codex
  **APPROVE** with one P3 (§1 wording) applied.
- v10 (2026-11-06): §4 and the Implementation Notes aligned with step 4 as implemented on `refactor/venue-step4-naming`:
  `Endpoints::base_url` is not a host table, what keeps the old names and why, and step 8 owns gating
  `validate_futures_settings` to position accounts. No risk number or operator decision changed. Codex **APPROVE** on
  round 1, with four P3s applied (the `/events/history` exposure above, a missing-versus-null assertion, fixture provenance
  and the stale DB-gated count).
- v14 (2026-11-10): §8, the failure modes and the Implementation Notes aligned with step 7 PR S as implemented against design note v8, in
  parts 1 and 2 of three: permits and slots, process epochs. Closes the J boundary that ended an entry's operation before its stop.
  Codex round 1 on the three parts: REQUEST_CHANGES (1 P1, 2 P2, a vacuous required test, P3s). Since then an unpermitted mutation is
  latched under the gate's mutex, an API operation runs in its own task, so a client's disconnect no longer cancels it, the `/panic`
  capacity test closes a real leg, and a venue that can trade does not start without an epoch. No risk number changed.
- v13 (2026-11-09): §8 and the failure modes aligned with step 7 PR C as implemented against design note v8: typed cancel classification,
  uncertainty journaled and not fencing until the fence PR. No risk number changed.
- v12 (2026-11-08): §2's finality paragraph, §8 and the failure modes aligned with step 7 PR J as implemented in three stacked PRs,
  against design note v8. Codex round 1 on the first draft: REQUEST_CHANGES (3 P1, 5 P2); interrupted cancels, cancelled result
  writes, the no-send recovery on `ExecutableSpan` legs, discarded ids, pool exhaustion, evidence validation, fault targeting and
  query bounds fixed. Round 4: PR 3 REQUEST_CHANGES (1 P2, 1 P3); a retry refused on a discarded id takes the same fallback. Round 3: PR 3 REQUEST_CHANGES (1 P2); a resubmission the journal refuses now goes out once under a fresh id. Round 2: REQUEST_CHANGES (1 P1, 5 P2, P3s); a full journal no longer refuses a bounded first send, reads and the startup
  probe retire their connections, a cancellation's evidence names the cancelled order, and typed no-sends and returned futures-settings rejections reach the amended policy, while the retained pre-send barriers and other error returns do not. States that uncertainty is journaled and does not fence until the fence PR, and boundary B as a view. No risk
  number changed.
- v11 (2026-11-07): the header, §2, §2.2, §2.3, §3, §5, §8, §10, the failure modes and the Implementation Notes aligned with
  step 6 as implemented on `refactor/venue-step6-quantize`. Codex rejected two wider USDⓈ-M designs. Rounds 1 and 2: exit-first with
  a cancel-then-resubmit fallback, a partial exit reducing the leg's quantity with P&L accumulated while open, and inventory with no
  live stop charged at the mark (7 P1 + 4 P2, then 10 P1 + 2 P2). Round 3: a leg kept `Active` after a short fill and closed only on
  a flat venue read (7 P1 + 3 P2; five in that lifecycle, one not fixable on today's port, because a position read carries no evidence
  that it follows the fill). Round 4 on the current rule: 5 P1 + 3 P2 on stop evidence, the withdrawal race and request accounting,
  applied except for two ownership races staged to step 7, and an invalid base's halt, which stayed as ADR-0051 §1 then decided (since amended, 2026-11-07). Round 5:
  1 P1 + 2 P2 + 2 P3, mostly on a worker lookup that closed a leg on its dead stop, whose coverage check could not be tied to the stop;
  the lookup was withdrawn. Round 6: 1 P1 + 2 P3, rejecting a dead recorded stop as a stated limit; the sweep now re-places a recorded stop
  gone from the venue. Round 7: 1 P1 + 2 P3, a replaced stop's later fill leaving the budget; displaced stops now stay evidence. Round 8:
  2 P1 + 2 P3 on that evidence; it now covers every displacement and counts only a whole fill. Round 9: **APPROVE**, with two P3 on
  documentation applied after the verdict, with no new round. **Operator decisions (2026-11-06 and 2026-11-07): narrow the step twice**, since the operator cannot trade
  that venue and spot needs a different model. So this version moves three promises: §2.3's residual priced at the mark goes to step 8;
  "never declared closed" no longer covers a leg with a remainder on a position account, only the remainder; and §5's retained
  protection does not cover the tick's market exit, which stays cancel-first. Per-fill accounting also goes to step 8. No risk constant
  changed. MonthlyHalt changed in one direction: realized losses exhausting the budget on a valid base now fire it with latent risk
  unresolved.
