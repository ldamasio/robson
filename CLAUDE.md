# Claude Code Context: Robson Bot

**Optimized context for Claude Code AI assistant.**

This document provides Claude Code with essential context for effective code generation, refactoring, and problem-solving in the Robson Bot project.

---

## Quick Context

**Project**: Robson Bot - Open-source cryptocurrency trading platform
**Architecture**: Hexagonal (Ports & Adapters)
**Backend**: Django 5.2 + Python 3.12
**Frontend**: React 18 + Vite
**Deployment**: Kubernetes (k3s) + GitOps (ArgoCD)
**Language Policy**: **100% English** (code, comments, docs)

---

## Critical Rules

### 1. English Only

**ALL code, comments, documentation, and commit messages MUST be in English.**

No exceptions. See [docs/LANGUAGE-POLICY.md](docs/LANGUAGE-POLICY.md).

### 2. Hexagonal Architecture

Backend code follows **Ports & Adapters** pattern **INSIDE** Django monolith:

```
api/application/
├── domain.py        # Pure entities (NO Django deps)
├── ports.py         # Port definitions (Protocol interfaces)
├── use_cases.py     # Business logic (use cases)
├── adapters.py      # Concrete implementations
├── wiring.py        # Dependency injection
├── validation.py    # Validation framework (PLAN → VALIDATE step)
└── execution.py     # Execution framework (EXECUTE step, SAFE BY DEFAULT)
```

**Rule**: `api/application/domain.py`, `ports.py`, and `use_cases.py` have **zero Django dependencies**. Only `adapters.py` imports Django.

### 3. Type Hints Required

Always use type hints:

```python
from typing import Protocol
from decimal import Decimal

class OrderRepository(Protocol):
    def save(self, order: Order) -> Order: ...
```

### 4. Test-Driven Development

Write tests **before** implementation when possible:

```python
# 1. Write test
def test_place_order_success():
    # Arrange, Act, Assert
    pass

# 2. Implement to pass test
```

### 5. Conventional Commits

```
<type>(<scope>): <subject>

<body>

<footer>
```

Types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

---

## **CRITICAL: Semantic Clarity - Strategy & Robson's Intelligence**

### Robson is a Risk Management Assistant, NOT an Auto-Trader

**Core Principle**: USER initiates → ROBSON calculates → USER confirms

**What Robson IS**:
- ✅ Position sizing calculator (1% risk rule)
- ✅ Risk limit validator (drawdown, exposure)
- ✅ Stop-loss monitor (24/7 automation for safety)
- ✅ Performance tracker (analytics by strategy)

**What Robson is NOT**:
- ❌ Autonomous trading system
- ❌ Signal generator (user decides when to trade)
- ❌ Auto-trader (no trades without user confirmation)

### Strategy = User's Choice

**Definition**: Strategy is the **trading approach selected by the USER**.

Examples: "Mean Reversion MA99", "Breakout Consolidation", "Manual Analysis"

**NOT**: System-generated trading algorithm

**Database Model**:
```python
class Strategy:
    name: str           # User-chosen name
    description: str    # User's documented plan
    config: dict        # Reference settings (NOT automation logic)
    risk_config: dict   # Risk parameters
```

### Robson's Primary Intelligence: Position Sizing

## ⚠️ GOLDEN RULE: Position Size is DERIVED from Technical Stop

**CRITICAL**: The position size is NEVER arbitrary. It is ALWAYS calculated
backwards from the technical stop-loss level.

**THE ORDER OF OPERATIONS**:
1. **FIRST**: Identify technical stop (2nd support level on chart)
2. **THEN**: Calculate stop distance = |Entry - Technical Stop|
3. **THEN**: Max Risk = Capital × 1%
4. **FINALLY**: Position Size = Max Risk / Stop Distance

**The Formula**:
```
Position Size = (Capital × 1%) / |Entry Price - Technical Stop|
```

**Example with TECHNICAL Stop**:
```python
Capital: $10,000
Entry Price: $95,000
Technical Stop: $93,500  # 2nd support level on 15m chart (FROM CHART!)

Stop Distance = |$95,000 - $93,500| = $1,500
Max Risk (1%) = $10,000 × 0.01 = $100
Position Size = $100 / $1,500 = 0.0667 BTC
Position Value = 0.0667 × $95,000 = $6,333.33

If stopped at $93,500: Loss = 0.0667 × $1,500 = $100 = 1% ✓
```

