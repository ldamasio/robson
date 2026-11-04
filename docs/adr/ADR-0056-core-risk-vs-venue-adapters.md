# ADR-0056 — Core Risk Policy vs Venue Adapters: Port Contracts, Adapter Relocation, Multi-Venue Execution Model

**Date**: 2026-11-04 (v2 after Codex gpt-6.1-sol xhigh architecture review, 15 findings, all addressed or carried into Needs Input)
**Status**: PROPOSED (operator decision pending)
**Deciders**: RBX Systems (operator + architecture)
**Amends**: [ADR-0024](ADR-0024-trading-policy-layer.md) §6 (capital base source becomes venue-neutral and names its valuation),
[ADR-0039](ADR-0039-exchange-side-insurance-stop.md) (the protective stop becomes a venue contract with an explicit lifecycle)
**Preserves**: ADR-0021 (level selection), ADR-0022 (reconciliation is never optional), ADR-0050 (valid-level selection,
no-valid-stop semantics, executable risk costing), ADR-0051 (net-from-start monthly budget, atomic reservation),
ADR-0052 (executable span, market exits, two-layer same-trigger enforcement), ADR-0049 (event payloads do not change)
**Related**: issue #154 (`SymbolTradingRules`), TD-2026-05-05-001, 2026-10-30 security assessment, Codex review of v1 (2026-11-04)

---

## Context

On 2026-11-04 Binance removed the `Enable Futures` permission from the production API key and refuses to
re-enable it ("not available in your region"). Its futures UI states that futures are available on ADGM
accounts only and that Binance cannot offer derivatives to residents of Brazil (Lei 6.385/1976). CVM has
stated that no crypto venue is authorised to offer derivatives to Brazilian investors and that the BCB SPSAV
licence does not cover them. Robson's only execution path, Binance USDⓈ-M perpetuals, is closed for a
Brazilian operator (CPF or CNPJ). The daemon still reads balances and income; every order fails with `-2015`.

Operator direction: long-only spot as the next execution path; hedge/short adapters outside Binance later
(B3 bitcoin futures is the only regulated derivative venue for Brazilians); and, as a standing goal, one
logical position executed across several operator-selected venues with capital distributed between them.

The risk core is venue-neutral in substance (level selection, executable span, adverse-fill costing, monthly
budget, hand-span trailing stop never ask which exchange they run on). The leaks are of shape. Survey of
2026-11-04, confirmed line by line by the Codex review:

| # | Leak | Where |
|---|------|-------|
| 1 | `ExchangePort` is modelled on Binance USDⓈ-M with spot methods mixed in (`validate_futures_settings`, `get_futures_balance`, `get_all_open_positions`, `close_position_market`, `universal_transfer`, `get_transfer_history`, `get_spot_*`, `place_spot_market_order`) | `robson-exec/src/ports.rs` |
| 2 | Production adapters live in the daemon; the daemon is built on the concrete type (`Daemon<BinanceExchangeAdapter, _>`, `new_binance*`) | `robsond/src/binance_exchange.rs`, `binance_ohlcv.rs`, `daemon.rs:359-442` |
| 3 | Market data is constructed on the futures WebSocket client outside any factory | `robsond/src/market_data.rs:37,53,355` |
| 4 | The position manager imports a quantisation helper from the adapter; the helper hard-codes a `0.001` step and checks only the minimum | `robsond/src/position_manager.rs:58,2225`, `binance_exchange.rs:40-50,87` |
| 5 | Persisted event, entity, SQL column, projector payload and frontend field are all named `binance_position_id` | `robson-domain/src/events.rs:251`, `entities.rs:73`, `migrations/20240101000005_add_binance_position_id.sql`, `frontend/src/lib/api/robson.ts:59` |
| 6 | The domain `Exchange` enum carries endpoints (`base_url()`); its serialized value is also the credential identity (AES-GCM AAD, store queries, SQL CHECK) | `robson-domain/src/credentials.rs:90,149`, `robson-store/src/credential_store.rs:459`, `migrations/20240101000004_exchange_credentials_multi_tenant.sql:20` |
| 7 | Binance wire encodings in the port layer (`as_binance_str`) | `robson-exec/src/ports.rs:74,603` |
| 8 | Execution costs default to the Binance futures taker rate and are read as one scalar from `RiskConfig`; accounting adds scalar fees to quote P&L although the port allows other fee assets | `robson-domain/src/value_objects.rs:302`, `executable_stop.rs:345`, `robsond/src/position_manager.rs:4124` |
| 9 | Reconciliation, startup recovery, live-risk filtering, orphan-stop recognition and `/status` sync all start from `get_all_open_positions()` and `reduce_only`, i.e. from a position account | `robsond/src/reconciliation_worker.rs:379,413,580`, `daemon.rs:1064`, `position_manager.rs:420,3699`, `api.rs:1408` |
| 10 | The executor's stop replacement (place-then-cancel) and close (`reduce_only = true`) rely on derivatives semantics | `robson-exec/src/executor.rs:440,552` |
| 11 | The funding saga uses spot balances, spot prices, symbol availability, order lookup by client id and base/quote orders, then refreshes capital from the futures wallet | `robsond/src/funding/saga.rs:70,160,249,303,389`, `daemon.rs:733` |

