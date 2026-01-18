# Robson v2 Documentation Index

**Version**: 2.0.0-alpha
**Last Updated**: 2026-01-12

---

## Quick Navigation


---

## Reading Order

### For Product/Business

1. Start: [ARCHITECTURE.md](./ARCHITECTURE.md) - Executive Summary
2. Read: [DOMAIN.md](./DOMAIN.md) - Core Concepts section
3. Try: [CLI.md](./CLI.md) - Examples section

**Time**: 30 minutes

---

### For Engineering (Architecture Review)

1. [ARCHITECTURE.md](./ARCHITECTURE.md) - Full document
2. [RELIABILITY.md](./RELIABILITY.md) - Full document
3. [DOMAIN.md](./DOMAIN.md) - Full document

**Time**: 2 hours

**Key Sections**:
- Architecture Decision Records (ADRs)
- Failure mode analysis
- State machine transitions
- Position sizing golden rule

---

### For Implementation (Developers)

1. [EXECUTION-PLAN.md](./EXECUTION-PLAN.md) - Review all phases
2. [PROMPT-PACK.md](./PROMPT-PACK.md) - Execute prompts sequentially
3. Reference: [DOMAIN.md](./DOMAIN.md) - When implementing business logic
4. Reference: [RELIABILITY.md](./RELIABILITY.md) - When implementing storage/reconciliation

**Time**: Work in progress (4-6 weeks implementation)

---

## Document Summaries

### [ARCHITECTURE.md](./ARCHITECTURE.md)

**Purpose**: High-level system design

**Key Sections**:
- System overview diagram
- Component architecture (7 Rust crates)
- Data flow (happy path + failure scenarios)
- State machine
- Technology stack decisions
- Why Rust? Why Bun? Why Postgres?

**For**: Architects, senior engineers, product leads

---

### [RELIABILITY.md](./RELIABILITY.md)

**Purpose**: Production-grade reliability mechanisms

**Key Sections**:
- Failure mode analysis (pod crash, network partition, exchange downtime, etc.)
- Leader election via Postgres advisory locks
- Reconciliation process (on startup, after reconnect)
- Idempotency guarantees (intent journal, WAL)
- Degraded mode (safe fallback)
- Insurance stop strategy (ADR-001)
- Observability (logs, metrics, alerts)

**For**: SREs, DevOps, senior engineers

**Critical Topics**:
- How we prevent split-brain
- How we survive daemon crashes
- How we handle exchange downtime
- Source of truth: always the exchange

---

### [DOMAIN.md](./DOMAIN.md)

**Purpose**: Business logic and domain rules

**Key Sections**:
- Core concepts ("Palma da Mão", user-initiated/system-managed)
- Entities (Position, Order, Trade)
- Value objects (Price, Quantity, Symbol, PalmaDaMao, etc.)
- State machine (Armed → Entering → Active → Exiting → Closed)
- **Position sizing golden rule** (CRITICAL)
- Risk management rules
- Events for event sourcing

**For**: All developers, product team

**Golden Rule**:
```
Position Size = (Capital × Risk %) / Palma Distance
```
Where Palma = |Entry - Technical Stop Loss|

**Critical Principle**: Position size is DERIVED from technical stop, NOT chosen arbitrarily.

---

### [CLI.md](./CLI.md)

**Purpose**: User-facing command reference

**Key Sections**:
- All CLI commands (arm, disarm, status, panic)
- JSON output schemas (for automation)
- Examples (basic workflow, automation scripts, monitoring)
- Configuration (env vars, config file)
- Troubleshooting

**For**: Users, automation engineers, QA

**Most Used Commands**:
```bash
robson arm BTCUSDT --strategy all-in
robson status --watch
robson panic --confirm
```

---

### [SMOKE-TEST.md](./SMOKE-TEST.md)

**Purpose**: Operational smoke test for MVP validation

**Key Sections**:
- Prerequisites (Rust, Bun only)
- Copy/paste test cases for all CLI commands
- Runtime invariants checklist (leverage, margin safety, sizing, etc.)
- Troubleshooting guide (5+ common issues)

**For**: QA, developers, anyone validating the MVP locally

**Quick Start**:
```bash
# Start daemon
cargo run -p robsond &

# Run smoke test
# See docs/SMOKE-TEST.md for full test suite
bun run dev arm BTCUSDT --capital 1000 --risk 1 --side long
```

---

### [EXECUTION-PLAN.md](./EXECUTION-PLAN.md)

