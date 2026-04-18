# Symbol-Agnostic Policies

**Status**: Active
**Effective Date**: 2026-04-18
**Owner**: Architecture / Risk Engineering
**Version**: 1.0
**Companion ADR**: [ADR-0023 — Symbol-Agnostic Policy Invariant](../adr/ADR-0023-symbol-agnostic-policy-invariant.md)

---

## Golden Rule

**Every Robson policy applies to every trading pair the system operates on. A policy
that is correct for `BTC/USDT` must be correct for `ETH/USDT`, `SOL/USDC`,
`BTC/USDC`, or any other symbol — unchanged in meaning.**

Policies, guardrails, and invariants are **never** allowed to reference a specific
symbol in their statement. Where a specific symbol appears, it must be as a
**configuration value** or as an **illustrative example**, never as a hard-coded
assumption in the rule itself.

---

## The Invariant (I3 — Symbol Neutrality)

For every policy document, architecture spec, ADR, runbook, guide, prompt, or test
fixture in this repository:

1. **The statement of the policy MUST be symbol-agnostic.** If you can write the rule
   with `{symbol}` as a variable and it still reads correctly, it is compliant.
2. **Symbol-specific constants (tick size, min notional, lot step) MUST be looked up
   at runtime from exchange metadata**, not hard-coded in the policy text.
3. **Examples MAY use a concrete symbol** (typically `BTC/USDT`) for clarity, but the
   example must be labeled as illustrative and the surrounding policy text must not
   narrow its scope to that symbol.
4. **Configuration values MAY whitelist specific symbols** the operator chooses to
   trade — but this is operator-configurable scope, not a policy exception. The
   policy itself still applies to every symbol; the configuration merely decides
   which symbols are currently in use.

---

## Why This Matters

Robson began its v1 life focused almost entirely on `BTCUSDT` and `BTCUSDC`. Several
documents, code paths, and runbooks absorbed that history as if it were a rule.

This creates two failure modes:

1. **Silent misapplication.** When the operator arms a position on `ETHUSDT`, the
   reader of a BTC-anchored spec cannot tell whether the rule still applies, applies
   with modification, or does not apply. Ambiguity is a governance hole.
2. **Scope creep friction.** Adding a new symbol should be a configuration change, not
   a documentation refactor. Every BTC-specific assumption is a rewrite cost later.

The Risk Engine's core rules (1% per trade, 4% monthly drawdown, Golden Rule position
sizing, Hand-Span Trailing Stop, Technical Stop Distance) are **properties of the
Robson risk model**, not properties of Bitcoin. They must read that way.

---

## What Is A Symbol-Specific Constant?

Symbol-specific constants belong to the **exchange**, not to the policy:

| Constant | Source | Where it lives |
|---|---|---|
| Tick size (price precision) | Binance `exchangeInfo.symbols[].filters[PRICE_FILTER]` | Loaded at runtime |
| Lot step (quantity precision) | Binance `exchangeInfo.symbols[].filters[LOT_SIZE]` | Loaded at runtime |
| Minimum notional | Binance `exchangeInfo.symbols[].filters[MIN_NOTIONAL]` | Loaded at runtime |
| Max leverage (futures) | Binance `leverageBracket` | Loaded at runtime |
| Trading fee rate | Binance account commission rate | Loaded at runtime |

Policies reference these by role (e.g., "rounded to the exchange's tick size"), never
by numeric value.

---

## Where This Invariant Applies

### Architecture & ADRs

- Every architecture document must state its rules in symbol-agnostic form.
- Diagrams and flows should either use `{symbol}` as a placeholder or note
  "example shown with `BTC/USDT`; the same flow applies to every supported symbol".
- ADRs that encode a decision about a trading rule must not scope the decision to a
  single symbol without explicit justification and a stated expiry.

### Risk Engine

The rules below are symbol-agnostic by construction and must remain so:

| Rule | Statement | Notes |
|---|---|---|
| 1% risk per trade | `risk_amount = capital × 0.01` | Capital is in quote-asset-neutral units (USD equivalent) |
| 4% monthly drawdown | `MonthlyPnL ≤ −(capital × 0.04)` | Aggregated across all symbols |
| Position size (Golden Rule) | `size = risk_amount / abs(entry − tech_stop)` | Works for any symbol |
| Span (palmo) | `span = abs(entry_price − technical_stop)` | Symbol-neutral; stop is chart-derived |
| Hand-Span Trailing Stop | Stop advances in integer multiples of `span` | Monotonic, per-position |
| Technical Stop Distance | Second support (LONG) / resistance (SHORT) on 15m chart | Chart-derived, not percentage |
| Max open positions | N (configurable) | Counted across all symbols |
| Max total exposure | 30% of capital | Summed across all symbols, in quote-asset-neutral units |
| Max single position | 15% of capital | Applies to any symbol |
| No duplicate position | Same `(symbol, side)` | Generalized: the duplicate check uses `symbol` as a variable |

### Detector / Opportunity Detection

The detector is a per-symbol process (one detector per configured symbol), but the
**detection logic itself** is symbol-agnostic:

- Moving-average crossover logic does not encode any `BTC`-specific behavior.
- Technical Stop Analyzer uses OHLCV for the target symbol; its algorithm is the
  same for every symbol.
- Prompt templates for LLM-assisted analysis (v3+) must accept `{symbol}` and not
  hard-code `BTC`.

### Runbooks

VAL-001 and VAL-002 may use a concrete symbol for the validation run (currently
`BTCUSDT` on testnet), but:

- The runbook procedure must apply verbatim to any symbol the operator chooses.
- Where the runbook uses `BTCUSDT` in a command, it should note "replace with the
  symbol under validation".
- A dedicated cross-symbol validation pass is follow-up required (run VAL-001 for at
  least one non-BTC symbol before promoting new exchange pairs to production).

### Tests & Fixtures

- Unit tests may use `BTCUSDT` as a default fixture value for readability.
- Integration and property tests for risk-gate logic MUST parameterize over at least
  two distinct symbols (different base asset, different quote asset) to prove
  symbol-agnosticism.
- A fixture named after a specific symbol (e.g., `btc_long_fixture()`) should be
  accompanied by an equivalent fixture on a different symbol in any test suite that
  claims to cover the Risk Engine end-to-end.

### Strategies

Pre-built strategies (`All In`, `Rescue Forces`, `Smooth Sailing`, `Bounce Back`)
define their **entry logic**, not their **symbol scope**. Each strategy can be
deployed on any symbol the operator configures. Strategy documentation must not
imply BTC-only scope; a strategy config lists `symbols: [...]` as operator input.

---

## Migration Guidance

Existing documents may reference `BTCUSDT` / `BTCUSDC` in ways that predate this
policy. When editing such a document:

1. **Rule statement** that reads "Robson trades BTC/USDT on 15-minute timeframe" →
   rewrite as "Robson trades the configured symbol(s) on the configured timeframe.
   Example: `BTCUSDT` on 15m."
2. **Code sample** with hard-coded `"BTCUSDT"` → replace with `"{symbol}"` placeholder
   and note that the symbol comes from configuration.
3. **Command example** like `curl ... -d '{"symbol": "BTCUSDT", ...}'` → keep as-is
   if clearly illustrative, but add the note "replace `BTCUSDT` with the symbol you
   are operating on".
4. **Diagrams** mentioning BTC in the general flow → replace with `{symbol}` or add
   a "(example shown with BTC/USDT)" caption.
5. **Test fixtures** that only cover BTC → add at least one parameterization with a
   non-BTC symbol to lock in symbol-agnosticism.

Do not do a bulk `sed`-style rewrite. Changes must preserve the meaning of each
document and respect the "current reality vs target architecture" separation already
mandated by `AGENTS.md`.

---

## Prohibited Practices

### ❌ Encoding A Symbol In A Policy Statement

```
Rule: Robson risks 1% of capital per BTC trade.  # WRONG — narrows to BTC
Rule: Robson risks 1% of capital per trade.       # CORRECT — symbol-neutral
```

### ❌ Hard-Coding Symbol-Specific Constants In Policy Text

```
"Stops must be rounded to $0.01 tick size."  # WRONG — that's BTCUSDT's tick
"Stops must be rounded to the exchange's tick size for the symbol." # CORRECT
```

### ❌ Referring To "The BTC Position" In A Generic Runbook

```
"Close the BTC position."                   # WRONG
"Close the position (symbol = {symbol})."   # CORRECT
```

### ❌ Per-Symbol Forks Of A Policy

A separate "BTC risk policy" and "ETH risk policy" are forbidden. One policy; one
code path; symbol is a parameter.

---

## Approved Practices

### ✅ Symbol In Examples, Not In Rules

```
Rule: `position_size = (capital × 0.01) / abs(entry − stop)`.
Example (BTCUSDT, capital = $100, entry = 95,000, stop = 93,500):
  position_size = (100 × 0.01) / 1,500 = 0.000666 BTC.
```

### ✅ Operator-Configurable Symbol Whitelist

The operator configures which symbols `robsond` actively trades. This is scope
selection, not a policy exception. The policy still applies to every symbol in the
whitelist.

### ✅ Per-Symbol Exchange Metadata Cached At Runtime

`ExchangePort` loads and caches tick size, lot step, min notional, and max leverage
for each symbol on startup and refreshes on `exchangeInfo` change. Policies
reference these via the cache, never by literal value.

---

## Enforcement Mechanisms

1. **Doc review checklist**: PRs touching `docs/policies/`, `docs/adr/`,
   `docs/architecture/`, and `docs/runbooks/` must be reviewed against this
   invariant. A rule that hard-codes a symbol is a blocker.
2. **Test parameterization**: risk-gate and sizing tests must cover at least two
   symbols with different base/quote assets (follow-up required where missing).
3. **Reconciliation worker symbol-agnosticism**: the worker described in
   [UNTRACKED-POSITION-RECONCILIATION.md](UNTRACKED-POSITION-RECONCILIATION.md)
   scans every symbol on the account, not a whitelist.

---

## Related Documentation

- **[ADR-0023 — Symbol-Agnostic Policy Invariant](../adr/ADR-0023-symbol-agnostic-policy-invariant.md)**
- **[UNTRACKED-POSITION-RECONCILIATION.md](UNTRACKED-POSITION-RECONCILIATION.md)**
- [v3-risk-engine-spec.md](../architecture/v3-risk-engine-spec.md)
- [v3-runtime-spec.md](../architecture/v3-runtime-spec.md)
- [v3-control-loop.md](../architecture/v3-control-loop.md)
- [STRATEGIES.md](../STRATEGIES.md)
- [PRODUCTION_TRADING.md](../PRODUCTION_TRADING.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](../adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)

---

## Changelog

| Version | Date | Changes | Author |
|---------|------|---------|--------|
| 1.0 | 2026-04-18 | Initial policy creation | Engineering Team |