**Key Insight**:
- Wide technical stop → Smaller position size
- Tight technical stop → Larger position size
- Risk amount stays CONSTANT at 1%

**For AI Agents**:
- ❌ NEVER ask "how much do you want to invest?"
- ✅ ALWAYS ask "where is your technical invalidation level?"
- The investment amount is CALCULATED, not chosen
- The stop-loss comes from CHART ANALYSIS, not arbitrary percentage

**Services**:
- `apps/backend/core/domain/technical_stop.py` - Technical stop calculator
- `api/application/technical_stop_adapter.py` - Binance integration
- `api/management/commands/technical_stop_buy.py` - CLI command

**See**: `docs/requirements/POSITION-SIZING-GOLDEN-RULE.md`

### User-Initiated Flow

1. **User provides intent**:
   - Symbol, side (BUY/SELL)
   - Entry price, stop price
   - Strategy choice (from dropdown)

2. **Robson calculates** (THE INTELLIGENCE):
   - Optimal position size (1% risk)
   - Validates exposure limits
   - Checks monthly drawdown

3. **User reviews**:
   - Sees calculated quantity
   - Reviews risk amount
   - Confirms or cancels

4. **Robson executes** (if confirmed):
   - Places order on exchange
   - Activates stop monitor
   - Records audit trail

**CLI Command**: `python manage.py create_user_operation`
**API Endpoints**:
- `POST /api/operations/calculate-size/` (preview)
- `POST /api/operations/create/` (create & execute)

See: **ADR-0007**, **STRATEGY-SEMANTIC-CLARITY.md**

---

## Project Structure

```
robson/
├── apps/
│   ├── backend/
│   │   └── monolith/                # Django monolith
│   │       └── api/
│   │           ├── application/     # ⭐ Hexagonal core (INSIDE Django)
│   │           │   ├── domain.py    # Entities (NO Django deps)
│   │           │   ├── ports.py     # Interface definitions
│   │           │   ├── use_cases.py # Business logic
│   │           │   ├── adapters.py  # Implementations
│   │           │   ├── wiring.py    # DI container
│   │           │   ├── validation.py    # ⭐ Validation framework
│   │           │   └── execution.py     # ⭐ Execution framework
│   │           ├── models/          # Django models
│   │           ├── views/           # REST endpoints
│   │           ├── management/      # Django commands
│   │           │   └── commands/
│   │           │       ├── validate_plan.py  # ⭐ Validation command
│   │           │       ├── execute_plan.py   # ⭐ Execution command
│   │           │       └── monitor_stops.py  # ⭐ Stop monitor command
│   │           └── tests/           # Tests
│   └── frontend/                    # React 18
│       └── src/
│           ├── domain/              # Types
│           ├── ports/               # Interfaces
│           ├── adapters/            # HTTP/WS clients
│           └── components/          # React components
├── cli/                             # ⭐ Go-based CLI (robson-go)
│   ├── main.go                      # Entry point
│   ├── cmd/
│   │   ├── root.go                  # Root command
│   │   ├── legacy.go                # Legacy commands
│   │   ├── agentic.go               # ⭐ PLAN → VALIDATE → EXECUTE
│   │   ├── margin.go                # ⭐ Margin trading commands
│   │   └── monitoring.go            # Portfolio monitoring
│   └── go.mod                       # Go dependencies
├── main.c                           # ⭐ C router (thin wrapper)
├── docs/                            # ⭐ Comprehensive docs
│   ├── AGENTS.md                    # Full AI guide
│   ├── INDEX.md                     # Navigation hub
│   ├── adr/                         # Architecture decisions
│   └── specs/                       # Feature specifications
└── infra/                           # Infrastructure as Code
    ├── ansible/                     # Node provisioning
    ├── k8s/                         # Kubernetes manifests
    └── charts/                      # Helm charts
```