**Purpose**: Step-by-step implementation roadmap

**Key Sections**:
- 10 phases from bootstrap to production
- Small, measurable steps with acceptance criteria
- Validation commands for each step
- Estimated LOC and timeline

**For**: Implementation team, project managers

**Phases**:
- **Phase 0**: Bootstrap (workspace, CLI skeleton) - ✅ DONE
- **Phase 1**: Domain types (pure logic)
- **Phase 2**: Engine (decision logic)
- **Phase 3**: Storage (PostgreSQL)
- **Phase 4**: Execution (idempotency)
- **Phase 5**: Daemon (runtime)
- **Phase 6**: CLI integration
- **Phase 7**: E2E test (MVP milestone)
- **Phase 8**: Detector interface
- **Phase 9**: Real exchange connector
- **Phase 10**: Production readiness

---

### [PROMPT-PACK.md](./PROMPT-PACK.md)

**Purpose**: Agentic coding execution guide

**Key Sections**:
- 20+ copy-paste prompts
- Sequential execution (Prompt 0.1 → 0.2 → 1.1 → 1.2 → ...)
- Clear validation commands
- Checklists for acceptance criteria

**For**: AI assistants, developers using agentic tools

**How to Use**:
1. Copy prompt (e.g., Prompt 1.1: Implement Value Objects)
2. Paste to AI assistant or execute manually
3. Run validation commands
4. Check off checklist
5. Move to next prompt

**Example Prompt**:
```
Prompt 1.1: Implement Value Objects

Create value_objects.rs with: Price, Quantity, Symbol, Side, Leverage, PalmaDaMao

Each must have:
- Private inner field
- new() constructor with validation
- Result<Self, DomainError> return type

Add tests for valid/invalid values.

Validation: cargo test -p robson-domain

Checklist:
- [ ] All value objects implemented
- [ ] Validation works
- [ ] Tests pass (minimum 10 tests)
```

---

## Architecture Decision Records (ADRs)

### ADR-001: Dual-Stop Strategy (Insurance + Local)

