# Contributing to Robson v2

Thank you for contributing to Robson v2! This document provides guidelines for contributing code, documentation, and other improvements.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Workflow](#development-workflow)
- [Code Standards](#code-standards)
- [Commit Guidelines](#commit-guidelines)
- [Pull Request Process](#pull-request-process)
- [Testing](#testing)
- [Documentation](#documentation)

---

## Code of Conduct

### Core Principles

1. **English Only**: All code, comments, documentation, and commit messages must be in English
2. **Safety First**: No `unwrap()` or `expect()` in production code
3. **Financial Precision**: Always use `rust_decimal::Decimal` for money/quantities (never `f64`)
4. **Test Coverage**: All business logic must have tests (aim for >80% coverage)
5. **Incremental Changes**: Small, focused commits and PRs

### Respect and Collaboration

- Be respectful and constructive in code reviews
- Assume positive intent
- Focus on the code, not the person
- Welcome newcomers and help them learn

---

## Getting Started

### Prerequisites

**Required**:
- Rust 1.83+ (`rustup`)
- Bun latest (`curl -fsSL https://bun.sh/install | bash`)
- Git 2.30+
- PostgreSQL 14+ (for database-dependent features)

**Optional**:
- SQLx CLI (`cargo install sqlx-cli`)
- Cargo-audit (`cargo install cargo-audit`)
- Cargo-watch (`cargo install cargo-watch`)

### First-Time Setup

```bash
# 1. Fork the repository on GitHub
# (Click "Fork" button at https://github.com/ldamasio/robson)

# 2. Clone your fork
git clone https://github.com/YOUR_USERNAME/robson.git
cd robson/v2

# 3. Add upstream remote
git remote add upstream https://github.com/ldamasio/robson.git

# 4. Install Rust dependencies
cargo build

# 5. Install CLI dependencies
cd cli && bun install && cd ..

# 6. Run verification to ensure setup is correct
./scripts/verify.sh
```

### Keeping Your Fork Updated

```bash
# Fetch latest changes from upstream
git fetch upstream

# Update your main branch
git checkout main
git merge upstream/main

# Push to your fork
git push origin main
```

---

## Development Workflow

### 1. Create a Feature Branch

```bash
# Update main first
git checkout main
git pull upstream main

# Create feature branch (use conventional naming)
git checkout -b feat/your-feature-name
# or
git checkout -b fix/bug-description
```

### Branch Naming Conventions

| Prefix | Purpose | Example |
|--------|---------|---------|
| `feat/` | New feature | `feat/add-order-entity` |
| `fix/` | Bug fix | `fix/division-by-zero` |
| `refactor/` | Code refactoring | `refactor/extract-risk-calculator` |
| `docs/` | Documentation only | `docs/update-architecture` |
| `test/` | Add/update tests | `test/add-engine-tests` |
| `chore/` | Maintenance | `chore/update-dependencies` |

### 2. Make Your Changes

Follow the **incremental development cycle**:

```bash
# A. Edit code (small, focused changes)
vim robson-domain/src/entities.rs

# B. Verify locally (BEFORE committing)
./scripts/verify.sh --fast  # Quick check

# C. Run full verification
./scripts/verify.sh  # Includes tests

# D. Commit (if verification passed)
git add .
git commit -m "feat(domain): add Order entity"
```

### 3. Keep Commits Small and Focused

**Good example** (one logical change):
```
feat(domain): add Order entity

- Create Order struct with id, symbol, side, quantity, price
- Implement Order::new() constructor
- Add OrderStatus enum (Pending, Executed, Cancelled)
- Add tests for order creation and state transitions
```

**Bad example** (multiple unrelated changes):
```
feat: add orders and fix bug and refactor engine

- Add Order entity
- Fix division by zero in risk calculator
- Refactor engine to use new architecture
- Update documentation
- Add tests for everything
```

### 4. Push and Create Pull Request

```bash
# Push to your fork
git push origin feat/your-feature-name

# Create PR on GitHub
gh pr create --title "feat: add Order entity" --body "Description..."
# or use GitHub web UI
```

---

## Code Standards

### Rust Code Style

#### 1. Formatting

**Use `rustfmt`** (enforced by CI):

```bash
cargo fmt --all
```

Configuration: `rustfmt.toml`

#### 2. Linting

**Use `clippy`** (strict mode):

```bash
cargo clippy --all-targets -- -D warnings
```

**No warnings allowed** - treat all warnings as errors.

#### 3. Error Handling

**Never use `unwrap()` or `expect()` in production code**:

```rust
// ❌ BAD
pub fn calculate(value: Decimal) -> Decimal {
    let result = value / Decimal::from(2);
    result.unwrap()  // NEVER do this
}

// ✅ GOOD
pub fn calculate(value: Decimal) -> Result<Decimal, DomainError> {
    let divisor = Decimal::from(2);
    value.checked_div(divisor)
        .ok_or(DomainError::DivisionError)
}

// ✅ OK in tests
#[test]
fn test_calculate() {
    let result = calculate(dec!(100)).unwrap();  // OK in tests
    assert_eq!(result, dec!(50));
}
```

#### 4. Financial Precision

**Always use `rust_decimal::Decimal`**:

```rust
// ❌ BAD
let price: f64 = 50000.0;
let quantity: f64 = 0.1;
let total = price * quantity;  // Precision loss!

// ✅ GOOD
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

let price = dec!(50000);
let quantity = dec!(0.1);
let total = price * quantity;  // Exact precision
```

#### 5. Async/Await

**Use Tokio idioms properly**:

```rust
// ✅ GOOD
use tokio::time::{sleep, Duration};

pub async fn fetch_data() -> Result<Data, ApiError> {
    let client = reqwest::Client::new();
    let response = client
        .get("https://api.example.com/data")
        .timeout(Duration::from_secs(5))
        .send()
        .await?;

    response.json().await
}

// ❌ BAD
pub async fn fetch_data() -> Data {
    std::thread::sleep(Duration::from_secs(1));  // Blocks entire runtime!
    // ...
}
```

### TypeScript Code Style

#### 1. Type Safety

**Always use explicit types**:

```typescript
// ✅ GOOD
interface Order {
  id: string;
  symbol: string;
  quantity: string;  // Decimal as string
  price: string;
}

function createOrder(order: Order): Promise<Order> {
  // ...
}

// ❌ BAD
function createOrder(order: any): Promise<any> {
  // ...
}
```

#### 2. Error Handling

**Use try/catch for async operations**:

```typescript
// ✅ GOOD
async function fetchOrder(id: string): Promise<Order> {
  try {
    const response = await axios.get(`/api/orders/${id}`);
    return response.data;
  } catch (error) {
    if (axios.isAxiosError(error)) {
      throw new ApiError(error.message);
    }
    throw error;
  }
}
```

---

## Commit Guidelines

### Conventional Commits

All commits **must** follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

```
<type>(<scope>): <subject>

<body>

<footer>
```

### Commit Types

| Type | Description | Example |
|------|-------------|---------|
| `feat` | New feature | `feat(domain): add Order entity` |
| `fix` | Bug fix | `fix(engine): prevent division by zero` |
| `docs` | Documentation only | `docs(readme): update setup instructions` |
| `refactor` | Code refactoring | `refactor(store): extract repository trait` |
| `test` | Add/update tests | `test(domain): add Order state tests` |
| `chore` | Maintenance | `chore(deps): update rust_decimal to 1.37` |
| `perf` | Performance improvement | `perf(engine): optimize risk calculation` |

### Commit Scopes

| Scope | Crate/Area | Example |
|-------|-----------|---------|
| `domain` | robson-domain | `feat(domain): add Position entity` |
| `engine` | robson-engine | `fix(engine): correct position sizing` |
| `exec` | robson-exec | `refactor(exec): extract executor trait` |
| `connectors` | robson-connectors | `feat(connectors): add Binance adapter` |
| `store` | robson-store | `feat(store): add order repository` |
| `daemon` | robsond | `fix(daemon): handle shutdown gracefully` |
| `sim` | robson-sim | `feat(sim): add backtesting engine` |
| `cli` | cli/ (TypeScript) | `feat(cli): add status command` |

### Commit Message Examples

**Good commit messages**:

```
feat(domain): add Order entity with lifecycle states

- Create Order struct with id, symbol, side, quantity, price
- Add OrderStatus enum (Pending, Executed, Cancelled, Failed)
- Implement state transitions with validation
- Add unit tests for all state transitions

Closes #42
```

```
fix(engine): prevent division by zero in position sizing

When stop_distance is zero, calculate_position_size would panic.
Now returns DomainError::InvalidStopDistance instead.

Added test to verify error handling.

Fixes #123
```

**Bad commit messages**:

```
fix bug  ❌ (not descriptive, missing scope)
```

```
feat: add stuff  ❌ (vague, no details)
```

```
Update code  ❌ (no type, no scope, not descriptive)
```

### Commit Message Rules

1. **Subject line**:
   - Max 72 characters
   - Use imperative mood ("add", not "added" or "adds")
   - No period at the end
   - Lowercase after scope

2. **Body** (optional):
   - Wrap at 72 characters
   - Explain *what* and *why*, not *how*
   - Separate from subject with blank line

3. **Footer** (optional):
   - Reference issues: `Closes #123`, `Fixes #456`
   - Breaking changes: `BREAKING CHANGE: ...`

---

## Pull Request Process

### Before Creating a PR

**Checklist**:

- [ ] All tests pass locally (`./scripts/verify.sh`)
- [ ] Code is formatted (`cargo fmt --all`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`)
- [ ] Commits follow Conventional Commits format
- [ ] Branch is up-to-date with `main`
- [ ] Documentation is updated (if needed)

### Creating the PR

**Title**: Use Conventional Commits format

```
feat: add Order entity
fix: prevent division by zero in risk calculator
docs: update architecture documentation
```

**Description Template**:

```markdown
## Description
Brief description of what this PR does.

## Motivation
Why is this change needed? What problem does it solve?

## Changes
- List of specific changes
- One per line
- Be specific

## Testing
How was this tested?
- [ ] Unit tests added/updated
- [ ] Integration tests added/updated
- [ ] Manual testing performed

## Checklist
- [ ] Code follows project style guidelines
- [ ] Tests pass locally
- [ ] Documentation updated
- [ ] Conventional commit messages
- [ ] No unwrap/expect in production code
- [ ] Uses Decimal for financial amounts

## Related Issues
Closes #123
Relates to #456
```

### PR Review Process

1. **Automated Checks** (CI):
   - Rust tests
   - Clippy linting
   - Format checking
   - TypeScript type checking

2. **Code Review**:
   - At least one approval required
   - Address all comments
   - Update PR if requested

3. **Merge**:
   - Squash and merge (default)
   - Ensure final commit message follows Conventional Commits

### Responding to Review Comments

**Good response**:
```
Thanks for the feedback! I've:
- Fixed the unwrap() issue (commit abc123)
- Added tests for edge cases (commit def456)
- Updated documentation (commit ghi789)
```

**Bad response**:
```
Fixed  ❌ (not specific)
```

---

## Testing

### Test Requirements

**All business logic must have tests**. Aim for >80% coverage on:
- Domain entities (robson-domain)
- Engine logic (robson-engine)
- Execution layer (robson-exec)

### Test Organization

```rust
// Unit tests in same file
#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_order_creation() {
        // Arrange
        let symbol = Symbol::from("BTCUSDT");

        // Act
        let order = Order::new(symbol, OrderSide::Buy, dec!(0.1), dec!(50000));

        // Assert
        assert_eq!(order.status, OrderStatus::Pending);
    }
}
```

### Test Naming

Use descriptive test names:

```rust
// ✅ GOOD
#[test]
fn test_order_execution_transitions_status_from_pending_to_executed() { }

#[test]
fn test_zero_stop_distance_returns_invalid_stop_distance_error() { }

// ❌ BAD
#[test]
fn test1() { }

#[test]
fn test_order() { }
```

### Running Tests

```bash
# Run all tests
cargo test --all

# Run tests for specific crate
cargo test -p robson-domain

# Run specific test
cargo test test_order_creation

# Run with output
cargo test -- --nocapture

# Run with coverage (requires cargo-tarpaulin)
cargo tarpaulin --all
```

---

## Documentation

### Code Documentation

**Use doc comments** for public APIs:

```rust
/// Creates a new order with the given parameters.
///
/// # Arguments
///
/// * `symbol` - Trading pair (e.g., BTCUSDT)
/// * `side` - Buy or Sell
/// * `quantity` - Amount to trade (must be > 0)
/// * `price` - Price per unit (must be > 0)
///
/// # Returns
///
/// A new `Order` in `Pending` status
///
/// # Examples
///
/// ```
/// use robson_domain::{Order, Symbol, OrderSide};
/// use rust_decimal_macros::dec;
///
/// let order = Order::new(
///     Symbol::from("BTCUSDT"),
///     OrderSide::Buy,
///     dec!(0.1),
///     dec!(50000),
/// );
/// ```
pub fn new(symbol: Symbol, side: OrderSide, quantity: Decimal, price: Decimal) -> Self {
    // ...
}
```

### Updating Documentation

When making changes that affect:

- **Public APIs**: Update inline doc comments
- **Architecture**: Update `docs/ARCHITECTURE.md`
- **Domain model**: Update `docs/DOMAIN.md`
- **CLI commands**: Update `docs/CLI.md`
- **Setup process**: Update `v2/README.md`

---

## Additional Guidelines

### File Organization

**Keep files focused and small**:
- Max ~500 lines per file (guideline, not hard limit)
- One main concept per file
- Use modules to organize related functionality

### Dependencies

**Be conservative with dependencies**:
- Prefer std library when possible
- Evaluate necessity before adding new crate
- Check license compatibility (prefer MIT/Apache-2.0)
- Audit security (`cargo audit`)

**Adding a dependency**:
```toml
# Add to workspace Cargo.toml [workspace.dependencies]
new-crate = "1.0"

# Then in crate Cargo.toml
[dependencies]
new-crate = { workspace = true }
```

### Performance

**Optimize only when necessary**:
- Profile before optimizing (`cargo flamegraph`)
- Write clear code first, optimize later
- Add benchmarks for critical paths (`criterion`)

---

## Getting Help

### Resources

- **Documentation**: `docs/`
- **CLAUDE.md**: Repository context for AI assistants
- **GitHub Issues**: Bug reports, feature requests
- **Discussions**: General questions, ideas

### Questions?

If you're unsure about:
- Architecture decisions: Check `docs/ARCHITECTURE.md`
- Domain model: Check `docs/DOMAIN.md`
- Code patterns: Check `v2/CLAUDE.md`
- Still stuck: Open a GitHub Discussion

---

## License

By contributing to Robson v2, you agree that your contributions will be licensed under the same license as the project (check root LICENSE file).

---

**Thank you for contributing to Robson v2!**

Your contributions help build a reliable, safe, and efficient trading platform.

---

**Version**: 1.0
**Last Updated**: 2026-01-15