**Key Paths**:
- Domain entities: `apps/backend/monolith/api/application/domain.py`
- Use cases: `apps/backend/monolith/api/application/use_cases.py`
- Ports: `apps/backend/monolith/api/application/ports.py`
- Adapters: `apps/backend/monolith/api/application/adapters.py`
- Validation framework: `apps/backend/monolith/api/application/validation.py`
- Execution framework: `apps/backend/monolith/api/application/execution.py`
- Django models: `apps/backend/monolith/api/models/`
- Django views: `apps/backend/monolith/api/views/`
- Django commands: `apps/backend/monolith/api/management/commands/`
- CLI (Go): `cli/cmd/*.go`
- CLI (C router): `main.c`
- React components: `apps/frontend/src/components/`
- Tests: `apps/backend/monolith/api/tests/`

---

## Common Patterns

### Adding a Use Case

```python
# 1. Define port (apps/backend/monolith/api/application/ports.py)
class MyRepository(Protocol):
    def save(self, entity: MyEntity) -> MyEntity: ...

# 2. Implement use case (apps/backend/monolith/api/application/use_cases.py)
class MyUseCase:
    def __init__(self, repo: MyRepository):
        self._repo = repo

    def execute(self, command: MyCommand) -> MyEntity:
        # Validation
        # Business logic
        # Persistence
        # Event publishing
        pass

# 3. Implement adapter (apps/backend/monolith/api/application/adapters.py)
class DjangoMyRepository:
    def save(self, entity: MyEntity) -> MyEntity:
        # Django ORM operations
        pass

# 4. Write tests (apps/backend/monolith/api/tests/test_my_use_case.py)
@pytest.mark.django_db
def test_my_use_case_success():
    # Arrange
    repo = DjangoMyRepository()
    use_case = MyUseCase(repo)
    command = MyCommand(...)

    # Act
    result = use_case.execute(command)

    # Assert
    assert result.id is not None
```

### Adding a REST Endpoint

```python
# apps/backend/monolith/api/views/my_views.py
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated
from rest_framework.response import Response

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def my_endpoint(request):
    """
    Endpoint description.

    Request body:
        field1 (str): Description
        field2 (int): Description

    Returns:
        Response with created entity
    """
    # 1. Validate input
    serializer = MySerializer(data=request.data)
    serializer.is_valid(raise_exception=True)

    # 2. Call use case (from core)
    command = MyCommand(**serializer.validated_data)
    result = my_use_case.execute(command)

    # 3. Serialize response
    response_serializer = MyResponseSerializer(result)
    return Response(response_serializer.data, status=201)
```

### Adding a React Component

```javascript
// apps/frontend/src/components/MyComponent.jsx
import React, { useState, useEffect } from 'react';
import PropTypes from 'prop-types';

/**
 * Component description.
 *
 * @param {Object} props - Component props
 * @param {string} props.title - Title to display
 * @param {Function} props.onAction - Callback for action
 */
const MyComponent = ({ title, onAction }) => {
  const [data, setData] = useState(null);

  useEffect(() => {
    // Fetch data
  }, []);

  const handleClick = () => {
    onAction(data);
  };

  return (
    <div className="my-component">
      <h2>{title}</h2>
      <button onClick={handleClick}>Action</button>
    </div>
  );
};

MyComponent.propTypes = {
  title: PropTypes.string.isRequired,
  onAction: PropTypes.func.isRequired,
};

export default MyComponent;
```

---

## Testing Patterns

### Unit Test (Domain)

```python
import pytest
from decimal import Decimal
from api.application import Symbol

def test_symbol_as_pair():
    """Test symbol pair formatting."""
    symbol = Symbol.from_pair("BTCUSDT")

    assert symbol.base == "BTC"
    assert symbol.quote == "USDT"
    assert symbol.as_pair() == "BTCUSDT"
```

### Integration Test (Django)

```python
import pytest
from django.contrib.auth import get_user_model

@pytest.mark.django_db
def test_create_order_via_api(client, user):
    """Test creating order via API endpoint."""
    client.force_authenticate(user=user)

    data = {
        'symbol': 'BTCUSDT',
        'quantity': '0.5',
        'price': '50000',
    }

    response = client.post('/api/orders/', data)

    assert response.status_code == 201
    assert response.data['symbol'] == 'BTCUSDT'
```

