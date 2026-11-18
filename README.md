# Robson: Execution & Risk Management

[![Backend Tests](https://github.com/ldamasio/robson/actions/workflows/robsond.yml/badge.svg)](https://github.com/ldamasio/robson/actions/workflows/robsond.yml)

**[Public roadmap: v2.5 → v2.6 Spot-first](ROADMAP.md)** · [Release policy and status](docs/RELEASING.md) · [Architecture decisions](docs/adr/ADR-0056-core-risk-vs-venue-adapters.md)

Robson is an execution and risk management system. The operator decides the entry and arms it; Robson sizes and executes governed orders and manages their lifecycle under deterministic risk policies. It does not originate trade ideas, predict prices, scan for opportunities, or use an LLM to make trading decisions.

**Status (2026-11-16): v2.6 is the next product target: Binance Spot, LONG-only. The transition is in progress and is not ready for real capital.** Spot adapters, tests and ledger foundations are present, but they do not constitute a complete or activated Spot runtime. The current execution path is the legacy v2.5 Binance USD-M Futures implementation at fixed 1x leverage; the Futures behavior described below is current context, not a claim that Spot is operational. Activation remains an explicit operator decision after the [acceptance gates](ROADMAP.md#next-gates-before-activation).

Robson is concerned with what happens **after** a trading decision is made: position sizing from chart-derived stops, governed order execution, lifecycle management through entry to settlement, and safe failure handling under volatile conditions.

The system provides a single-operator runtime with budget-metered entry capacity, deterministic execution semantics, and lifecycle audit records. Slots express remaining risk capacity rather than a fixed position-count limit.

## Release identity

This source carries **`2.6.0-alpha.2`**. Published snapshots, their exact source identities, artifacts and scoped validation are recorded in [Releases](https://github.com/ldamasio/robson/releases); package metadata alone does not assert publication or deployment. The first development snapshot, **[v2.6.0-alpha.1](https://github.com/ldamasio/robson/releases/tag/v2.6.0-alpha.1)**, was published on 2026-11-15 from `ccac64c39dfaa2c4c88e4a46d4925f3e469674bd`. Progress follows alpha → beta → rc → `2.6.0`; `2.5.x` is compatible maintenance only. Spot operational acceptance remains pending.

The historical [`v2.5.0` source](https://github.com/ldamasio/robson/tree/v2.5.0) is the 2026-05-09 [I3 reconciliation merge](https://github.com/ldamasio/robson/pull/59), not the later July/August development entries. See the [correction record and original notes](docs/RELEASING.md#read-version-labels-carefully). Release identity does not establish operational readiness or authorize capital.

## Risk Model at a Glance

- **Monthly budget**: 4% of `capital_base`, with consumption governed by the month's persisted budget model. [ADR-0051](docs/adr/ADR-0051-net-from-start-monthly-budget.md) defines the net-from-start model; historical HWM months retain their recorded model. Open-position risk remains reserved against the budget
- **Per-trade risk**: 1% of `capital_base` as a **worst-case cap** (derived from position size, never set directly)
- **Position sizing**: `size = (capital_base × 1%) / (technical_stop_distance + gap_allowance + round_trip_taker_fees)` — the budget absorbs execution costs, not just chart distance, within the reviewed execution envelope ([ADR-0039](docs/adr/ADR-0039-exchange-side-insurance-stop.md))
- **Technical stop**: always from chart analysis (second S/R level, 15m timeframe) — never a percentage of entry price
- **Current Futures stop enforcement**: two independent layers at one executable trigger. A robsond-authored reduce-only conditional stop order rests on the exchange and fills first in practice; the software monitor evaluates every trade (discrete trailing, audit ticks persisted at most once per 20 s per position) and is the fallback, so stop enforcement survives daemon outages (ADR-0039)
- **Exit execution**: exits and protective stops are always market orders (taker) — non-execution there is an unbounded loss; fee optimization is only ever applied to legs where waiting is costless (ADR-0040)
- **Entry capacity**: budget-metered (ADR-0043). Each entry is charged its actual planned worst-case loss against the monthly budget, not the full 1% cap — **at least 4 full-cap operations per month are guaranteed, and saved risk becomes extra operations**. No static cap on concurrent positions or entries per day; the budget is the only constraint

`capital_base` is governed by the persisted monthly budget model. Net-from-start **requires** no intra-month increase, including from confirmed deposits; this is a policy, not a proven concurrent implementation guarantee. [Position reconciliation](robsond/src/reconciliation_worker.rs) no longer has a wallet-total basis writer: successful scans use the existing guarded, alarm-only checksum. Startup `auto_reconcile` returns the stale-Active error before Phase 2 if real evidence is missing or ambiguous. These scoped changes do not make all accounting paths safe or establish Spot acceptance.

**Required before Spot capital acceptance:** atomic validation against current persisted state, single-use transfer attribution, exact trade/amount/asset linkage, durable missing-evidence and freshness reporting with status errors kept distinct from zero, and complete Spot settlement, reservations and month-boundary carry. The [income ledger](robsond/src/income_ledger.rs) still uses a TRANSFER aggregate and idle-book snapshots; funding recognition remains distinct from its wallet-accounting integration. Startup Phase 2 remains sequential rather than transactional. See [ADR-0045's implementation limits](docs/adr/ADR-0045-income-ledger-reconciliation.md) and the [recovery runbook](docs/runbooks/td-2026-05-05-001-stale-active-recovery.md).

## Position Lifecycle

```
Armed → Entering → Active → Exiting → Closed
  └─ Cancelled (disarmed before entry)
  └─ Error (unrecoverable, requires operator action)
```

The operator **arms** a position by specifying a symbol and direction
(Long/Short). In the v2.5 operator surface, Robson processes the entry
immediately at ARM time. The request still passes through technical-stop
analysis, the query engine, and the risk gate before any order reaches the
exchange.

**Supported v2.5 entry mode**:

- `immediate`: entry intent is generated and processed at ARM time, still
  subject to the selected approval mode and every risk control

The backend retains `confirmed_trend`, `confirmed_reversal`, and
`confirmed_key_level` for historical event/API compatibility. They are not
operationally accepted for v2.5 and are intentionally unavailable in the
dashboard.

**Approval modes** (whether human confirmation is required):

- `automatic`: entry proceeds without operator action (default)
- `human_confirmation`: operator must approve via dashboard before the order is placed

## Why Robson Exists

Most open-source trading systems conflate signal generation with execution. The result is software where risk management is an afterthought bolted onto an indicator library.

Robson inverts this. The execution and risk layers are the primary concern. The operator decides the entry; Robson times and executes it, sizes the position from the technical stop, and manages the exit deterministically.

Execution quality matters across market types: missing protection, uncontrolled sizing and ambiguous settlement can put capital at risk. Robson makes those responsibilities explicit and auditable. The Spot transition must demonstrate them for balance-account inventory before activation.

## Architecture

The canonical runtime is written in Rust and lives at the repository root. The SvelteKit operations dashboard lives under `frontend/`.

```
robson-domain/       # Pure domain logic — no external dependencies
robson-engine/       # Decision engine (risk calculations, position sizing)
robson-exec/         # Execution layer (port definitions, orchestration)
robson-connectors/   # Binance adapters; Spot components await runtime activation
robson-store/        # PostgreSQL persistence (SQLx)
robson-eventlog/     # Append-only event log, stream sequencing and idempotency
robson-projector/    # Apply logged events to persisted projections
robson-db/           # Database migrations and initialization
robson-testkit/      # Dev-only fixtures, fake venues and conformance helpers
robsond/             # Runtime daemon (Axum HTTP API, control loop)
robson-sim/          # Backtesting and simulation
robson-cli/          # Exceptional operator recovery CLI (Rust)

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

**Detector** — Preserves the governed strategy boundary used by deferred entry
policies. The supported v2.5 `immediate` mode bypasses strategy waiting but
does not bypass technical-stop analysis, the query engine, or the risk gate.

**Reconciliation Worker** — Runs startup and periodic USD-M Futures scans and
closes exchange positions that have no matching active local `(symbol, side)`.
Exact originating-order correlation and wider account coverage remain ADR-0022
follow-up work; the authorship invariant itself is non-negotiable.

**Query Engine** — Every state transition passes through a lifecycle-tracked `ExecutionQuery`: `Accepted → Processing → RiskChecked → Acting → Completed / Denied / Failed`. Denials are governed outcomes, not errors.

**Event Stream** — All domain events are persisted in the canonical event log. An authenticated SSE projection carries incremental runtime updates, while an authenticated REST bootstrap supplies durable events for the dashboard's current UTC day.

## API

```
GET  /health                          # Public health check
GET  /auth/session                    # Verified RBX subject, tenant, roles, permissions
GET  /status                          # Positions, slots, monthly risk state
GET  /positions?month=YYYY-MM         # Monthly position history
GET  /positions/{id}                  # Single position detail
POST /positions                       # Arm a new position
POST /positions/{id}/signal           # Inject entry signal (testing)
DEL  /positions/{id}                  # Cancel Armed / close Active position
POST /queries/{id}/approve            # Approve a pending human-confirmation query
GET  /monthly-halt                    # Monthly halt status
POST /monthly-halt                    # Trigger halt manually (kill switch)
POST /panic                           # Emergency close all open positions
GET  /events/history?date=YYYY-MM-DD  # Latest 100 durable events for a UTC day (bearer header)
GET  /events                          # SSE event stream (bearer header)
```

Every route except health, readiness, and metrics requires an RBX
Identity/ZITADEL access token. Read routes require an observer-capable role;
ordinary mutations require `operator`, funding mutations require `funding`,
and panic/monthly-halt require `emergency` (ADR-0057).

## Development

### Prerequisites

- Rust stable + nightly (nightly required for `rustfmt`)
- PostgreSQL
- [pnpm](https://pnpm.io) — frontend (`frontend/`)
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

### Operator CLI (Rust)

The narrow `robson-cli` binary supports exceptional reconciliation and income-ledger recovery. Routine trading operations use the SvelteKit dashboard or authenticated `robsond` API.

```bash
cargo build --release -p robson-cli
./target/release/robson-cli --help
```

The CLI is not currently packaged in the runtime image or published as a versioned artifact. See [docs/CLI.md](docs/CLI.md) before using it against an operated environment.

### Environment

Copy `.env.example` and configure `DATABASE_URL` and exchange credentials. The daemon loads configuration from environment variables and an optional `.env` file; accepted names and defaults are defined in [config.rs](robsond/src/config.rs).

## Deployment

Production deployments are performed via GitOps (GitHub Actions + ArgoCD + k3s) with Traefik ingress and cert-manager-managed TLS. Infrastructure automation is managed separately in `rbx-infra`.

See `docs/runbooks/` for deployment and operational procedures.

## AI-First Repository Rules

Canonical AI-first instructions live in [AGENTS.md](AGENTS.md). Vendor-specific files such as [CLAUDE.md](CLAUDE.md) are compatibility adapters and must remain thin.

## License

Open source. See [LICENSE](LICENSE) for details.