None of these touch a risk number. All of them make a second venue expensive, and 9–11 would make a spot
venue *unsafe* if the split simply made the futures-only methods optional.

### Target picture

```text
            operator (API / UI)
                  │  arm LogicalOperation{symbol, side, legs: [(venue, weight)]}
                  ▼
   ┌──────────────────────────────┐
   │  risk core (venue-neutral)   │  ADR-0021/0050/0051/0052: levels, span, costing,
   │  admission · budget · stops  │  atomic reservation, hand-span trailing stop
   └──────────────┬───────────────┘
                  │ per leg: plan {qty, executable stop, cost envelope}
      ┌───────────┼─────────────────┐
      ▼           ▼                 ▼
 Venue binance:usdm   Venue binance:spot     Venue b3:btc-fut (later)
 ┌──────────────┐   ┌──────────────┐        ┌──────────────┐
 │ Trading      │   │ Trading      │        │ Trading      │   required ports
 │ Account      │   │ Account      │        │ Account      │
 │ PositionAcct │   │ Inventory    │        │ PositionAcct │   one of the two is required
 │ Income       │   │ (fills only) │        │ Income       │   settlement evidence always required
 │ Funding      │   │ —            │        │ —            │   optional
 │ MarketData   │   │ MarketData   │        │ MarketData   │   required (own feed)
 └──────────────┘   └──────────────┘        └──────────────┘
```

---

## Decision

### 1. Vocabulary

- **Venue** = exchange × market kind × account scope. `VenueId` is a stable string (`binance:usdm`,
  `binance:spot`, `b3:btc-fut`) used in config, logs, `/status`, allocations and, later, on positions.
- **Market kind**: `PerpUsdM` (position account, funding, reduce-only) or `Spot` (balance account, holdings,
  protection is an order on the book). Dated futures later.
- **Account kind**: `PositionAccount` (the venue reports positions) or `BalanceAccount` (the venue reports
  holdings only; Robson derives its governed position from its own fills).
- **Leg**: the execution of a logical operation on one venue. Today every position is a single leg.
- **Governed holding**: base-asset inventory that Robson bought for a leg, net of base-asset fees. Robson never
  sells inventory it did not buy (an operator's own spot holdings on the same account are out of scope of
  every automatic action).

### 2. Port contracts (replace `ExchangePort`)

Names are venue-neutral; Binance mechanics move to the adapter docs. All traits are `#[async_trait]` with
`Send + Sync` supertraits; optional capabilities are exposed as borrowed accessors on the venue object.
A runtime-selected venue is represented by a sized dispatch enum in `robsond` (`VenueKindRuntime { Usdm(..),
Spot(..), Stub(..) }`) that forwards to the generic code paths; `Executor<E: Venue, S: Store>` stays generic.
Capability flags and accessor availability are checked for agreement by the conformance suite (§9).

