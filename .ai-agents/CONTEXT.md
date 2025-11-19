# Project Context for AI Agents

## Project: ROBSON BOT
Open-source cryptocurrency trading platform

## Repository
- **GitHub**: ldamasio/robson
- **Language**: Python 3.12
- **License**: Open Source

---

## Architecture

### Fundamental Principles
- **Hexagonal Architecture (Ports & Adapters)**: Framework-independent core
- **Multi-Tenant**: Isolated trading environments for multiple users
- **Domain-Driven Design**: Rich domain model
- **Test-Driven**: Minimum 80% coverage
- **API-First**: RESTful + WebSocket

### Technology Stack

#### Backend
- **Python**: 3.12
- **Framework**: Django 5.2, Django REST Framework 3.14
- **Database**: PostgreSQL (primary), Redis (cache)
- **Exchange Integration**: python-binance 1.0.16
- **Data Analysis**: pandas 2.1, numpy 1.26
- **Server**: gunicorn 20.1 + gevent 24.11
- **Testing**: pytest, pytest-django
- **Type Checking**: mypy (strict mode)

#### Frontend
- **Framework**: React 18.2
- **Build Tool**: Vite 4.5
- **Testing**: Vitest 1.6
- **HTTP Client**: Axios 1.1

#### Infrastructure
- **Container**: Docker
- **Orchestration**: Kubernetes (k3s)
- **Service Mesh**: Istio Ambient
- **GitOps**: ArgoCD
- **Provisioning**: Ansible
- **Monitoring**: Prometheus + Grafana

---

## Code Patterns (MANDATORY)

### Type Hints (100% Coverage)

```python
# ✅ ALWAYS specify types
from typing import Protocol, Optional
from decimal import Decimal
from datetime import datetime

async def calculate_risk(
    portfolio: Portfolio,
    order: Order,
    confidence_level: Decimal = Decimal("0.95")
) -> RiskMetrics:
    """Calculate risk metrics for order."""
    pass

# ✅ Use Protocol for ports
class OrderRepository(Protocol):
    def save(self, order: Order) -> Order: ...
    def find_by_id(self, order_id: str, tenant_id: str) -> Optional[Order]: ...

# ❌ NEVER omit types
def calculate_risk(portfolio, order):
    pass
```

### Docstrings (Google Style - MANDATORY)

```python
def calculate_var(
    portfolio: Portfolio,
    confidence_level: Decimal = Decimal("0.95")
) -> Decimal:
    """Calculate Value at Risk for portfolio.

    Uses historical simulation method to calculate VaR at specified
    confidence level.

    Args:
        portfolio: Current portfolio with positions and values
        confidence_level: Confidence level for VaR (0.95 = 95%)

    Returns:
        Value at Risk in USD (absolute value). For example, Decimal("5000.0")
        means there is (1 - confidence_level) probability of losing
        more than $5000.

    Raises:
        ValueError: If portfolio is empty or confidence_level not in (0, 1)
        InsufficientDataError: If market data has fewer than 252 points

    Example:
        >>> portfolio = Portfolio(tenant_id="t1", positions=[...])
        >>> var = calculate_var(portfolio, Decimal("0.95"))
        >>> print(f"95% VaR: ${var}")
        95% VaR: $5432.10
    """
    pass
```

### Hexagonal Architecture (CRITICAL)

```python
# ✅ DOMAIN LAYER (apps/backend/core/domain/)
# Pure Python, NO framework dependencies
from dataclasses import dataclass
from decimal import Decimal
from datetime import datetime

@dataclass(frozen=True)
class Order:
    """Order domain entity."""
    id: str
    tenant_id: str
    symbol: str
    side: str  # "BUY" or "SELL"
    quantity: Decimal
    price: Decimal
    status: str
    created_at: datetime

    def total_value(self) -> Decimal:
        """Calculate total order value."""
        return self.quantity * self.price

# ✅ APPLICATION LAYER (apps/backend/core/application/)
# Use cases + port definitions
from typing import Protocol

class OrderRepository(Protocol):
    """Port for order persistence."""
    def save(self, order: Order) -> Order: ...
    def find_by_id(self, order_id: str, tenant_id: str) -> Optional[Order]: ...

class PlaceOrderUseCase:
    """Use case for placing orders."""

    def __init__(
        self,
        order_repo: OrderRepository,
        exchange_client: ExchangeClient,
        risk_calculator: RiskCalculator
    ):
        self._order_repo = order_repo
        self._exchange = exchange_client
        self._risk = risk_calculator

    def execute(self, command: PlaceOrderCommand) -> Order:
        # 1. Validate
        # 2. Calculate risk
        # 3. Execute on exchange
        # 4. Persist
        # 5. Return result
        pass

# ✅ ADAPTER LAYER (apps/backend/core/adapters/driven/)
# Implementations of ports
class DjangoOrderRepository:
    """Django implementation of OrderRepository port."""

    def save(self, order: Order) -> Order:
        # Django ORM operations
        django_order = OrderModel.objects.create(
            id=order.id,
            tenant_id=order.tenant_id,
            # ... map domain to Django model
        )
        return self._to_domain(django_order)

# ❌ NEVER import Django in domain or application layers
# BAD: from django.db import models  # In core/domain/
```