### Frontend Test (Vitest)

```javascript
import { render, screen, fireEvent } from '@testing-library/react';
import { describe, it, expect, vi } from 'vitest';
import MyComponent from './MyComponent';

describe('MyComponent', () => {
  it('renders title', () => {
    render(<MyComponent title="Test" onAction={vi.fn()} />);
    expect(screen.getByText('Test')).toBeInTheDocument();
  });

  it('calls onAction when button clicked', () => {
    const mockAction = vi.fn();
    render(<MyComponent title="Test" onAction={mockAction} />);

    fireEvent.click(screen.getByRole('button'));
    expect(mockAction).toHaveBeenCalled();
  });
});
```

---

## Domain Glossary

**Trading Terms**:
- **Order**: Buy/sell instruction
- **Position**: Currently held asset
- **Strategy**: Trading algorithm
- **Signal**: Generated recommendation
- **Symbol**: Trading pair (BTCUSDT)
- **Stop-Loss**: Auto-sell at loss threshold
- **Take-Profit**: Auto-sell at profit target

**Architecture Terms**:
- **Port**: Interface definition
- **Adapter**: Implementation of port
- **Entity**: Business object with identity
- **Value Object**: Immutable object
- **Use Case**: Single business operation
- **Repository**: Data access abstraction

**Agentic Workflow Terms**:
- **PLAN**: Create execution plan (no real orders, just blueprint)
- **VALIDATE**: Paper trading stage - check operational/financial constraints
- **EXECUTE**: Final step - DRY-RUN (simulation) or LIVE (real orders)
- **DRY-RUN**: Default execution mode - simulation, no real orders
- **LIVE**: Real execution mode - requires `--live` AND `--acknowledge-risk`
- **Guard**: Safety check that can PASS or FAIL (blocks execution if failed)
- **Validation Report**: Result of validation with PASS/FAIL/WARNING status
- **Execution Result**: Result of execution with guards, actions, audit trail

---

## File Path Patterns

| Task | Path |
|------|------|
| Domain entity | `apps/backend/monolith/api/application/domain.py` |
| Use case | `apps/backend/monolith/api/application/use_cases.py` |
| Port | `apps/backend/monolith/api/application/ports.py` |
| Adapter | `apps/backend/monolith/api/application/adapters.py` |
| Validation framework | `apps/backend/monolith/api/application/validation.py` |
| Execution framework | `apps/backend/monolith/api/application/execution.py` |
| Django model | `apps/backend/monolith/api/models/*.py` |
| Django view | `apps/backend/monolith/api/views/*.py` |
| Django command | `apps/backend/monolith/api/management/commands/*.py` |
| CLI command (Go) | `cli/cmd/*.go` |
| CLI router (C) | `main.c` |
| React component | `apps/frontend/src/components/*/*.jsx` |
| Test (backend) | `apps/backend/monolith/api/tests/test_*.py` |
| Test (frontend) | `apps/frontend/tests/*.test.js` |
| ADR | `docs/adr/ADR-XXXX-*.md` |

---

## Key Dependencies

**Backend**:
- Django 5.2, DRF 3.14 (web framework, API)
- python-binance 1.0.16 (exchange integration)
- pandas 2.1, numpy 1.26 (data analysis)
- gunicorn 20.1 + gevent 24.11 (async server)

**Frontend**:
- React 18.2, Vite 4.5 (UI, build tool)
- Vitest 1.6 (testing)
- Axios 1.1 (HTTP client)

**Infrastructure**:
- k3s (Kubernetes), Istio Ambient (service mesh)
- ArgoCD (GitOps), cert-manager (TLS)
- Ansible (node provisioning)
- K9s (terminal UI for cluster operations, read-mostly debugging)
- ktop (top-style Kubernetes resource monitoring)

---

## Architecture Decisions (ADRs)

Read full context in `docs/adr/`:

1. **ADR-0001**: Binance Service Singleton (rate limit handling)
2. **ADR-0002**: Hexagonal Architecture (framework independence)
3. **ADR-0003**: Istio Ambient + Gateway API (sidecarless mesh)
4. **ADR-0004**: GitOps Preview Environments (per-branch testing)
5. **ADR-0005**: Ansible Bootstrap (automated hardening)
6. **ADR-0006**: English-Only Codebase (international positioning)

