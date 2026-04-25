# ADR-0023 — Symbol-Agnostic Policy Invariant

**Date**: 2026-04-18
**Status**: DECIDED — FOLLOW-UP REQUIRED (documentation sweep + test parameterization)
**Deciders**: RBX Systems (operator + architecture)

---

## Context

Robson's development history has centered on `BTCUSDT` and `BTCUSDC`. Over time, this
focus has leaked into documents, runbooks, tests, and prompts in ways that read as
policy:

- `v3-runtime-spec.md` reserves an `allowed_symbols` set in `RuntimeConfig`, but the
  example logs show `"symbol": "BTCUSDT"` as if it were the canonical value.
- `OPERATION-LIFECYCLE.md` and parts of `TRANSACTION-HIERARCHY.md` use `BTCUSDC` in
  example flows without clearly marking them as examples.
- `PRODUCTION_TRADING.md` documents `POST /api/trade/buy-btc/` as the "first
  production trade" endpoint — a symbol-specific API path in a trading system that
  is meant to be symbol-agnostic.
- `STRATEGIES.md` describes the `Rescue Forces` strategy with a detector-side
  example that names only `BTCUSDT` and `ETHUSDT`.
- VAL-001 uses `BTCUSDT` in hard-coded `curl` bodies without noting that the symbol
  is operator-selectable.

None of these documents say "this policy is BTC-specific". They simply fail to say
"this policy applies to every symbol". The ambiguity is itself the problem.

Meanwhile, the **Risk Engine's core rules** (1% per trade, 4% monthly drawdown,
Golden Rule position sizing, Hand-Span Trailing Stop, Technical Stop Distance via
second support/resistance) are symbol-neutral by construction. They reference only
`capital`, `entry_price`, `technical_stop`, and `span` — concepts that exist for any
instrument on any exchange. Hard-coding them to `BTC/USDT` in documentation is not
a constraint of the rules; it is a narrative artifact.

As the operator plans to operate `ETHUSDT`, `SOLUSDC`, and other pairs, every
BTC-anchored assumption becomes a silent source of risk. A reader cannot tell
whether the rule was intended to be symbol-specific or merely illustrated with BTC.

---

## Decision

Establish a non-negotiable invariant on every policy, spec, ADR, runbook, guide,
prompt, and test in this repository:

> **Policies apply to every symbol the system operates on. A policy statement that
> narrows to a specific symbol (e.g., "Robson trades BTC/USDT") is non-compliant.
> Symbol-specific constants MUST come from exchange metadata at runtime, not from
> policy text.**

The invariant has three components:

### I3.a — Rules Are Stated Symbol-Agnostically

A rule must read correctly when the symbol is replaced by `{symbol}`. Concrete
symbols appear only as labeled examples or as operator-configured values, never as
silent assumptions in the rule statement.

### I3.b — Symbol Constants Come From The Exchange, Not The Policy