### Multi-Tenant Isolation (MAXIMUM PRIORITY)

**Golden Rule**: NEVER access data without filtering by `tenant_id`

```python
# ✅ ALWAYS filter by tenant_id
def get_order(order_id: str, tenant_id: str) -> Optional[Order]:
    """Get order ensuring tenant isolation."""
    try:
        order = Order.objects.get(
            id=order_id,
            user__id=tenant_id  # CRITICAL - Django uses user for tenant
        )
        return order
    except Order.DoesNotExist:
        return None

# ✅ Validate tenant_id in ALL queries
def list_orders(tenant_id: str) -> List[Order]:
    """List all orders for tenant."""
    return Order.objects.filter(user__id=tenant_id).all()

# ✅ Verify tenant_id in updates/deletes
def update_order(
    order_id: str,
    tenant_id: str,
    updates: dict
) -> Order:
    """Update order ensuring tenant isolation."""
    order = Order.objects.filter(
        id=order_id,
        user__id=tenant_id  # CRITICAL
    ).first()

    if not order:
        raise TenantIsolationError(
            f"Order {order_id} not found for tenant {tenant_id}"
        )

    for key, value in updates.items():
        setattr(order, key, value)
    order.save()
    return order

# ❌ NEVER query without tenant_id
def get_order(order_id: str) -> Order:
    # ❌ CRITICAL: can return order from another tenant!
    return Order.objects.get(id=order_id)
```

**Mandatory Isolation Test**:
```python
@pytest.mark.django_db
def test_order_isolation_between_tenants():
    """Ensure order from tenant A cannot be accessed by tenant B."""
    # Arrange
    tenant_a = User.objects.create(username="tenant_a")
    tenant_b = User.objects.create(username="tenant_b")

    order_a = Order.objects.create(
        symbol="BTCUSDT",
        user=tenant_a
    )

    # Act: Try to get order_a with tenant_b credentials
    result = get_order(order_a.id, tenant_b.id)

    # Assert
    assert result is None, "Tenant B should not access Tenant A's order"
```

### Error Handling (Explicit and Specific)

```python
# ✅ Specific exceptions
class RobsonBotError(Exception):
    """Base exception for Robson Bot."""
    pass

class TenantIsolationError(RobsonBotError):
    """Raised when tenant isolation is violated."""
    pass

class RiskLimitExceededError(RobsonBotError):
    """Raised when trade exceeds risk limits."""
    pass

class InsufficientDataError(RobsonBotError):
    """Raised when not enough data for calculation."""
    pass

# ✅ Explicit error handling
def execute_order(order: Order, user: User) -> OrderResult:
    """Execute order with risk validation."""
    try:
        # Validate tenant isolation
        portfolio = Portfolio.objects.filter(
            id=order.portfolio_id,
            user=user
        ).first()

        if not portfolio:
            raise TenantIsolationError(
                f"Portfolio {order.portfolio_id} not found for user {user.id}"
            )

        # Calculate risk
        risk = calculate_risk(portfolio, order)

        # Validate risk limits
        if risk > user.risk_limit:
            raise RiskLimitExceededError(
                f"Order risk {risk} exceeds limit {user.risk_limit}"
            )

        # Execute
        result = exchange_client.execute(order)
        logger.info(
            "Order executed successfully",
            extra={
                "order_id": order.id,
                "user_id": user.id,
                "symbol": order.symbol
            }
        )
        return result

    except TenantIsolationError:
        logger.critical(
            "Tenant isolation violation",
            extra={"order_id": order.id, "user_id": user.id}
        )
        raise

    except RiskLimitExceededError as e:
        logger.warning(str(e), extra={"order_id": order.id})
        raise

    except Exception as e:
        logger.exception(
            "Unexpected error in order execution",
            extra={"order_id": order.id, "user_id": user.id}
        )
        raise

# ❌ NEVER catch generic without re-raise
def execute_order(order: Order) -> OrderResult:
    try:
        # ...
    except Exception:  # ❌ Too generic, hides errors
        pass
```

