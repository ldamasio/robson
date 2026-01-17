# Robson v2

**Status**: Development (Alpha)
**Version**: 2.0.0-alpha

Complete rewrite of Robson trading platform with focus on reliability, safety, and operational excellence.

## Architecture

- **Core**: Rust (100%)
- **CLI**: Bun + TypeScript
- **Database**: PostgreSQL
- **Deployment**: Kubernetes (k3s)

## Components

| Crate | Description |
|-------|-------------|
| `robson-domain` | Pure domain logic (entities, value objects, invariants) |
| `robson-engine` | Decision engine (pure, deterministic, no I/O) |
| `robson-exec` | Execution layer (idempotent order execution) |
| `robson-connectors` | Exchange adapters (REST + WebSocket) |
| `robson-store` | PostgreSQL persistence + event sourcing |
| `robsond` | Runtime daemon (orchestration + API server) |
| `robson-sim` | Backtesting and simulation |

## Quick Start

### Prerequisites

- Rust 1.75+
- Bun 1.0+
- PostgreSQL 16+

### Build

```bash
cd v2
cargo build --all
```

### Run Tests

```bash
cargo test --all
```

### Run Daemon

```bash
export DATABASE_URL="postgres://postgres:password@localhost/robson"
cargo run -p robsond
```

### CLI

```bash
cd cli
bun install
bun run src/index.ts status
```

## Documentation

- [Architecture](./docs/ARCHITECTURE.md)
- [Domain Model](./docs/DOMAIN.md)
- [Reliability](./docs/RELIABILITY.md)
- [CLI Reference](./docs/CLI.md)
- [Execution Plan](./docs/EXECUTION-PLAN.md)
- [Phase 6: Detector Runtime](./docs/PHASE_6.md) ← NEW
- [Prompt Pack](./docs/PROMPT-PACK.md) (for agentic coding)

## Development

### For Contributors

**New to the project?** Start here:

1. **[CONTRIBUTING.md](CONTRIBUTING.md)** - Contribution guidelines, code standards, PR process
2. **[CLAUDE.md](CLAUDE.md)** - Repository context for AI assistants and development conventions
3. **[Prompt Pack](./docs/PROMPT-PACK.md)** - Step-by-step implementation guide

### Development Workflow

```bash
# 1. Setup (first time only)
cargo build --all
cd cli && bun install && cd ..

# 2. Verify code quality
./scripts/verify.sh          # Full verification (format, lint, tests)
./scripts/verify.sh --fast   # Fast mode (skip tests)
./scripts/verify.sh --rust   # Rust only
./scripts/verify.sh --cli    # CLI only

# 3. Format code
cargo fmt --all

# 4. Run linter
cargo clippy --all-targets -- -D warnings

# 5. Run tests
cargo test --all

# 6. Create feature branch
git checkout -b feat/your-feature

# 7. Commit with conventional commits
git commit -m "feat(domain): add Order entity"
```

### Code Quality Tools

| Tool | Purpose | Command |
|------|---------|---------|
| **rustfmt** | Code formatting | `cargo fmt --all` |
| **clippy** | Linting | `cargo clippy -- -D warnings` |
| **cargo test** | Unit & integration tests | `cargo test --all` |
| **verify.sh** | All-in-one verification | `./scripts/verify.sh` |

Configuration files:
- `rustfmt.toml` - Rust formatting rules
- `clippy.toml` - Clippy lint rules
- `.cargo/config.toml` - Cargo workspace config

### Claude Code Integration

**AI-Assisted Development** with automatic quality checks:

```bash
# Optional: Enable Claude Code hooks
# Hooks automatically run verification after edits

export CLAUDE_HOOK_FAST=1     # Fast mode (skip tests)
export CLAUDE_HOOK_DISABLED=1 # Disable hooks

# See .claude/hooks/README.md for details
```

**MCP Integration** (optional):
- GitHub integration for code search
- PostgreSQL integration for schema inspection
- See `.claude/mcp/README.md` for setup

### Definition of Done

Before creating a PR, ensure:

- [ ] Code compiles without warnings
- [ ] `cargo fmt --all` applied
- [ ] `cargo clippy -- -D warnings` passes
- [ ] All tests pass (`cargo test --all`)
- [ ] No `unwrap()`/`expect()` in production code
- [ ] Uses `Decimal` for financial amounts (not `f64`)
- [ ] Commit messages follow Conventional Commits
- [ ] Documentation updated (if needed)
- [ ] English only (code, comments, docs)

### Key Principles

1. **Financial Precision**: Always use `rust_decimal::Decimal` for money/quantities
2. **Error Handling**: Return `Result<T, E>`, never `unwrap()` in production
3. **Hexagonal Architecture**: Domain crate has zero external dependencies
4. **Type Safety**: Use Rust's type system for correctness
5. **Test Coverage**: Aim for >80% on domain/engine logic

### Directory Structure

```
v2/
├── robson-domain/      # Pure domain logic (NO external deps)
├── robson-engine/      # Business logic (NO I/O)
├── robson-exec/        # Execution orchestration
├── robson-connectors/  # Exchange adapters (Binance, etc.)
├── robson-store/       # PostgreSQL persistence
├── robsond/            # Runtime daemon (API server)
├── robson-sim/         # Backtesting engine
├── cli/                # Bun + TypeScript CLI
├── scripts/            # Development scripts (verify.sh)
├── .claude/            # Claude Code hooks & MCP config
└── docs/               # Documentation (in v2/docs/)
```

## License

MIT