Tick size, lot step, minimum notional, max leverage, and fee rate are **exchange
metadata** loaded at runtime. Policies reference these by role ("rounded to the
exchange's tick size"), never by numeric value.

### I3.c — Configuration Selects Scope; Policy Defines Rule

The operator's `allowed_symbols` whitelist is a **scope selector**, not a policy
exception. The policy applies to every symbol; the configuration chooses which
symbols are currently active. Adding a new symbol is a configuration change, not a
policy rewrite.

### Rejected Alternatives

- **Per-symbol policy forks.** Rejected — forces the Risk Engine, the detector, and
  every downstream consumer to branch on `symbol`. This is both a code anti-pattern
  and a governance hole (which branch governs which trade?).
- **Treat BTC-specific documentation as acceptable because it's "just examples".**
  Rejected — ambiguity between "example" and "rule" has already caused real
  confusion in migration discussions. The invariant removes the ambiguity.
- **Scope the invariant to the Risk Engine only.** Rejected — the detector, the
  trailing-stop monitor, the reconciliation worker (see
  [ADR-0022](ADR-0022-robson-authored-position-invariant.md)), the prompts, and the
  runbooks all participate in governance. A symbol-agnostic Risk Engine with a
  BTC-locked runbook is not symbol-agnostic in practice.

---

## Consequences

### Positive

- Adding a new symbol becomes a configuration change (plus exchange metadata
  refresh), not a documentation refactor.
- Readers of any spec can parameterize the rules for any symbol without second-
  guessing intent.
- The Risk Engine's correctness claim generalizes: "correct for any symbol" is
  provable rather than implied.
- Test parameterization across symbols catches accidental symbol-coupling early.
- The reconciliation worker from
  [ADR-0022](ADR-0022-robson-authored-position-invariant.md) has a well-defined
  scope: "all symbols on the account", with no ambiguity.

### Negative / Trade-offs

- A backlog of documentation edits: many existing documents contain BTC-anchored
  rules or examples that need relabeling. This work is scoped as a sweep (follow-up
  required).
- Some endpoints already have symbol-specific names (`/api/trade/buy-btc/`,
  `buy-btc` command). These become legacy adapters; the symbol-agnostic endpoint is
  the forward path.
- Tests that only cover BTC do not prove symbol-agnosticism; expanding coverage
  requires test work.

### Operational

- PR review for any doc edit in `docs/policies/`, `docs/adr/`, `docs/architecture/`,
  or `docs/runbooks/` includes a symbol-agnosticism check: does the edit hard-code
  a symbol in a rule? If yes, the edit is blocked until reworded.
- Risk-gate and sizing tests must be parameterized over at least two symbols with
  different base/quote assets (e.g., `BTCUSDT` + `SOLUSDC`).
- VAL-001 must be executed against at least one non-BTC symbol before any new pair
  is promoted to production trading.

---

## Implementation Notes

Follow-up work required:

1. **Documentation sweep** (tracked separately): identify every rule statement that
   hard-codes a symbol and rewrite it symbol-agnostically with labeled examples.
   Prioritize `docs/policies/`, `docs/architecture/`, `docs/runbooks/`.
2. **Legacy endpoint deprecation**: mark `/api/trade/buy-btc/` and
   `/api/trade/sell-btc/` as legacy, exposing the generic equivalents as the
   primary path.
3. **Test parameterization**: expand risk-gate and sizing unit/property tests to
   parameterize over multiple symbols.
4. **Runbook cross-symbol validation**: VAL-001 re-run scheduled for a non-BTC
   symbol once Phase 2 passes on BTC.
5. **Prompt templates**: LLM-assisted analysis prompts (v3+) must accept
   `{symbol}` as a variable, not bake `BTC` into the template.
6. **Exchange metadata cache**: `ExchangePort` caches tick size, lot step, min
   notional, and max leverage per symbol on startup and refreshes on `exchangeInfo`
   change. Policies reference this cache.

### Invariants (non-negotiable)

1. A policy statement that names a specific symbol as a hard constraint is a
   violation of this ADR.
2. A symbol-specific numeric constant (tick size, lot step, etc.) in policy text is
   a violation of this ADR.
3. The Risk Engine rules (1%, 4%, Golden Rule, Hand-Span, Technical Stop Distance)
   MUST remain symbol-agnostic in both statement and implementation.
4. The reconciliation worker from
   [ADR-0022](ADR-0022-robson-authored-position-invariant.md) MUST scan every
   symbol on the account, not a whitelist.

### Related Components

- `v3/robson-domain/src/value_objects.rs` — `RiskConfig`, `TechnicalStopDistance`
- `v3/robson-engine/src/risk.rs` — risk-gate evaluation (already parameterized by
  symbol)
- `v3/robson-exec/src/executor.rs` — exchange metadata cache (target architecture)
- `docs/STRATEGIES.md` — strategy configs accept `symbols: [...]` from operator

---

## References

- [docs/policies/SYMBOL-AGNOSTIC-POLICIES.md](../policies/SYMBOL-AGNOSTIC-POLICIES.md) — full policy text
- [docs/architecture/v3-risk-engine-spec.md](../architecture/v3-risk-engine-spec.md)
- [docs/architecture/v3-runtime-spec.md](../architecture/v3-runtime-spec.md)
- [docs/architecture/v3-control-loop.md](../architecture/v3-control-loop.md)
- [docs/STRATEGIES.md](../STRATEGIES.md)
- [docs/PRODUCTION_TRADING.md](../PRODUCTION_TRADING.md)
- [ADR-0021 — Opportunity Detection vs Technical Stop Analysis](ADR-0021-opportunity-detection-vs-technical-stop-analysis.md)
- [ADR-0022 — Robson-Authored Position Invariant](ADR-0022-robson-authored-position-invariant.md) (companion)