### Logging (Structured)

```python
import logging
import structlog

logger = structlog.get_logger(__name__)

# ✅ Structured logging with context
def process_signal(signal: Signal, user_id: str) -> ProcessedSignal:
    """Process trading signal."""
    logger.info(
        "Processing signal",
        extra={
            "signal_id": signal.id,
            "user_id": user_id,
            "symbol": signal.symbol,
            "signal_type": signal.type
        }
    )

    try:
        validated = validate_signal(signal, user_id)
        enriched = enrich_signal(validated)

        logger.info(
            "Signal processed successfully",
            extra={
                "signal_id": signal.id,
                "confidence": enriched.confidence
            }
        )

        return enriched

    except ValidationError as e:
        logger.error(
            "Signal validation failed",
            extra={
                "signal_id": signal.id,
                "user_id": user_id,
                "error": str(e)
            }
        )
        raise

# ❌ NEVER use print() or logging without context
def process_signal(signal):
    print(f"Processing {signal.id}")  # ❌ Don't use print
    logger.info("Processing signal")  # ❌ No context
```

---

## Critical Business Rules

### 1. Multi-Tenant Isolation (MAXIMUM PRIORITY)
- ALWAYS filter queries by `user__id` (Django's user is tenant)
- Mandatory isolation tests for all queries
- Log all access with user_id context

### 2. Risk Management (HIGH PRIORITY)
- Validate limits BEFORE executing trades
- Never execute without risk calculation
- Log all risk decisions

### 3. Hexagonal Architecture (HIGH PRIORITY)
- Domain and Application layers: NO Django imports
- All framework code in Adapters layer
- Use Protocol for port definitions

---

## Testing Structure

### Coverage Minimum: 80% (Goal: 90%+)

```python
# tests/test_risk/test_calculator.py
import pytest
from decimal import Decimal
from datetime import datetime

@pytest.mark.django_db
def test_calculate_var_with_valid_portfolio():
    """Test VaR calculation with valid portfolio."""
    # Arrange
    user = User.objects.create(username="trader1")
    portfolio = Portfolio.objects.create(
        user=user,
        total_value=Decimal("100000")
    )

    # Act
    var = calculate_var(portfolio, Decimal("0.95"))

    # Assert
    assert var > 0
    assert var < portfolio.total_value
    assert isinstance(var, Decimal)

@pytest.mark.django_db
def test_tenant_isolation_in_portfolio_fetch():
    """Test that tenant B cannot access tenant A's portfolio."""
    # Arrange
    user_a = User.objects.create(username="tenant_a")
    user_b = User.objects.create(username="tenant_b")

    portfolio_a = Portfolio.objects.create(user=user_a)

    # Act
    result = get_portfolio(portfolio_a.id, user_b.id)

    # Assert
    assert result is None
```

---

## File Structure

```
robson/
├── apps/
│   ├── backend/
│   │   ├── core/                         # Hexagonal core
│   │   │   ├── domain/                   # Entities (NO Django)
│   │   │   │   ├── trade.py
│   │   │   │   └── __init__.py
│   │   │   ├── application/              # Use cases + ports
│   │   │   │   ├── place_order.py
│   │   │   │   ├── ports.py
│   │   │   │   └── __init__.py
│   │   │   ├── adapters/                 # Implementations
│   │   │   │   ├── driven/
│   │   │   │   │   ├── persistence/
│   │   │   │   │   ├── external/
│   │   │   │   │   └── messaging/
│   │   │   │   └── driving/
│   │   │   └── wiring/                   # DI container
│   │   └── monolith/                     # Django app
│   │       └── api/
│   │           ├── models/               # Django models
│   │           ├── views/                # REST endpoints
│   │           ├── serializers/
│   │           ├── services/
│   │           └── tests/
│   └── frontend/
│       └── src/
├── docs/
│   ├── requirements/                     # Business requirements
│   ├── specs/                            # Technical specs
│   ├── adr/                              # Architecture decisions
│   └── plan/                             # Weekly plans
└── .ai-agents/                           # AI governance
    ├── MODES.md
    ├── CONTEXT.md (this file)
    └── ADR-0001-ai-governance.md
```

---

## Naming Conventions

### Files and Modules
- `snake_case.py` for Python files
- `src/[domain]/[module].py` for modules
- `tests/test_[module].py` for tests

### Classes
- `PascalCase` for classes
- Suffixes:
  - `*UseCase`: For use cases (e.g., `PlaceOrderUseCase`)
  - `*Repository`: For data access ports (e.g., `OrderRepository`)
  - `*Adapter`: For implementations (e.g., `BinanceAdapter`)
  - `*Service`: For business logic (e.g., `RiskService`)

### Functions
- `snake_case` for functions
- Descriptive verbs:
  - `calculate_*`: For calculations
  - `fetch_*`, `get_*`: For retrieving data
  - `process_*`: For processing data
  - `validate_*`: For validations
  - `execute_*`: For executing actions

---

## Performance Targets

- **API Response Time**: <200ms (p95)
- **Order Execution**: <500ms end-to-end
- **Database Queries**: <50ms (p95)
- **Test Suite**: <5 min for full suite
- **Test Coverage**: >80% (target: >90%)

---

## Security

### Authentication & Authorization
- JWT tokens with 15min expiration
- Refresh token strategy (7 days)
- Rate limiting per user (100 req/min)
- All endpoints require authentication (except /auth/login)

### Data Privacy
- ALWAYS filter by user (multi-tenant)
- Encrypt sensitive data at rest
- TLS 1.3 for transport
- No secrets in code (use environment variables)

---

## Current Project State

**Last updated**: 2024-11-16

**Phase**: Development

**Modules**:
- `apps/backend/core/domain/`: Domain entities
- `apps/backend/core/application/`: Use cases
- `apps/backend/monolith/api/`: Django REST API
- `apps/frontend/`: React dashboard

---

## Sensitive Areas (EXTRA CARE)

### `apps/backend/core/domain/` - Domain Layer 🔴
- **Criticality**: CRITICAL
- **Reason**: Core business logic, must be framework-independent
- **Rules**:
  - NO Django imports
  - Pure Python only
  - 100% test coverage
  - Type hints mandatory

### `apps/backend/monolith/api/models/` - Django Models 🔴
- **Criticality**: CRITICAL
- **Reason**: Data integrity, multi-tenant isolation
- **Rules**:
  - ALWAYS filter by user
  - Mandatory isolation tests
  - Migration review required

### `apps/backend/core/adapters/driven/external/` - Exchange APIs 🟠
- **Criticality**: HIGH
- **Reason**: Real money, external systems
- **Rules**:
  - Rate limiting mandatory
  - Robust error handling
  - Timeout on all requests
  - Mock in tests (never call real API)

---

## For Interactive Mode (Cursor Chat, Codex)

When working in interactive mode:
- Ask questions when there's ambiguity
- Suggest alternatives for architectural decisions
- Point out potential security/performance issues
- Explain trade-offs of different approaches
- Validate with user before important decisions

## For Autonomous Mode (Agents, Claude CLI)

When working in autonomous mode:
- Follow specs literally
- Apply ALL patterns from this CONTEXT.md
- Generate tests for all new code
- Log unexpected errors (don't fail silently)
- If spec has critical ambiguity, use conservative approach

---

## References

### For Comprehensive Context
- **[docs/AGENTS.md](../docs/AGENTS.md)** - Complete AI guide with architecture
- **[.cursorrules](../.cursorrules)** - Hexagonal architecture rules
- **[CLAUDE.md](../CLAUDE.md)** - Quick reference

### For Mode-First Governance
- **[MODES.md](MODES.md)** - Interactive vs autonomous modes
- **[DECISION-MATRIX.md](DECISION-MATRIX.md)** - Mode selection guide
- **[AI-WORKFLOW.md](AI-WORKFLOW.md)** - Workflows by mode

---

**Last Revision**: 2024-11-16
**Repository**: C:\app\robson
**Version**: 1.0