```text
TradingPort                      required
  trading_rules / trading_rules_preview            (issue #154 contracts unchanged)
  place_market_order(symbol, side, qty, client_order_id, intent: EntryOrReduce)
  place_protective_stop(symbol, close_side, qty, trigger, client_order_id) -> StopRef
  replace_protective_stop(symbol, old: StopRef, new: StopParams) -> ReplaceOutcome
  cancel_protective_stop(symbol, StopRef) -> StopCancelOutcome    (AlreadyGone semantics kept)
  cancel_order / open_protective_orders / order_by_exchange_id / protective_stop_fill
  user_trades_since / price / health

AccountPort                      required
  cash(asset) -> Cash { free, locked }
  valuation(symbol) -> Mark { price, at }                       (for governed inventory)
  execution_costs(symbol) -> ExecutionCostEnvelope { maker_bps, taker_bps, gap_bps, fee_asset_policy,
                                                     funding: Option<FundingTerms>, as_of }
  capabilities() -> VenueCapabilities

PositionAccountPort              required when account_kind = PositionAccount
  validate_position_settings(symbol, expected_leverage = 1)       (One-way mode)
  open_positions() -> Vec<ExchangePosition>
  close_position_market(symbol, side, qty, client_order_id)

InventoryPort                    required when account_kind = BalanceAccount
  holdings(asset) -> Holding { free, locked }
  governed_inventory(symbol) -> GovernedInventory { qty, from_fills: Vec<fill ids> }
  unmatched_inventory(symbol) -> Decimal       (held base beyond governed legs: reported, never sold)

SettlementPort                   required (every venue)
  settlement_since(cursor) -> Vec<SettlementRecord>   realized pnl, commission (with asset),
                                                      funding where applicable; derived from
                                                      authoritative fills/account records when the venue
                                                      has no income endpoint; never omitted

FundingPort                      optional (spot→derivatives conversion feature)
  spot_balances / spot_price / spot_symbol_is_trading / place_spot_market_order / spot_order
  transfer / transfer_history

MarketDataPort, OhlcvPort        required, supplied by the same factory as the venue (own endpoints)

trait Venue: TradingPort + AccountPort {
  fn id(&self) -> &VenueId;
  fn position_account(&self) -> Option<&dyn PositionAccountPort>;
  fn inventory(&self) -> Option<&dyn InventoryPort>;
  fn settlement(&self) -> &dyn SettlementPort;
  fn funding(&self) -> Option<&dyn FundingPort>;
}
```

`VenueCapabilities` is data: `market_kind`, `account_kind`, `supports_short`, `reduce_only_wire_flag`,
`protection: ProtectionKind`, `replacement: ReplacementProtocol`, `has_funding`. The executor and the daemon
branch on capabilities, never on the venue id.

**Protection kind (resolves the v1 P0).** An eligible venue must provide exchange-resident protection that
triggers **market** execution at the resolved executable trigger: `ProtectionKind::MarketOnTrigger`.
Binance USDⓈ-M provides it as a conditional `STOP_MARKET`; Binance spot provides it as `STOP_LOSS`
(market on trigger; `STOP_LOSS` is listed in BTCUSDT `orderTypes` on 2026-11-04). A venue whose only
protection is stop-limit (`StopLimitOnBook`) is **ineligible** under ADR-0039/0052: a stop-limit can trigger
and rest unfilled while price keeps moving. Admitting such a venue requires a policy amendment, not a
capability flag.

**Protective-order lifecycle contract (resolves v1 P1-2).** The venue declares its `ReplacementProtocol`:
- `PlaceThenCancel` (position accounts, reduce-only stops: the current executor path, ADR-0039 / #154 d6);
- `AtomicCancelReplace` (balance accounts: the open stop **locks** the inventory, so a second full-quantity
  stop cannot coexist; the venue must expose an atomic cancel-replace whose failure mode leaves the old stop
  in place — Binance spot `cancelReplace` with `STOP_ON_FAILURE`).
The contract also fixes: who owns the inventory while a stop is open (the stop); close arbitration on a
balance account (cancel the stop, then sell the governed quantity, with the intent journal recording both
steps so startup recovery either completes the sell or re-arms protection); partial execution (the governed
quantity is the net filled base after base-asset commission); and durable recovery evidence (every step is an
intent with a result, ADR-0044 visibility rule). "Reduce this governed holding" is a core instruction; the
exchange `reduceOnly` wire flag is an adapter detail used only where `reduce_only_wire_flag = true`.

**Reconciliation is never optional (resolves v1 P1-3).** The reconciliation worker, startup recovery, the
live-risk filter, orphan-stop recognition and `/status` sync are rewritten against an `AccountObservation`
produced by the venue: `Positions(Vec<ExchangePosition>)` for position accounts, `Inventory(holdings,
governed, unmatched)` for balance accounts. Unmatched derivatives positions keep today's treatment (ADR-0022:
closed); unmatched spot inventory is **reported and blocks new legs** but is never sold. Quantity
discrepancies between governed inventory and Robson's position block new legs and surface in `/status`.
The existing gap (matching by symbol/side rather than order-level authorship) stays a follow-up.

