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
monthly budget, atomic reservation, cost-priced boundary downside), ADR-0049 (event payloads do not change)
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
  `b3:btc-fut`) used in config, logs, `/status`, allocations and, later, on positions and market-data events.
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
  trading_rules / trading_rules_preview                      (issue #154 contracts unchanged)
  place_market_order(symbol, side, qty, client_id, intent: EntryOrReduce) -> OrderResult
  place_protection(symbol, close_side, qty, trigger, client_id) -> StopRef
  cancel_protection(symbol, StopRef) -> StopCancelOutcome
      StopCancelOutcome = { Cancelled{filled_before_cancel: Decimal, fills}, AlreadyGone{final: OrderLookup}, Unknown }
  replace_protection(symbol, old: StopRef, new: StopParams) -> ReplaceOutcome
      ReplaceOutcome = { Replaced{new, old_filled: Decimal}, OldKeptNewRejected, OldCancelledNewRejected{old_filled},
                         TriggerAlreadyCrossed{old: OrderLookup, residual: Decimal}, Unknown }
      TriggerAlreadyCrossed is a venue rejection of the new trigger, not execution evidence; `old` is the old stop's
      final state with authoritative cumulative fills and `residual = governed − old_filled`
  order_by_client_id(symbol, client_id) -> OrderLookup
      OrderLookup = { Absent{finality: VenueFinalityRule}, Open{filled_qty}, PartiallyFilled{..}, Terminal{fills,
                      cancelled_qty}, Unknown }
      Absent is returned only under the venue's documented finality rule (e.g. not found on both the live and the
      historical lookup after the venue's propagation window); a raw not-found response is Unknown
  order_by_exchange_id / protection_fill(symbol, StopRef) / open_protective_orders
  user_trades_since(cursor) / price / health

AccountPort                      required
  cash(asset) -> Cash { free, locked, as_of }
  valuation(symbol) -> Mark { price, as_of }
  execution_costs(symbol) -> ExecutionCostEnvelope { maker_bps, taker_bps, gap_bps, fee_asset_policy,
                                                     funding: Option<FundingTerms>, as_of, source }
  capabilities() -> VenueCapabilities

PositionAccountPort              required when account_kind = PositionAccount
  validate_position_settings(symbol, expected_leverage = 1)  (One-way)
  position_snapshot() -> { wallet_balance, unrealized_pnl, positions: Vec<ExchangePosition>, as_of }
  close_position_market(symbol, side, qty, client_id)

InventoryPort                    required when account_kind = BalanceAccount
  account_observation() -> { holdings: Vec<(Asset, Holding{free, locked})>   every nonzero holding, all assets,
                             open_orders: Vec<OpenOrderRef>,                   independent of configured symbols
                             as_of }
  governed_inventory(symbol) -> GovernedInventory{qty, fill_ids}            derived from authoritative fills

SettlementPort                   required (every venue)
  settlement_since(cursor) -> Vec<SettlementRecord>      realized pnl, commission (amount, asset), funding
                                                         where applicable; derived from authoritative fills and
                                                         account records when the venue has no income endpoint

FundingPort                      optional (spot→derivatives conversion feature, position accounts only)
  spot_balances / spot_price / spot_symbol_is_trading / place_spot_market_order / spot_order
  transfer / transfer_history

MarketDataPort, OhlcvPort        required; built by the same factory; every PriceUpdate / Candle carries VenueId

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
`client_id` is idempotent only within the venue's finality rule; a `Terminal` outcome releases only the liabilities
proven extinguished: filled inventory and unsettled costs stay charged until settlement evidence arrives.

**2.3 Reconciliation is never optional.** The reconciliation worker, startup recovery, the live-risk filter, orphan-stop
recognition and `/status` sync consume an `AccountObservation` produced by the venue: `Positions(snapshot)` for position
accounts, `Inventory(account_observation)` for balance accounts, in both cases **account-wide** (every symbol, every
asset, every open order in the operated account scope, independent of configured or tracked symbols). Governed inventory
is identified through authoritative fills; everything else is unauthored and is closed (ADR-0022, policy unchanged).
A residual that cannot be executed (below the lot or notional minimum) is a visible unresolved liability with durable
evidence, priced conservatively at the venue mark, never declared closed. A discrepancy between governed inventory and
Robson's leg quantity blocks new legs on that venue and surfaces in `/status`. The existing gap (matching by symbol/side
rather than order-level authorship) stays a follow-up.

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

