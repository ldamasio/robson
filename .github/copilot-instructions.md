# GitHub Copilot Instructions: Robson Bot

## Project Overview

**Robson Bot** is an open-source cryptocurrency trading platform built with hexagonal architecture (ports & adapters pattern).

**Tech Stack**:
- Backend: Django 5.2 + Python 3.12
- Frontend: React 18 + Vite
- Database: PostgreSQL 16
- Deployment: Kubernetes (k3s) + GitOps (ArgoCD)

## Critical Rules

### 1. English Only

**ALL code, comments, and documentation MUST be in English.**

Never use Portuguese or any other language.

### 2. Hexagonal Architecture

New backend code follows **Ports & Adapters**:

```
apps/backend/core/
├── domain/        # Pure entities (NO Django)
├── application/   # Use cases + ports
├── adapters/      # Implementations
└── wiring/        # Dependency injection
```

**NEVER import Django in `core/domain/` or `core/application/`.**

### 3. Type Hints

Always use type hints in Python:

```python
from typing import Protocol

class OrderRepository(Protocol):
    def save(self, order: Order) -> Order:
        ...
```

### 4. Testing

Write tests for all new code. Target: 80% coverage.

### 5. Conventional Commits

```
feat(trading): add stop-loss orders
fix(api): prevent race condition
docs(readme): update installation steps
```

## Code Patterns

### Use Case Pattern

```python
class PlaceOrderUseCase:
    def __init__(
        self,
        order_repo: OrderRepository,
        exchange: ExchangeExecutionPort,
    ):
        self._order_repo = order_repo
        self._exchange = exchange

    def execute(self, command: PlaceOrderCommand) -> Order:
        # Validate
        # Create domain entity
        # Execute on exchange
        # Persist
        # Return result
        pass
```

### Port Definition

```python
from typing import Protocol

class MarketDataPort(Protocol):
    def get_current_price(self, symbol: Symbol) -> Price:
        ...
```

### REST Endpoint

```python
from rest_framework.decorators import api_view, permission_classes
from rest_framework.permissions import IsAuthenticated

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def place_order(request):
    """Place a new trading order."""
    # Validate
    serializer = OrderSerializer(data=request.data)
    serializer.is_valid(raise_exception=True)

    # Call use case
    command = PlaceOrderCommand(**serializer.validated_data)
    result = place_order_use_case.execute(command)

    # Return response
    return Response(OrderSerializer(result).data, status=201)
```

### React Component

```javascript
import React, { useState } from 'react';
import PropTypes from 'prop-types';

const OrderForm = ({ onSubmit }) => {
  const [quantity, setQuantity] = useState('');

  const handleSubmit = (e) => {
    e.preventDefault();
    onSubmit({ quantity: parseFloat(quantity) });
  };

  return (
    <form onSubmit={handleSubmit}>
      {/* Form fields */}
    </form>
  );
};

OrderForm.propTypes = {
  onSubmit: PropTypes.func.isRequired,
};

export default OrderForm;
```

## Naming Conventions

| Element | Convention | Example |
|---------|-----------|---------|
| Variables | snake_case | `order_id` |
| Functions | snake_case | `place_order()` |
| Classes | PascalCase | `Order` |
| Constants | UPPER_SNAKE_CASE | `MAX_POSITION_SIZE` |
| React Components | PascalCase | `OrderForm` |

## Testing Patterns

### Python Unit Test

```python
import pytest

def test_order_total_value():
    order = Order(
        symbol=Symbol("BTCUSDT"),
        quantity=Decimal("0.5"),
        price=Decimal("50000"),
    )
    assert order.total_value == Decimal("25000")
```

### Django Integration Test

```python
@pytest.mark.django_db
def test_create_order_api(client, user):
    client.force_authenticate(user=user)
    response = client.post('/api/orders/', {
        'symbol': 'BTCUSDT',
        'quantity': '0.5',
        'price': '50000',
    })
    assert response.status_code == 201
```

### React Test

```javascript
import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';

describe('OrderForm', () => {
  it('renders form', () => {
    render(<OrderForm onSubmit={vi.fn()} />);
    expect(screen.getByRole('button')).toBeInTheDocument();
  });
});
```

## File Paths

| Purpose | Path |
|---------|------|
| Domain entity | `apps/backend/core/domain/*.py` |
| Use case | `apps/backend/core/application/*.py` |
| Port | `apps/backend/core/application/ports.py` |
| Adapter | `apps/backend/core/adapters/driven/*/*.py` |
| Django model | `apps/backend/monolith/api/models/*.py` |
| Django view | `apps/backend/monolith/api/views/*.py` |
| React component | `apps/frontend/src/components/*/*.jsx` |
| Python test | `apps/backend/monolith/api/tests/test_*.py` |
| React test | `apps/frontend/tests/*.test.js` |

## Dependencies

**Backend**: Django 5.2, DRF 3.14, python-binance 1.0.16, pandas 2.1, gunicorn 20.1

**Frontend**: React 18.2, Vite 4.5, Vitest 1.6, Axios 1.1

## Domain Glossary

- **Order**: Buy/sell instruction
- **Position**: Currently held asset
- **Strategy**: Trading algorithm
- **Signal**: Generated recommendation
- **Symbol**: Trading pair (e.g., BTCUSDT)
- **Port**: Interface definition
- **Adapter**: Implementation of port

## Security

- All endpoints require JWT authentication
- Multi-tenant: Always filter by `user`
- Input validation with DRF serializers
- Use `@permission_classes([IsAuthenticated])`

## Performance

- Use `select_related()` for foreign keys
- Use `prefetch_related()` for many-to-many
- Async for I/O-bound operations
- Memoize expensive React calculations

## Documentation

For comprehensive context, see:
- `docs/AGENTS.md` - Complete guide
- `CLAUDE.md` - Quick reference
- `docs/ARCHITECTURE.md` - Architecture
- `docs/DEVELOPER.md` - Workflow

## Git Workflow

1. Create branch: `feature/my-feature`
2. Implement + test
3. Commit: `feat(scope): description`
4. Push (triggers CI + preview env)
5. Create PR
6. Preview at: `https://h-feature-my-feature.preview.robsonbot.com`

---

**Note**: This is a quick reference. See `docs/AGENTS.md` for comprehensive details.
