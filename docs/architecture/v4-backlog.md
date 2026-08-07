# ROBSON v4 — Backlog

**Status**: Living document — items accumulate as v3 scope is finalized.
**Owner**: Operator (Leandro Damasio)
**Last updated**: 2026-08-06

Items in this backlog were explicitly deferred from v3 scope. They are not forgotten —
they are intentionally parked until v3 is declared complete.

---

## Deferred Items

| ID | Description | Origin | Rationale |
|----|-------------|--------|-----------|
| MIG-v4#1 | Hash-chained EventLog | was MIG-v3#6 | Good integrity property; append-only PostgreSQL is sufficient for v3 single-operator. Prioritize when audit requirements grow or multi-operator is introduced. |
| MIG-v4#2 | PaymentRail trait (TRON/TRC-20) | was MIG-v3#7 | Architecture is TRON-ready (see ADR table #12 in v3-migration-plan.md). Activate when FINMA stablecoin guidance is clear and Zero Hash obtains Swiss-compatible license. |
| MIG-v4#3 | Startup `auto_reconcile` validation and policy-drift corrections | was TD-2026-05-05-001 Slice 5B2C | The policy is implemented and production configuration was read-only verified as enabled on 2026-08-06. Complete the deferred drill, restore exit 78 when evidence is absent, and enforce or safely redesign the no-partial-close batch guarantee. |
| MIG-v4#4 | Multi-user / multi-tenant support | — | v3 is single-operator by design (ADR-0007). Introduce when a second operator joins or a client-facing product is scoped. |
| MIG-v4#5 | LLM / Context Governance (QE-P5) | was QE-P5 | No LLM in v3. When LLM reasoning is added, it enters via `ReasoningPort` trait (already defined in v3-runtime-spec.md). The Runtime governs context and output. |
| MIG-v4#6 | Backtesting / Simulation (`robson-sim`) | — | Deferred from v3 launch. Trigger: operator wants to validate strategy changes before arming. |
| MIG-v4#7 | Estimated-evidence auto-close path | — | `Estimated` is hard-blocked in v3 `reconcile_close`. A future operator-confirmed estimated close (with PnL bounds) could be designed for v4, guarded by explicit human approval flow. |
| MIG-v4#8 | Additional operator controls | was MIG-v3#5 | The dashboard already covers supported ARM, approval, monthly-halt, and panic actions. Add controls only for an approved policy need; do not restore the removed general-purpose CLI or obsolete pause, resume, and risk-override concepts. |

---

## Constraints Inherited by All v4 Work

1. **Single Runtime authority** — all exchange actions pass through `robsond` GovernedAction. No direct exchange access from any other component.
2. **Policy-first** — ADR-0022, ADR-0023, ADR-0024 invariants apply to v4 as well. No feature may introduce static exposure caps, hard-coded symbols, or non-audited closes.
3. **Estimated evidence remains operator-confirmed** — any v4 auto-close path that uses estimated PnL MUST require explicit operator confirmation before persisting the terminal event.
4. **English only** — all code, comments, and documentation.

---

## v3 Finalization Criteria (for reference)

v3 is complete when all of the following are repository-verified:

- [x] TD-2026-05-05-001 closed (`docs/technical-debt.md` Status: Closed)
- [x] MIG-v3#8 Chaos testing suite implemented with repository tests passing
- [x] MIG-v3#14 Risk Dashboard shipped in `apps/frontend/`
- [x] MIG-v3#15 ARM Entry Policy Selection shipped in `apps/frontend/` (mode + approval controls in `ArmModal`)
- [x] `v3-migration-plan.md` has no items marked `⏳ Pending`
- [ ] Production running ≥ 30 days with 0 `position_untracked_detected` events (Grafana/Loki)