---

## When to Reference Full Documentation

For comprehensive context:
- **[docs/AGENTS.md](docs/AGENTS.md)** - Complete AI guide
- **[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)** - Architecture overview
- **[docs/DEVELOPER.md](docs/DEVELOPER.md)** - Development workflow
- **[docs/INDEX.md](docs/INDEX.md)** - Navigation hub

---

## Development Workflow

```bash
# 1. Create feature branch
git checkout -b feature/my-feature

# 2. Implement + test
# (see patterns above)

# 3. Run tests locally
cd apps/backend/monolith
python manage.py test -v 2

cd apps/frontend
npm test

# 4. Commit (Conventional Commits)
git commit -m "feat(trading): add stop-loss orders

Implement automatic stop-loss order execution.

Closes #123"

# 5. Push (triggers CI + preview environment)
git push origin feature/my-feature

# 6. Create PR
gh pr create --title "feat: stop-loss orders"

# 7. Preview available at:
# https://h-feature-my-feature.preview.robsonbot.com
```

---

## GitOps Auto-Deploy (NEW - December 2024)

**Zero-touch production deployments!**

Push to `main` → GitHub Actions builds images → Updates manifests → ArgoCD syncs → Production live.

```
Push to main → Build (sha-XXXXXX) → Update manifests → ArgoCD sync → ✅ Live
```

**No manual `kubectl rollout restart` required!**

See: [ADR-0011: GitOps Automatic Manifest Updates](docs/adr/ADR-0011-gitops-automatic-manifest-updates.md)

---

## Audit Trail (NEW - December 2024)

All financial movements are recorded in `AuditTransaction` for complete transparency.

### Transaction Hierarchy

```
STRATEGY (e.g., "Mean Reversion MA99")
    └── OPERATION (e.g., "3x LONG BTCUSDC")
        └── MOVEMENT (atomic, auditable)
            - SPOT_BUY, MARGIN_BUY
            - TRANSFER_SPOT_TO_ISOLATED
            - MARGIN_BORROW, MARGIN_REPAY
            - STOP_LOSS_PLACED, STOP_LOSS_TRIGGERED
            - TRADING_FEE, INTEREST_CHARGED
```

**Key Models**:
- `api.models.audit.AuditTransaction` - Every financial movement
- `api.models.audit.BalanceSnapshot` - Periodic balance snapshots

**CLI Command**:
```bash
python manage.py operations  # Show operations with movements
```

See: [docs/architecture/TRANSACTION-HIERARCHY.md](docs/architecture/TRANSACTION-HIERARCHY.md)

---

## Code Quality Checklist

When generating code, ensure:

- [ ] English only (no Portuguese)
- [ ] Type hints for Python
- [ ] PropTypes for React
- [ ] Docstrings for public functions/classes
- [ ] Tests written
- [ ] Follows hexagonal architecture
- [ ] No Django dependencies in `api/application/domain.py`, `ports.py`, or `use_cases.py`
- [ ] Only `api/application/adapters.py` imports Django
- [ ] Conventional commit message
- [ ] Updated OpenAPI spec (if endpoint changed)

---

## Common Commands