**Capital source (resolves v1 P1-4).** `AccountPort` separates three quantities: `cash.available` (the
cap for new entries, ADR-0024 capital cap), **governed equity** = quote cash (free + locked) + Σ governed
inventory × `Mark` (position accounts: wallet balance + unrealized P&L, both reported separately by the
venue; the connector's current `wallet_balance` mapping, which omits unrealized P&L, is corrected in step 2),
and the **persisted monthly budget basis** (ADR-0024 §6: governed equity at month boundary minus carried
risk; ADR-0051: fixed for the month, non-expanding). Numeraire is the venue's quote asset; this ADR admits
USDT-quoted venues only (Needs Input for others). A `Mark` older than `ROBSON_VALUATION_MAX_AGE` (proposed
60 s) marks the basis invalid, exactly as an unreachable venue does today.

**Execution costs drive sizing (resolves v1 P1-5).** `ExecutionCostEnvelope` replaces the single
`RiskConfig` taker rate for every leg: admission, latent risk and the executable worst-loss per unit
(ADR-0050 §3) read the venue's envelope; a missing or stale envelope (`as_of` older than
`ROBSON_COSTS_MAX_AGE`, proposed 24 h) fails admission closed. `fee_asset_policy` states whether
commissions settle in quote, in base (reducing sellable inventory: the governed quantity is net of it) or in a
third asset (BNB): a third-asset fee is converted at the venue mark at fill time and recorded with
provenance. Settlement evidence is mandatory on every venue (`SettlementPort`); a venue without an income
endpoint derives it from fills. Rebates, if any, are recorded as negative commission.

### 3. Adapter relocation and construction by configuration

- `robsond/src/{binance_exchange,binance_ohlcv}.rs` and the futures WebSocket wiring in
  `market_data.rs` move to `robson-connectors/src/binance/usdm/`. `normalize_market_quantity` moves
  unchanged in the mechanical step (its behaviour change is step 5).
- `Daemon<E: Venue + 'static, S: Store + 'static>`; `Daemon::with_venue(config, venue, market_data,
  ohlcv)`. A `VenueFactory` in `robsond` is the only place that names a concrete adapter; it builds the venue,
  its market-data and OHLCV ports and its credential scope from `ROBSON_VENUE` (`binance_usdm` today,
  `binance_spot` next, `stub` for tests).
- `/readyz` keeps `binance_api` and adds `venue: { id, kind, health }` (additive); `health` there means
  execution and accounting eligibility (trading rules fresh, costs fresh, valuation fresh, account observation
  succeeded), not the public ping used today.

### 4. Domain cleanup with stable encodings (resolves v1 P1-6, P2-13)

- **Credential identity is not the venue.** `robson_domain::Exchange` keeps its serialized values
  (`"binance"`, `"binance_testnet"`) because they are the AES-GCM AAD, the store query key and the SQL CHECK.
  A venue is `(Exchange, MarketKind, profile)`; `binance:spot` reuses the `"binance"` credential. The
  `base_url()` method leaves the domain for `robson_connectors::binance::Endpoints`. A new exchange (B3)
  adds a credential value through an explicit migration with old-ciphertext compatibility tests.
- `as_binance_str` leaves `ports.rs`; wire encodings belong to the adapter.
- `binance_position_id` → `exchange_position_id` in Rust only. The event payload keeps the key via
  `#[serde(rename = "binance_position_id")]`, and so does the `Position` serialization (it derives serde
  independently). The SQL column, the projector payload and the API/frontend field keep their names; the API
  may add `exchange_position_id` additively later. Fixtures with null, non-null and missing field prove
  byte-equal round trips.
- Sizing vocabulary: the "margin cap" in `size_entry` is named the **capital cap**
  (`notional ≤ available_quote × (1 − headroom)`, leverage constant 1); numbers unchanged.
  `RiskConfig::leverage()` is validated only by venues with a `PositionAccountPort`.

### 5. Quantisation is a separate, behaviour-changing step (resolves v1 P1-7)

