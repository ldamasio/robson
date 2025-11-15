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

New backend code follows **Ports & Adapters** pattern:

```
core/
├── domain/        # Pure entities (NO Django)
├── application/   # Use cases + port definitions
├── adapters/      # Concrete implementations
└── wiring/        # Dependency injection
```

**Rule**: `core/domain/` and `core/application/` have **zero framework dependencies**.

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

## Project Structure

```
robson/
├── apps/
│   ├── backend/
│   │   ├── core/                    # ⭐ Hexagonal architecture
│   │   │   ├── domain/              # Entities (NO Django)
│   │   │   ├── application/         # Use cases + ports
│   │   │   ├── adapters/            # Implementations
│   │   │   └── wiring/              # DI container
│   │   └── monolith/                # Legacy Django (migrating)
│   │       └── api/
│   │           ├── models/          # Django models
│   │           ├── views/           # REST endpoints
│   │           ├── services/        # Business logic
│   │           └── tests/           # Tests
│   └── frontend/                    # React 18
│       └── src/
│           ├── domain/              # Types
│           ├── ports/               # Interfaces
│           ├── adapters/            # HTTP/WS clients
│           └── components/          # React components
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
- Domain entities: `apps/backend/core/domain/`
- Use cases: `apps/backend/core/application/`
- Django models: `apps/backend/monolith/api/models/`
- React components: `apps/frontend/src/components/`
- Tests: `apps/backend/monolith/api/tests/`

---

## Common Patterns

### Adding a Use Case

```python
# 1. Define port (apps/backend/core/application/ports.py)
class MyRepository(Protocol):
    def save(self, entity: MyEntity) -> MyEntity: ...

# 2. Implement use case (apps/backend/core/application/my_use_case.py)
class MyUseCase:
    def __init__(self, repo: MyRepository):
        self._repo = repo

    def execute(self, command: MyCommand) -> MyEntity:
        # Validation
        # Business logic
        # Persistence
        # Event publishing
        pass

# 3. Implement adapter (apps/backend/core/adapters/driven/persistence/)
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
from apps.backend.core.domain.trade import Order, Symbol

def test_order_total_value():
    """Test order total value calculation."""
    order = Order(
        id="123",
        symbol=Symbol("BTCUSDT"),
        quantity=Decimal("0.5"),
        price=Decimal("50000"),
        status=OrderStatus.PENDING,
        created_at=datetime.now(),
    )

    assert order.total_value == Decimal("25000")
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

---

## File Path Patterns

| Task | Path |
|------|------|
| Domain entity | `apps/backend/core/domain/*.py` |
| Use case | `apps/backend/core/application/*.py` |
| Port | `apps/backend/core/application/ports.py` |
| Adapter | `apps/backend/core/adapters/driven/*/*.py` |
| Django model | `apps/backend/monolith/api/models/*.py` |
| Django view | `apps/backend/monolith/api/views/*.py` |
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

## Code Quality Checklist

When generating code, ensure:

- [ ] English only (no Portuguese)
- [ ] Type hints for Python
- [ ] PropTypes for React
- [ ] Docstrings for public functions/classes
- [ ] Tests written
- [ ] Follows hexagonal architecture
- [ ] No Django in `core/domain/` or `core/application/`
- [ ] Conventional commit message
- [ ] Updated OpenAPI spec (if endpoint changed)

---

## Common Commands

```bash
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

**Last Updated**: 2025-11-14
**Repository**: C:\app\robson
**Version**: 1.0