- `robsond/src/{binance_exchange,binance_ohlcv}.rs` and the futures WebSocket wiring in `market_data.rs` move to
  `robson-connectors/src/binance/usdm/` (mechanical; `normalize_market_quantity` moves unchanged).
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
  (`market_data.rs:68,253`) are preserved unchanged by the relocation and become per venue; on reconnect the venue resyncs
  from a REST snapshot anchored at the last applied tick; a leg's technical analysis uses its own venue's OHLCV. No feed
  timing changes in this ADR.

### 4. Domain cleanup with stable encodings

- **Credential identity is not the venue.** `robson_domain::Exchange` keeps its serialized values (`"binance"`,
  `"binance_testnet"`): they are the AES-GCM AAD, the store query key and the SQL CHECK. A venue is `(Exchange,
  MarketKind, profile)`; `binance:spot` reuses the `"binance"` credential. `base_url()` leaves the domain for
  `robson_connectors::binance::Endpoints`. A new exchange adds a credential value through an explicit migration with
  old-ciphertext compatibility tests.
- `as_binance_str` leaves `ports.rs`; wire encodings belong to adapters.
- `binance_position_id` → `exchange_position_id` in Rust only, on **both** the event payload and the independently
  serialized `Position`, each with `#[serde(rename = "binance_position_id")]`. Compatibility contract: canonical
  serialization before and after the refactor is equal (fixtures with null and non-null values; a missing optional field
  reserializes as `null` before and after, which is the existing behaviour); historical payloads deserialize; the legacy
  SQL column, the active projector payload (which does not carry the field) and the API/frontend field are unchanged.
- Sizing vocabulary: the "margin cap" in `size_entry` is named the **capital cap** (`notional ≤ available_quote × (1 −
  headroom)`, leverage constant 1); numbers unchanged. `RiskConfig::leverage()` is validated only by position accounts.

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
| 1 | Mechanical relocation of the two adapters and the futures WS wiring into `robson-connectors`; `ExchangePort` unchanged | preserving | revert alone while nothing else imports the new paths; afterwards revert together with its dependants |
| 2 | New port contracts (§2) + USDⓈ-M adapter implementing them; legacy `ExchangePort` kept as a forwarding compatibility layer; separate `unrealized_pnl` field added, `wallet_balance` untouched | preserving | revert 2 (and 3 if merged) |
| 3 | `VenueFactory`, `with_venue`, feed injection with `(VenueId, Symbol)` keys, additive `/readyz.venue` | preserving for execution; additive for readiness consumers | revert alone |
| 4 | Rust naming and endpoint cleanup with stable event, `Position` and credential encodings; fixtures | preserving | revert alone |
| 5 | Accounting: governed equity derived once; month-boundary basis uses ADR-0051 §3 cost-priced downside; **migration** for persisted carried bases with compatible settlement attribution and replay handling; atomic reset | **changing** (basis, persisted state version) | once new accounting state is written, rollback needs demonstrated reader/writer compatibility of the older binary with that state, or a verified book free of risk-bearing and unsettled liabilities; keeping `capital_base` alone is insufficient |
| 6 | Quantisation and metadata rules (§5) with before/after fixtures per order kind | **changing** | revert alone only with a verified empty book (the old helper is incompatible with spot lots) |
| 7 | Durable intent journal in the store (migration): intents and results persisted in order relative to reservations, stable venue-scoped client ids, explicit unresolved states, transition intents (§2.2); **durable venue/account routing on every risk-bearing and unsettled leg**, with the legacy mapping of every existing position to `binance:usdm`; process-kill tests at every external-call/persistence boundary | preserving (adds evidence and identity) | revert alone with an empty book; after step 8 only together with step 8 |
| 8 | Spot venue acceptance behind `ROBSON_VENUE=binance_spot`: `InventoryPort`, protection protocol per N1, settlement from fills, conformance suite green; the long-only entry gate is a separate ADR | new venue | disabling a venue stops new admissions on it but **keeps managing its open and unsettled legs** through the step-7 routing; a binary rollback requires a verified empty book on that venue |

Migrations: none in steps 1–4 and 6; step 5 (carried bases) and step 7 (intent journal, leg routing) ship with explicit
migrations and persisted-state versions. Every rollback names its transitive dependency prefix: a step is reverted only
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
  the steady-state ceilings, recorded per incident. Degraded mode (feed silent) keeps the same ceilings.
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
  decryption after step 4; replay fixtures; the 68 DB-gated tests; security gates unchanged (gitleaks, osv-scanner,
  cargo-audit, SHA-pinned actions); allocation and arming mutations frozen during a request (guardrails §4).

---

## Failure modes (external dependencies and this service)