```bash
# CLI Build & Install
make build-cli                       # Build C router + Go CLI
make test-cli                        # Run CLI smoke tests
make install-cli                     # Install to system PATH
make clean-cli                       # Remove built binaries

# Agentic Workflow (CLI)
robson plan buy BTCUSDT 0.001        # Create execution plan
robson validate <plan-id> --client-id 1  # Validate plan
robson execute <plan-id> --client-id 1   # Execute (DRY-RUN)
robson execute <plan-id> --client-id 1 --live --acknowledge-risk  # LIVE execution

# Django Management Commands
python manage.py validate_plan --plan-id <id> --client-id 1  # Validate
python manage.py execute_plan --plan-id <id> --client-id 1   # Execute (DRY-RUN)
python manage.py execute_plan --plan-id <id> --client-id 1 --live --acknowledge-risk  # LIVE

# Stop Monitor Commands
python manage.py monitor_stops --dry-run     # Check stops without executing
python manage.py monitor_stops               # Execute stop-loss/take-profit orders
python manage.py monitor_stops --continuous  # Run continuously (loop mode)

# Backend
python manage.py test -v 2          # Run tests
python manage.py migrate             # Apply migrations
python manage.py makemigrations      # Create migration
python manage.py runserver           # Dev server
python manage.py shell               # Django shell

# Frontend
npm test                             # Run tests
npm run dev                          # Dev server
npm run build                        # Production build
npm run lint                         # Lint code

# Docker
docker-compose up -d                 # Start local stack
docker-compose logs -f backend       # View backend logs

# Kubernetes
kubectl get pods -n production       # List pods
kubectl logs -f pod/name             # View logs
argocd app sync robson-backend       # Force sync
make k9s                             # Launch K9s (terminal UI)
make k9s-ns NAMESPACE=<name>         # K9s for specific namespace
make k9s-preview BRANCH=<branch>     # K9s for preview environment
make ktop                            # Launch ktop (top-style monitor)
make ktop-ns NAMESPACE=<name>        # ktop for specific namespace
make ktop-preview BRANCH=<branch>    # ktop for preview environment

# Stop Monitor CronJob
kubectl get cronjob -n robson                              # List CronJobs
kubectl get jobs -n robson -l app=rbs-stop-monitor         # List monitor jobs
kubectl logs -n robson -l app=rbs-stop-monitor --tail=50   # View monitor logs

# Margin Trading Commands (CLI)
robson margin-status                 # Account overview (via Django → Binance)
robson margin-status --detailed      # With position details
robson margin-positions              # List margin positions
robson margin-positions --live       # With real-time prices
robson margin-positions --json       # JSON for automation
robson margin-buy --capital 100 --stop-percent 2 --leverage 3  # DRY-RUN
robson margin-buy --capital 100 --stop-percent 2 --leverage 3 --live --confirm  # LIVE

# Django Margin Commands
python manage.py status              # Account status
python manage.py status --detailed   # With position details
python manage.py positions           # List positions
python manage.py positions --live    # Real-time prices
python manage.py positions --json    # JSON output
python manage.py isolated_margin_buy --capital 100 --stop-percent 2 --leverage 3  # DRY-RUN
python manage.py isolated_margin_buy --capital 100 --leverage 3 --live --confirm  # LIVE

# Strategy Commands (NEW - Playful UX)
# See docs/STRATEGIES.md for full guide

# Seed pre-defined strategies (All In, Rescue Forces, Smooth Sailing, Bounce Back)
python manage.py seed_production_data

# Execute "All In" strategy (BUY with technical stop from 15m chart)
python manage.py technical_stop_buy --capital 100 --strategy "All In"  # DRY-RUN
python manage.py technical_stop_buy --capital 100 --strategy "All In" --live --confirm  # LIVE

# Use 4h timeframe instead of 15m
python manage.py technical_stop_buy --capital 100 --strategy "All In" --timeframe 4h --live --confirm

# Scan for "Rescue Forces" setups (MA4/MA9 crossover)
python manage.py scan_patterns --strategy "Rescue Forces" --timeframe 15m

# Audit Trail Commands
python manage.py operations          # Show operations with movements
python manage.py operations --open   # Only open operations
python manage.py operations --json   # JSON output
python manage.py sync_transactions   # Sync trades from Binance
```

---

## Helpful Context for Code Generation

### Security
- All endpoints require JWT authentication (except `/auth/login`)
- Multi-tenant: Filter queries by `user`
- Input validation with DRF serializers

### Performance
- Use `select_related()` for foreign keys
- Use `prefetch_related()` for many-to-many
- Async for I/O-bound operations (exchange API)

### Error Handling
- Domain: Raise domain exceptions
- Application: Translate to application errors
- Views: Return appropriate HTTP status codes

---

**For detailed information, see [docs/AGENTS.md](docs/AGENTS.md).**

This guide provides quick context. The full AGENTS.md has comprehensive details on architecture, testing, deployment, troubleshooting, and more.

---

**Last Updated**: 2026-01-13
**Repository**: https://github.com/ldamasio/robson
**Version**: 1.4 (Added ktop Kubernetes monitoring tool)