`normalize_market_quantity` hard-codes a `0.001` step and checks only `minQty`; `SymbolTradingRules`
aligns to the real step and validates min, max and alignment. They differ (for a `0.01` step, `0.019`
stays `0.019` under the old helper and becomes `0.01` under rules). Today ordinary market orders already
prefer metadata (`binance_exchange.rs:391`) while protective quantities (`:556`) and the admission
precheck (`position_manager.rs:2225`) use the old helper. Step 5 replaces the helper with
`robson_exec::quantize::market_quantity(rules, qty)` for entry, exit and protection, defines the behaviour
when metadata is unavailable (`ExecutableSpan` flows fail closed, as today), and reuses the domain stop
resolver's adverse tick rounding instead of introducing a second stop derivation. ADR-0052's persisted span
is never recomputed.

### 6. Long-only readiness

Arming `Side::Short` on a venue with `supports_short = false` is rejected at admission with a typed error
(`VenueUnsupported { side }`) before any order. No other core change.

### 7. Multi-venue execution model (constraints fixed here; implemented by a later ADR)

The operator arms **one logical operation** (`symbol`, `side`, selected venues, weights). The entry pipeline
fans out into one **leg per venue**; a leg is a full position with its own fills, its own protective stop on
its own venue, its own reconciliation and its own close. Prerequisites, all durable: `LogicalOperationId`,
`LegId`, `VenueId` and account scope on every position; venue-scoped exchange order ids, settlement ids and
checkpoints (today they are globally unique in the projection and income schemas); an explicit mapping of
every pre-existing position to `binance:usdm`.

- **Weights allocate planned risk, not quantity.** The operation's planned risk `R` is bounded by
  `min(1% × capital_base, remaining monthly budget)` (ADR-0051). Leg *i* receives `R × wᵢ` and derives its
  own quantity from its own venue price, its own executable stop (levels from that venue's OHLCV feed), its
  own cost envelope, its own cash cap and its own trading rules. A quantity is never transferred between
  venues.
- **One governed admission reserves `Σ leg planned risk` atomically** before anything is externally
  visible (ADR-0051 §1 proof). Reservations are held through unknown acknowledgements, partial fills and
  unsettled closes; a failed leg's reservation is released only after authoritative evidence that no
  liability remains. Failed weights are **not** redistributed and a recovered venue is **not** entered later
  without the operator's authorised retry policy (Needs Input).
- **Budget is global with local survival constraints.** The monthly budget stays a single −4% on the
  consolidated basis (Σ venue bases). In addition each leg's planned risk is capped at `1% × Bᵢ`, where
  `Bᵢ` is the venue's own persisted basis (its allocated bankroll, measured from the venue's balance, not
  from weights). This bounds the loss any single venue can absorb per operation; it does not by itself bound a
  venue's monthly loss at 4% (see the decision matrix and Needs Input).
- **Viability floor per venue** (operator requirement: no venue may be left with capital that cannot keep
  trading after ~3 bad months). With `Cₛ = Bᵢ × (1 − 0.04)^K` the stressed bankroll after `K` worst months
  at the local cap, a venue is viable for a symbol only if both hold:

  ```text
  0.01 × Cₛ                       ≥  q_min × worst_loss_per_unit(d_max, costs)
  Cₛ × (1 − headroom)              ≥  n_min
  ```

  where `q_min` is the venue's minimum executable quantity after lot rounding, `n_min` its minimum
  notional, `d_max` the maximum admissible executable distance (ADR-0050 §5 bound, a conservative input; a
  historical median cannot carry the guarantee) and `worst_loss_per_unit` the ADR-0050 §3 costing (distance +
  gap + fees per fill, from the venue envelope). Illustration with the repository defaults (gap 10 bps, taker
  5 bps per fill, `d_max` 10 %, cost ratio ≈ 0.1018), K = 3: Binance USDⓈ-M BTCUSDT (`n_min` 50 USDT,
  `q_min` 0.001 BTC at ~77,500 ≈ 77.5 USDT) → `Bᵢ ≥ 77.5 × 0.1018 / (0.01 × 0.8847) ≈ 892 USDT`; Binance
  spot BTCUSDT (`n_min` 5 USDT, `q_min` 0.00001 BTC ≈ 0.78 USDT) → `Bᵢ ≥ 5 / (0.99 × 0.8847) ≈ 5.7 USDT`
  from the notional bound. `K` is `ROBSON_VENUE_VIABILITY_MONTHS` (proposed 3). The floor is checked when an
  allocation is saved, at every month boundary and again at admission (prices and metadata move); a venue
  below the floor is flagged in `/status` and excluded from new legs, never from exits.