**File**: [RELIABILITY.md](./RELIABILITY.md#insurance-stop-optional)

**Problem**: Daemon downtime during stop loss trigger = uncontrolled loss

**Decision**: Place insurance stop on exchange as backup

**Trade-offs**:
- 🟢 Increased safety (worst-case loss capped)
- 🔴 Increased complexity (two stop mechanisms)
- 🟡 Risk of false trigger (mitigated with LIMIT orders)

---

### ADR-002: Postgres Advisory Locks for Leader Election

**File**: [RELIABILITY.md](./RELIABILITY.md#leader-election)

**Problem**: Need leader election for HA

**Decision**: Use Postgres advisory locks + TTL table

**Alternatives Considered**:
- Kubernetes Lease API (separate system)
- etcd/Consul (overkill)
- Redis (less ACID)

**Why Postgres**: Single source of truth, simpler deployment

---

### ADR-003: Event Sourcing with Snapshots

**File**: [RELIABILITY.md](./RELIABILITY.md#event-sourcing-with-snapshots)

**Problem**: How to reconstruct position state after failures?

**Decision**: Append-only event log + periodic snapshots

**Benefits**:
- Complete audit trail
- Can replay for debugging
- Reconciliation after crashes

---

## Key Concepts Reference

### "Palma da Mão" (Palm of the Hand)

**Definition**: Distance between entry price and technical stop loss

**Why Universal?**:
- Structural foundation for position sizing
- Risk is ALWAYS defined by technical invalidation level
- NOT arbitrary percentage

**Example**:
```
Entry: $95,000
Technical SL: $93,500
Palma: $1,500 (1.58%)

Position Size = ($10,000 × 1%) / $1,500 = 0.0666 BTC
```

**File**: [DOMAIN.md](./DOMAIN.md#palma-da-mão-technical-stop-distance)

---

### User-Initiated, System-Managed

**Principle**: User arms → System decides → User confirms

**What User Does**:
- Choose symbol
- Choose strategy
- Choose capital allocation

**What System Does**:
- Calculate entry price (detector)
- Calculate stop loss (technical analysis)
- Calculate position size (golden rule)
- Execute entry/exit (market orders)
- Monitor SL/SG 24/7

**File**: [DOMAIN.md](./DOMAIN.md#user-initiated-system-managed)

---

### State Machine

```
Armed → Entering → Active → Exiting → Closed
         (or Error at any stage)
```

**Valid Transitions**:
- Armed → Entering: Detector signal received
- Entering → Active: Entry order filled
- Active → Exiting: SL/SG trigger or panic
- Exiting → Closed: Exit order filled

**Invalid Transitions**:
- Armed → Active: Cannot skip entering
- Active → Entering: Cannot re-enter

**File**: [DOMAIN.md](./DOMAIN.md#state-machine)

---

## Validation Checklist

### After Phase 7 (MVP)

- [ ] Workspace compiles: `cargo build --all`
- [ ] Tests pass: `cargo test --all`
- [ ] Clippy clean: `cargo clippy --all -- -D warnings`
- [ ] Daemon runs: `cargo run -p robsond`
- [ ] API responds: `curl http://localhost:8080/health/live`
- [ ] CLI works: `cd v2/cli && bun run dev status`
- [ ] Can arm position via CLI
- [ ] Position transitions through states
- [ ] Events logged to database
- [ ] Lease acquired successfully
- [ ] **Smoke test passes**: See [SMOKE-TEST.md](./SMOKE-TEST.md)

---

## Quick Links

### Code

- [Rust Workspace](../../v2/Cargo.toml)
- [Domain Crate](../../v2/robson-domain/)
- [Engine Crate](../../v2/robson-engine/)
- [Daemon](../../v2/robsond/)
- [CLI](../../v2/cli/)

### Docs

- [Delivery Summary](../../v2/DELIVERY-SUMMARY.md)
- [Project README](../../v2/README.md)
- [CLI README](../../v2/cli/README.md)

---

## FAQ

### Q: Where should I start reading?

**A**: Depends on your role:
- **Product/Business**: ARCHITECTURE.md Executive Summary
- **Engineering**: ARCHITECTURE.md → RELIABILITY.md → DOMAIN.md
- **Implementation**: EXECUTION-PLAN.md → PROMPT-PACK.md

---

### Q: What's the difference between "Palma da Mão" and "stop distance"?

**A**: They're the same thing. "Palma da Mão" (Palm of the Hand) is the business term. "Stop distance" is the technical term. It's the distance between entry and technical stop loss.

---

### Q: Why Rust instead of Python/Go?

**A**:
- **Safety**: No segfaults, no data races, catches errors at compile time
- **Performance**: Low latency, predictable performance
- **Correctness**: Type system enforces invariants
- **Decimal math**: `rust_decimal` for precise financial calculations

See: [ARCHITECTURE.md - Why Rust?](./ARCHITECTURE.md#why-rust-for-core)

---

### Q: How do we prevent duplicate orders?

**A**: Intent journal with write-ahead log (WAL). Every order gets a unique intent_id. Before execution, we write to journal. On retry, we check journal first.

See: [RELIABILITY.md - Idempotency](./RELIABILITY.md#idempotency)

---

### Q: What happens if the daemon crashes mid-trade?

**A**:
1. On restart, acquire lease
2. Reconcile state with exchange
3. If price passed SL during downtime → Close immediately (degraded mode)
4. Resume normal operation

See: [RELIABILITY.md - Pod Crash](./RELIABILITY.md#1-pod-crash-oomkill-panic-bug)

---

### Q: How do we prevent split-brain (two active traders)?

**A**: Leader election via Postgres advisory locks. Only one daemon can hold lease per (account, symbol). Lease has TTL and requires heartbeat renewal.

See: [RELIABILITY.md - Leader Election](./RELIABILITY.md#leader-election)

---

## Glossary

| Term | Definition |
|------|------------|
| **Palma da Mão** | Distance between entry and technical stop loss |
| **Armed** | Position waiting for detector signal |
| **Entering** | Entry order placed, waiting for fill |
| **Active** | Position open, monitoring SL/SG |
| **Exiting** | Exit order placed, waiting for fill |
| **Closed** | Position fully closed, PnL realized |
| **Intent** | Write-ahead log entry for idempotency |
| **Lease** | Lock for leader election (per account+symbol) |
| **Reconciliation** | Process to sync local state with exchange |
| **Degraded Mode** | Safe fallback when state mismatch detected |

---

## Status

**Planning**: ✅ Complete
**Skeleton**: ✅ Complete
**Implementation**: ⏳ Ready to start
**Production**: 🔜 4-6 weeks away

---

**Last Updated**: 2026-01-12
**Next Action**: Execute Prompt 1.1 from PROMPT-PACK.md
