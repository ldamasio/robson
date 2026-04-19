# MIG-v3#11 Policy Layer — Session Closeout

**Date**: 2026-04-19
**Status**: Repository implementation complete; testnet rollout pending
**Scope**: ADR-0024 policy layer, dynamic slot calculation, realized-loss semantics,
and documentation alignment

---

## Summary

MIG-v3#11 is complete in the repository. Robson no longer enforces the legacy
v2 static soft limits (`max_open_positions`, `max_total_exposure_pct`,
`max_single_position_pct`) in `RiskGate`. Entry approval now follows ADR-0024:

- risk per trade: 1% of capital base
- monthly budget: 4% of capital base
- slot budget: realized losses plus latent risk from open positions
- no wins offset realized loss budget
- no duplicate open position for the same symbol+side

VAL-001 Phase 2 is unblocked in repository state, but it has not yet passed
operationally. The next operator-visible work is deploy/sync/testnet execution.

---

## Commits

### robson

| SHA | Message | Notes |
|-----|---------|-------|
| `2db23ad2` | `feat(risk): add trading policy dynamic slots` | Adds `TradingPolicy`, `TechStopConfig`, dynamic slots, and policy wiring. |
| `6283844d` | `docs(spec): mark MIG-v3#11 policy layer and dynamic slots done` | Marks the migration item complete. |
| `0b3653a7` | `fix(risk): track realized losses for dynamic slots` | Corrects realized-loss semantics: losses consume budget, wins do not offset them. |
| `19130cf3` | `docs(spec): update MIG-v3#11 verification sha` | Updates architecture references to the corrective commit. |

### rbx-infra

| SHA | Message | Notes |
|-----|---------|-------|
| `c3b1bc3` | `chore(testnet): configure robson technical stop policy` | Adds `ROBSON_MIN_TECH_STOP_PCT: "1.0"` to testnet config. |

---

## Validation

Run from `/home/psyctl/apps/robson/v2` after the corrective commit:

| Command | Result |
|---------|--------|
| `cargo fmt --all --check` | Pass |
| `cargo build --all` | Pass |
| `cargo test --all` | Pass: 409 passed, 24 ignored |
| `cargo clippy --all-targets -- -D warnings` | Blocked by pre-existing baseline issues outside MIG-v3#11 |

The clippy blocker is not introduced by MIG-v3#11. Current failures are missing-docs
and clippy configuration baseline issues in existing support modules. Treat this as
a repository hygiene item before requiring clippy as a hard close gate again.

---

## Documentation Updated

- `docs/architecture/v3-migration-plan.md`: MIG-v3#11 complete, VAL-001 Phase 2
  redeploy-and-run status, updated risk terminology.
- `docs/runbooks/val-001-testnet-e2e-validation.md`: Phase 2 no longer blocked by
  static exposure limits; dynamic-slot sizing notes added.
- `docs/audits/AUDIT-2026-04-18-testnet-readiness.md`: post-audit addendum records
  that I4/R1 are superseded by ADR-0024.
- `docs/adr/ADR-0021-opportunity-detection-vs-technical-stop-analysis.md`: Phase 2
  exposure blocker noted as resolved outside ADR-0021.
- `docs/adr/ADR-0024-trading-policy-layer.md`: implementation status and validation
  state added.
- `docs/architecture/v3-runtime-spec.md` and `docs/architecture/v3-control-loop.md`:
  runtime/risk terminology updated from `RiskLimits` to `TradingPolicy` and
  `RiskContext`.
- `v2/docs/architecture/RISK-ENGINE-PLAN.md` and `v2/docs/CLI.md`: legacy risk-limit
  references marked as superseded.

---

## Next Session Start Point

1. Deploy latest `robsond` image containing `0b3653a7` or newer.
2. Sync `rbx-infra` testnet config at `c3b1bc3` or newer.
3. Execute `docs/runbooks/val-001-testnet-e2e-validation.md` Phase 2.
4. Record exchange order, fill, trailing-stop, exit, and PnL evidence in the runbook.
5. Do not start VAL-002 until VAL-001 passes and MIG-v3#12 monthly state persistence
   is complete.

Recommended engineering order after rollout:

1. Add `exchange_order_id` to order domain events.
2. Fix entry event ordering so `EntryOrderPlaced` follows exchange acknowledgement.
3. Add `StartupReconciling` before accepting new decisions on daemon boot.
4. Implement MIG-v3#12 monthly state persistence.