- **Costs are visible where the weights are edited**: the API returns each venue's envelope next to its
  weight.
- **Out of scope here**: cross-venue routing by liquidity, synthetic hedges (spot long vs B3 short),
  transfer automation, per-venue monthly budgets.

### 8. Sequencing and rollback boundaries (resolves v1 P1-12)

Six PRs, each Codex-reviewed before merge, during the empty-book window. Steps 1–4 are
behaviour-preserving; 5 changes behaviour on purpose; 6 is the first new venue.

| Step | Content | Rollback boundary |
|------|---------|-------------------|
| 1 | Mechanical relocation of the two adapters and the futures WS wiring into `robson-connectors`, existing `ExchangePort` unchanged | revert alone |
| 2 | New port contracts (§2) + USDⓈ-M adapter implementing them by forwarding to the existing calls; `ExchangePort` kept as a deprecated alias for one release; `wallet_balance` corrected to include unrealized P&L | revert with step 3 if 3 is in |
| 3 | Generic daemon construction (`VenueFactory`, `with_venue`), market-data injection, additive `/readyz.venue` | revert alone (steps 1–2 stand) |
| 4 | Rust naming and endpoint cleanup with stable event, `Position` and credential encodings; fixtures | revert alone |
| 5 | Quantisation alignment (§5), explicitly behaviour-changing, with before/after fixtures per order kind | revert alone |
| 6 | Spot venue acceptance: `InventoryPort`, `AtomicCancelReplace`, settlement from fills, conformance suite green; separate ADR for the long-only entry gate | feature-flagged by `ROBSON_VENUE` |

No schema migration in steps 1–5. Step 6 and the §7 identities ship with explicit migrations.

### 9. Acceptance criteria

- Conformance suite in `robson-testkit` run against `StubExchange`, the USDⓈ-M adapter and (step 6) the
  spot adapter: capability flags agree with accessors; protection is `MarketOnTrigger`; replacement protocol
  property-tested for the stop race (old stop never absent while the new one is unaccepted; on balance
  accounts the inventory is never unlocked without a successor or a close intent); admission race
  property-tested (reservation never exceeds `min(1%, remaining)`).
- Request budget: bounded calls per venue per symbol per minute (rules TTL, costs TTL, valuation TTL,
  single-flight); WebSocket subscriptions bounded by the active symbol set; no per-tick REST.
- Durable recovery evidence: every recovery decision emits an event (checked, healed, still bounded),
  ADR-0044 style; `/status` exposes per-venue eligibility.
- Existing security gates unchanged (gitleaks, osv-scanner, cargo-audit, SHA-pinned actions); allocation and
  arming mutations are frozen during a request (guardrails §4 on the frontend).
- All 68 DB-gated tests and replay fixtures green; old-ciphertext decryption test green after step 4.

---

## Failure modes (external dependencies and this service)

| Dependency / event | What happens | Degraded mode | Visibility |
|---|---|---|---|
| Database dies | no admission, no new legs; open legs keep exchange-resident protection | read-only `/status` from memory; daemon refuses arms | `/readyz` database=false, event on recovery |
| Venue REST dies | rules/costs/valuation go stale → basis invalid, admission fails closed; exits keep retrying with backoff | reads from cache until TTL; protection is on the venue | per-venue `health`, `/status` flag, alert |
| Venue WS silent | REST fallback for price (existing `FallbackSupport`); trailing advances continue on REST ticks | bounded REST polling | silent-feed metric, event |
| Daemon dies mid fan-out | legs already accepted are protected on their venues; unaccepted legs are intents without result | startup recovery reads the intent journal and each venue's observation; completes or releases per evidence | recovery events per leg |
| Daemon dies mid stop replacement | position account: old stop still live (place-then-cancel); balance account: atomic cancel-replace either succeeded or left the old stop | recovery heals from `open_protective_orders` | recovery event |
| Unknown order outcome (timeout) | reservation held; order looked up by client id before any retry | no blind retry | event + `/status` pending count |
| Stale fees / rules / valuation | admission fails closed; exits unaffected | — | `/status` per-venue staleness |
| Incomplete settlement | income/commission missing → month accounting flagged invalid, new legs blocked (ADR-0051) | — | `unmatched_income_count`, event |
| Unmatched spot inventory | reported, never sold; new legs blocked on that venue | — | `/status` per-venue inventory discrepancy |

