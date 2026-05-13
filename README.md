# Robson — Execution & Risk Engine for Leveraged Markets

[![Backend Tests](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml/badge.svg)](https://github.com/ldamasio/robson/actions/workflows/backend-tests.yml)

Robson is an execution and risk management engine for leveraged cryptocurrency markets. It is not an autonomous trading bot. It does not decide what to trade, predict prices, or scan for opportunities.

Robson is concerned with what happens **after** a trading decision is made: position sizing from chart-derived stops, governed order execution, lifecycle management through entry to settlement, and safe failure handling under volatile conditions.

The system provides a single-operator runtime with a slot-based monthly risk model, deterministic execution semantics, and full auditability at every state transition.

## Risk Model at a Glance

- **Monthly budget**: 4% of `capital_base` — hard limit enforced by a circuit breaker
- **Per-trade risk**: 1% of `capital_base` (derived from position size, never set directly)
- **Position sizing**: `size = (capital_base × 1%) / technical_stop_distance`
- **Technical stop**: always from chart analysis (second S/R level, 15m timeframe) — never a percentage of entry price
- **Slot capacity**: 4 concurrent positions maximum

`capital_base` is set once at month start as a pessimistic snapshot: `wallet_balance − carried_risk(Entering + Active + Armed)`. It is immutable for the duration of the month.

## Position Lifecycle

```
Armed → Entering → Active → Exiting → Closed
  └─ Cancelled (disarmed before entry)
  └─ Error (unrecoverable, requires operator action)
```

The operator **arms** a position by specifying a symbol, direction (Long/Short), and an entry mode. Robson's detector then monitors the market and fires an entry signal when the chosen condition is met. The signal is routed through the query engine and risk gate before any order reaches the exchange.

**Entry modes** (what triggers the entry signal):
- `confirmed_trend` — SMA crossover signal (default)
- `confirmed_reversal` — reversal candlestick pattern
- `confirmed_key_level` — key level breakout
- `immediate` — operator injects signal manually via API

**Approval modes** (whether human confirmation is required):
- `automatic` — entry proceeds without operator action (default)
- `human_confirmation` — operator must approve via dashboard before the order is placed

## Why Robson Exists

Most open-source trading systems conflate signal generation with execution. The result is software where risk management is an afterthought bolted onto an indicator library.

Robson inverts this. The execution and risk layers are the primary concern. The operator makes the trading decision — Robson executes it deterministically, sizes it correctly, and enforces the stop.

In leveraged markets, **how** you execute matters more than **what** you execute. A sound signal with poor execution, missing stop logic, or uncontrolled position sizing will lose capital. Robson exists to make the execution path deterministic, auditable, and safe by default.

## Architecture

The canonical runtime is written in Rust and lives at the repository root. The SvelteKit operations dashboard lives under `frontend/`.

```
robson-domain/       # Pure domain logic — no external dependencies
robson-engine/       # Decision engine (risk calculations, position sizing)
robson-exec/         # Execution layer (port definitions, orchestration)
robson-connectors/   # Exchange adapters (Binance Futures)
robson-store/        # PostgreSQL persistence (SQLx)
robsond/             # Runtime daemon (Axum HTTP API, control loop)
robson-sim/          # Backtesting and simulation
cli/                 # Operator CLI (Bun / TypeScript)

frontend/              # SvelteKit operations dashboard

docs/
  adr/                 # Architecture Decision Records
  architecture/        # System specs, migration plans, v4 backlog
  policies/            # Risk and reconciliation policies
  runbooks/            # Operational procedures
```

### Core Subsystems

**Execution Engine** — Manages the full position lifecycle through explicit state transitions. Every transition is governed by the query engine and produces an immutable audit event. No implicit side effects.

**Risk Engine** — Enforces per-position and portfolio-level constraints before and during execution. Position sizing is derived from the Golden Rule. Every exit carries a typed reason code.

**Detector** — Monitors market conditions for the entry mode chosen at ARM time. Fires a single entry signal when conditions are met. It is a governed input boundary — signals do not bypass the risk gate.

**Reconciliation Worker** — Continuously verifies that every open position on the Binance account traces to a `robsond`-authored entry. Untracked positions are automatically closed. This invariant is non-negotiable (ADR-0022).

**Query Engine** — Every state transition passes through a lifecycle-tracked `ExecutionQuery`: `Accepted → Processing → RiskChecked → Acting → Completed / Denied / Failed`. Denials are governed outcomes, not errors.

**Event Stream** — All domain events are persisted and broadcast via SSE. The dashboard updates in real time.

## API

```
GET  /health                          # Health check
GET  /status                          # Positions, slots, monthly risk state
GET  /debug/armed-positions           # Armed positions debug snapshot (detector status, market data)
GET  /positions?month=YYYY-MM         # Monthly position history
GET  /positions/{id}                  # Single position detail
POST /positions                       # Arm a new position
POST /positions/{id}/signal           # Inject entry signal (immediate mode)
DEL  /positions/{id}                  # Cancel Armed / close Active position
POST /queries/{id}/approve            # Approve a pending human-confirmation query
GET  /monthly-halt                    # Monthly halt status
POST /monthly-halt                    # Trigger halt manually (kill switch)
POST /panic                           # Emergency close all open positions
GET  /safety/status                   # Reconciliation worker status
GET  /events                          # SSE event stream (bearer token via query param)
```

## Development

### Prerequisites

- Rust stable + nightly (nightly required for `rustfmt`)
- PostgreSQL
- [pnpm](https://pnpm.io) — frontend (`frontend/`)
- [Bun](https://bun.sh) — operator CLI (`cli/`)
- [`just`](https://just.systems) — task runner

### Backend (Rust)

```bash
# Build
cargo build

# Unit and in-memory tests (no database needed)
cargo test --all

# Format (nightly rustfmt)
cargo +nightly fmt --all

# Lint
cargo clippy --all-targets -- -D warnings

# PostgreSQL integration tests (requires DATABASE_URL)
just v2-db-up        # start local database container
just test-pg         # run integration tests against real DB
```

### Frontend (SvelteKit)

```bash
cd frontend
pnpm install
pnpm dev             # development server
pnpm check           # type check (svelte-check + tsc)
pnpm build           # production build
```

### CLI (Bun)

```bash
cd cli
bun install
bun run dev          # run CLI in development mode
bun test             # run tests
```

### Environment

Copy `.env.example` and configure `DATABASE_URL` and exchange credentials. The daemon reads configuration from environment variables and a `robsond.toml` file.

## Deployment

Production deployments are performed via GitOps (GitHub Actions + ArgoCD + k3s) with Traefik ingress and cert-manager-managed TLS. Infrastructure automation is managed separately in `rbx-infra`.

See `docs/runbooks/` for deployment and operational procedures.

## AI-First Repository Rules

Canonical AI-first instructions live in [AGENTS.md](AGENTS.md). Vendor-specific files such as [CLAUDE.md](CLAUDE.md) are compatibility adapters and must remain thin.

## License

Open source. See [LICENSE](LICENSE) for details.
