# Robson Documentation Index

**Status**: Repository-verified navigation on 2026-08-06

This index points to current repository documentation. Historical v1 and early v2 plans remain available for product archaeology, but they are not implementation or operational instructions.

## Start here

| Need | Document |
|---|---|
| Product and runtime overview | [Project README](../README.md) |
| Contributor setup | [Developer Quickstart](onboarding/DEVELOPER-QUICKSTART.md) |
| Repository rules | [AGENTS.md](../AGENTS.md) |
| Operator CLI boundaries | [Robson Operator CLI](CLI.md) |
| Current architecture map | [Architecture Index](architecture/README.md) |
| Architecture decisions | [ADR Index](ADRs.md) |
| Operational procedures | [Runbook Index](runbooks/README.md) |
| Known debt and deferred work | [Technical Debt](technical-debt.md) |

## Architecture

- [v3 Migration Plan](architecture/v3-migration-plan.md)
- [v3 Runtime Specification](architecture/v3-runtime-spec.md)
- [v3 Control Loop](architecture/v3-control-loop.md)
- [v3 Query Engine](architecture/v3-query-query-engine.md)
- [v3 Architectural Decisions](architecture/v3-architectural-decisions.md)
- [v4 Backlog](architecture/v4-backlog.md)

Repository evidence and deployed state are different claims. Architecture documents use `repository-verified` when only source evidence is available. Verify live state separately before operational action.

## Data contracts

- [bronze-v1 cold retention export](data-contracts/bronze-v1.md)

## Policies and critical ADRs

- [Robson-Authored Position Invariant](adr/ADR-0022-robson-authored-position-invariant.md)
- [Trading Policy Layer](adr/ADR-0024-trading-policy-layer.md)
- [Exchange-Side Insurance Stop](adr/ADR-0039-exchange-side-insurance-stop.md)
- [Maker-First Entry Execution](adr/ADR-0040-maker-first-entry-execution.md)
- [Income Ledger Reconciliation](adr/ADR-0045-income-ledger-reconciliation.md)
- [Executable Span Single Stop Policy](adr/ADR-0052-executable-span-single-stop-policy.md)
- [Untracked Position Reconciliation Policy](policies/UNTRACKED-POSITION-RECONCILIATION.md)
- [Production Deployment Policy](policies/PRODUCTION-DEPLOYMENT.md)

## Operations

- [Stale-Active Recovery](runbooks/td-2026-05-05-001-stale-active-recovery.md)
- [Market Data Degraded Mode](runbooks/market-data-degraded-mode.md)
- [robsond Database Migrations](runbooks/robsond-db-migrations.md)
- [Real Capital Activation](runbooks/val-002-real-capital-activation.md)

Use the SvelteKit dashboard or authenticated `robsond` API for routine operator actions. The Rust `robson-cli` binary is limited to exceptional recovery workflows documented in [CLI.md](CLI.md).

## Historical documents

The following root-level documents are superseded planning artifacts and are retained only for archaeology:

- [Robson v2 Architecture](ARCHITECTURE.md)
- [Robson v2 Execution Plan](EXECUTION-PLAN.md)
- [Robson v2 Prompt Pack](PROMPT-PACK.md)
- [Robson v2 Smoke Test](SMOKE-TEST.md)
- [Robson v2 Reliability Architecture](RELIABILITY.md)
- [Legacy Developer Guide](DEVELOPER.md)
- [Agentic Trading Concept Note](AGENTIC-TRADING.md)

Do not run commands from a historical document unless a current runbook explicitly repeats and validates them.