## Consequences

- Positive: a second venue is an adapter plus a factory arm; the risk ADRs are untouched; spot long-only
  becomes a small ADR; the regulatory single point of failure that materialised on 2026-11-04 is structurally
  addressed; failure modes are written down per venue.
- Negative / trade-offs: six refactor PRs with no operator-visible feature before step 6; optional ports add
  branching in the daemon; the consolidated basis needs every venue reachable and valued at month start;
  spot inventory accounting is new surface area (governed vs operator-owned holdings).
- Left on the table: cross-venue netting and routing, per-venue monthly budgets, transfer automation, order-level
  authorship verification in reconciliation.

## Alternatives and decision matrix

| Option | Second venue cost | Safety of spot | Risk-ADR impact | Chosen |
|---|---|---|---|---|
| A. Keep `ExchangePort`, stub futures methods in spot adapters | low | **unsafe** (reconciliation and replacement assume a position account) | none on paper, broken in practice | no |
| B. Capability-split ports + required observation/settlement (this ADR) | medium | safe by contract + conformance | none | **yes** |
| C. One crate per adapter now | medium+ | same as B | none | later, when a second exchange lands |
| D. Rename the persisted field | — | — | breaks ADR-0049 replay | no |

Budget model for multi-venue:

| Option | −4% consolidated | Local depletion bounded | Budget fungibility | Chosen |
|---|---|---|---|---|
| Global only | yes | **no** (one venue can absorb the whole 4%) | full | no |
| Per-venue 4% × Bᵢ (Σ = 4% × ΣBᵢ for any weights) | yes | yes | none (stranded budget) | no |
| Global + local per-operation cap (1% × Bᵢ) + viability floor | yes | per operation yes; per month only via K-stress floor | high | **yes**, pending Needs Input |

(v1's claim that per-venue budgets preserve −4% only with equal weights was wrong: `Σ 0.04·Bᵢ = 0.04·ΣBᵢ`.)

---

## Needs Input (operator)

- Weights as **risk** allocation (chosen above) or as **capital** allocation; whether to add a per-venue
  monthly loss cap (e.g. 8% × Bᵢ) on top of the global 4%.
- `K` (proposed 3) and `ROBSON_VENUE_VIABILITY_MONTHS` semantics (calendar months of the budget model).
- Failed-leg policy: never redistribute (proposed) vs operator-approved redistribution; retry authority for a
  venue that recovers mid-operation.
- Numeraire beyond USDT-quoted venues (USDC venues, BRL on B3): conversion source and staleness rule.
- Cost envelope source: venue API (Binance exposes commission rates) with a 24 h staleness rule, or
  configuration with an `as_of` date.
- Account ownership on spot: dedicated sub-account per venue (recommended: no operator-owned inventory on
  the execution account) or shared account with governed-inventory tracking.
- `/status`: consolidated only, or per-venue section in the PR that introduces legs.

## Implementation Notes

- Step 1 files: `robsond/src/{binance_exchange,binance_ohlcv,market_data,daemon,main}.rs`,
  `robson-connectors/src/binance/usdm/*`, `robson-connectors/src/lib.rs`.
- Step 2 files: `robson-exec/src/{ports,executor,stub}.rs`, `robson-connectors/src/binance/usdm/exchange.rs`
  (`wallet_balance` + unrealized P&L), `robson-testkit` (conformance suite skeleton).
- Step 3 files: `robsond/src/{daemon,main,config,api,market_data}.rs` (`VenueFactory`, `/readyz.venue`).
- Step 4 files: `robson-domain/src/{credentials,entities,events,lib}.rs`, `robson-engine/src/lib.rs`,
  `robson-store/src/memory.rs`, `robson-exec/src/ports.rs`; fixtures for events, `Position` and credentials.
- Step 5 files: `robson-exec/src/quantize.rs`, `robsond/src/position_manager.rs`, usdm adapter; before/after
  fixtures for entry, exit and protective quantities.
- Review: Codex gpt-6.1-sol xhigh on this ADR (v1: REQUEST_CHANGES, 15 findings; v2 addresses P0 and P1 1–12,
  P2 13 and 15, P1 14 via §9 and the tables above), then one Codex review per PR.