| Dependency / event | What happens | Degraded mode | Visibility |
|---|---|---|---|
| Database dies | no admission, no new legs; already-installed protection remains on the exchange; inventory in an N1-a transition may remain unprotected without a duration or loss bound; no cancellation starts without a durable intent, and unfinished transitions retain liabilities and alerts and resume from that intent on recovery; intent results cannot be persisted → in-flight operations enter `Unknown` and are resolved by lookup on recovery | read-only `/status` from memory; arms refused | `/readyz.database=false`; `unprotected_inventory` gauge; recovery events |
| Venue REST dies | rules/costs/valuation stale → basis invalid, admission fails closed; risk reduction proceeds on the persisted rules snapshot with retries | reads from cache until TTL; protection on the venue | per-venue `eligible=false` + reasons; alert |
| Venue feed silent | watchdog after `ROBSON_FEED_SILENCE_MAX`; REST snapshot resync; trailing advances continue on REST ticks within the request budget | bounded REST polling | silent-feed metric, event |
| Daemon dies mid fan-out | accepted legs are protected on their venues except for a leg killed between entry fill and protection placement, which recovery protects or flattens first | startup recovery reads the durable journal and each venue's observation; completes, protects or releases per evidence | recovery events per leg |
| Daemon dies, venue unreachable or placement rejected inside a balance-account transition (N1-a) | the leg is unprotected for the whole interval, with no maximum duration; loss can exceed the priced envelope and every cap | retry, flatten, then re-protect-or-flatten as the first recovery action; admission on the venue blocked | `unprotected_inventory` gauge and alert while nonzero; transition event with duration; recovery event |
| Daemon dies mid replacement (position account) | old stop still live (place-then-cancel) | recovery heals from `open_protective_orders` | recovery event |
| Unknown order outcome (timeout) | reservation held; `order_by_client_id` before any retry; `Unknown` keeps the leg pending | no blind retry | event, `/status` pending count |
| Stale fees / rules / valuation | admission fails closed; risk reduction unaffected (persisted snapshot) | — | `/status` per-venue staleness |
| Incomplete settlement | commission/pnl missing → month accounting flagged invalid, new legs blocked (ADR-0051) | — | `unmatched_income_count`, event |
| Unauthored inventory or position on an execution account | closed (ADR-0022); new legs blocked until the account is clean | — | `/status` per-venue discrepancy, event |

## Consequences

- Positive: a second venue is an adapter plus a factory arm; the risk numbers and the ADR-0050/0051 policies are
  untouched; spot long-only becomes a small ADR once N1 is decided; the regulatory single point of failure of 2026-11-04 is structurally addressed; failure modes
  are written down per venue.
- Negative / trade-offs: eight refactor PRs with no operator-visible feature before step 8; optional ports add branching;
  the consolidated basis needs every venue reachable and valued at month start; spot inventory accounting is new surface;
  N1-a accepts an unprotected interval without a guaranteed maximum duration that position accounts do not have, and
  qualifies the ADR-0039 and ADR-0052 guarantees on balance accounts.
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

- Step 1: `robsond/src/{binance_exchange,binance_ohlcv,market_data,daemon,main}.rs` → `robson-connectors/src/binance/usdm/*`.
- Step 2: `robson-exec/src/{ports,executor,stub}.rs`, `robson-connectors/src/binance/usdm/exchange.rs`, `robson-testkit`.
- Step 3: `robsond/src/{daemon,main,config,api,market_data,event_bus,position_manager}.rs`.
- Step 4: `robson-domain/src/{credentials,entities,events,lib}.rs`, `robson-engine/src/lib.rs`, `robson-store/src/memory.rs`, fixtures.
- Step 5: `robsond/src/daemon.rs` (boundary), `robson-domain` (carried basis), migration for persisted carried bases
  and settlement attribution, persisted-state version bump.
- Step 6: `robson-exec/src/quantize.rs`, `robsond/src/position_manager.rs`, usdm adapter; before/after fixtures.
- Step 7: `robson-exec/src/intent.rs` → store-backed journal; migration (journal tables, leg routing columns, legacy
  mapping); process-kill tests.
- Step 8: `robson-connectors/src/binance/spot/*`; conformance suite; `ROBSON_VENUE=binance_spot`.
- Evidence: `docs/evidence/2026-11-04-binance-exchangeinfo-btcusdt.md` (retained in this PR): timestamped verbatim
  extracts with source URLs and SHA-256 of the raw responses; spot `orderTypes` includes `STOP_LOSS`, `LOT_SIZE` step
  0.00001, `NOTIONAL` min 5 USDT (applyMinToMarket, avgPriceMins 5); futures `LOT_SIZE` step 0.001, `MIN_NOTIONAL` 50
  USDT, `orderTypes` includes `STOP_MARKET`. Symbol availability only; account authorisation per order type is an
  activation gate (§2.1). The 68 DB-gated test annotations reproduce from source; security workflows are unchanged.

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
