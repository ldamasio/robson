# ADR-0020: Gate 6 - Operation Cancellation (Domain Core)

**Status**: PROPOSED (Revised)
**Date**: 2025-01-01
**Deciders**: Product Owner, Development Team
**Related**:
- Gate 4: Operation Creation (L2 from LIVE TradingIntent)
- Gate 5: Operation Lifecycle Validation
- ADR-0002: Hexagonal Architecture (domain/application separation)
- ADR-0007: Robson is Risk Assistant (user-initiated actions)
- docs/architecture/OPERATION-LIFECYCLE.md
- Gate 7: Operation Cancellation API (REST endpoint - FUTURE)

---

## Context

### Current State (Post-Gate 5)

After Gate 5, the system has:

1. **Operation Lifecycle State Machine** (`Operation.set_status()`):
   - `PLANNED → {ACTIVE, CANCELLED}`
   - `ACTIVE → {CLOSED, CANCELLED}`
   - `CLOSED → ()` (terminal)
   - `CANCELLED → ()` (terminal)

2. **Close Flows** (synchronous, proven):
   - **Stop Monitor** (`stop_monitor.py`): Automated stop/take-profit execution
   - **Manual Close** (`close_position.py` CLI): User-initiated position closure

3. **State Machine Gap**: The `CANCELLED` status exists in the state machine, but there's **no domain use case** to perform cancellation.

### Problem Statement

**No domain-level use case exists for cancelling operations.**

The state machine supports `ACTIVE → CANCELLED` and `PLANNED → CANCELLED` transitions, but:
- No `CancelOperationUseCase` exists in `api/application/`
- No business logic defines *when* cancellation is valid
- No invariants are enforced beyond state transition rules
- Direct use of `operation.set_status("CANCELLED")` bypasses business logic

**Architectural Concern**: Following ADR-0002 (Hexagonal Architecture), business logic should live in **use cases**, not be scattered across views, commands, and ad-hoc scripts.

### Why This Matters

1. **Testability**: Use cases are easier to test than views/CLI commands
2. **Reusability**: Multiple adapters (REST, CLI, WebSocket) can share the same use case
3. **Consistency**: All cancellation paths go through the same business logic
4. **Maintainability**: Business rules in one place, not duplicated

---

## Decision

**Gate 6: Define a `CancelOperationUseCase` at the domain/application level.**

### Scope

**IN Scope (Gate 6)**:
1. **Use Case**: `CancelOperationUseCase` in `api/application/use_cases.py`
2. **Business Logic**: Validate cancellability + execute cancellation
3. **Domain Invariants**: Rules for when cancellation is allowed
4. **Unit Tests**: Pure business logic tests (no Django views)
5. **CLI Trigger**: Internal management command for testing/validation

**OUT of Scope (Gate 6)**:
- REST API endpoint (deferred to Gate 7)
- WebSocket support
- Exchange order cancellation
- Background job processing
- UI implementation

**Future (Gate 7)**:
- REST endpoint: `POST /api/operations/{id}/cancel/`
- Public API documentation
- Frontend integration

### Hexagonal Architecture Compliance

Following ADR-0002:

```
┌─────────────────────────────────────────────────┐
│  DRIVING ADAPTERS (Gate 7 - FUTURE)            │
│  ├─ REST API View                               │
│  ├─ CLI Command (internal - Gate 6)            │
│  └─ WebSocket Handler (future)                  │
└─────────────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│  APPLICATION LAYER (Gate 6 - THIS ADR)         │
│  ┌──────────────────────────────────────────┐  │
│  │  CancelOperationUseCase                   │  │
│  │  - execute()                              │  │
│  │  - validate cancellability               │  │
│  │  - update status                          │  │
│  └──────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
                    │
                    ▼
┌─────────────────────────────────────────────────┐
│  DOMAIN LAYER (Existing - Gate 5)              │
│  ┌──────────────────────────────────────────┐  │
│  │  Operation                               │  │
│  │  - set_status("CANCELLED")               │  │
│  │  - state machine validation              │  │
│  └──────────────────────────────────────────┘  │
└─────────────────────────────────────────────────┘
```

---

## Options Considered

### Option 1: Domain Use Case + CLI Trigger (CHOSEN)

**Description**:
- Create `CancelOperationUseCase` in `api/application/use_cases.py`
- Define `CancelOperationCommand` (request DTO)
- Create `cancel_operation` CLI command as internal trigger
- REST endpoint deferred to Gate 7

**Pros**:
- ✅ Follows hexagonal architecture (ADR-0002)
- ✅ Testable business logic (pure use case tests)
- ✅ Reusable across future adapters (REST, WebSocket)
- ✅ Single source of truth for cancellation rules
- ✅ CLI provides immediate test/validation path

**Cons**:
- ❌ No public API yet (user-facing delayed to Gate 7)

**Blast Radius**:
- Low: New use case file + CLI command
- Rollback: Delete files, no migration needed

---

### Option 2: Skip Use Case, Add CLI Directly

**Description**:
- Create management command that directly calls `operation.set_status("CANCELLED")`
- No use case abstraction
- Add REST endpoint later

**Pros**:
- ✅ Faster implementation (less code)
- ✅ Works for CLI-only use case

**Cons**:
- ❌ Violates hexagonal architecture (business logic in adapter)
- ❌ Duplicates logic when REST endpoint added in Gate 7
- ❌ Harder to test (Django command tests vs pure unit tests)
- ❌ No single source of truth for cancellation rules

**Blast Radius**:
- Low: Single CLI command file
- Rollback: Delete file

---

### Option 3: Full Stack (Use Case + REST + CLI)

**Description**:
- Implement everything in one gate:
  - Use case
  - REST endpoint
  - CLI command
  - API documentation

**Pros**:
- ✅ Complete feature delivered
- ✅ User-visible immediately

**Cons**:
- ❌ Larger gate (more review surface)
- ❌ Mixes concerns (domain vs API)
- ❌ Harder to reason about invariants

**Blast Radius**:
- Medium: Multiple files across layers
- Rollback: Revert multiple PRs

---

### Option 4: Do Nothing (Use set_status Directly)

**Description**:
- Continue using `operation.set_status("CANCELLED")` directly in views/commands
- No dedicated use case

**Pros**:
- ✅ No new code

**Cons**:
- ❌ Business logic scattered
- ❌ Inconsistent patterns (close flows have structure)
- ❌ Harder to test

**Blast Radius**:
- Zero: No changes

---

## Non-Goals

**Explicitly OUT of scope for Gate 6**:

1. **REST API Endpoint**: Deferred to Gate 7
2. **Exchange Order Cancellation**: Assumes no open orders or handled separately
3. **Partial Cancellation**: Cancel only part of a position
4. **Bulk Cancellation**: Cancel multiple operations at once
5. **Scheduled Cancellation**: Cancel at a specific time
6. **WebSocket Events**: Real-time cancellation broadcasts
7. **UI Implementation**: Frontend changes

---

## Acceptance Criteria

### Functional Requirements

1. **Use Case**: `CancelOperationUseCase.execute(command)`
   - Validates operation exists
   - Validates operation is cancellable (status in {PLANNED, ACTIVE})
   - Validates caller owns the operation (tenant isolation)
   - Calls `operation.set_status("CANCELLED")`
   - Saves operation
   - Returns result (success/error)

2. **Command DTO**: `CancelOperationCommand`
   ```python
   @dataclass
   class CancelOperationCommand:
       operation_id: int
       client_id: int  # For tenant isolation
   ```

3. **Result DTO**: `CancelOperationResult`
   ```python
   @dataclass
   class CancelOperationResult:
       success: bool
       operation_id: int
       previous_status: str
       new_status: str
       error_message: str | None = None
   ```

4. **CLI Command**: `python manage.py cancel_operation --operation-id {id}`
   - Internal use only (testing/validation)
   - Calls `CancelOperationUseCase`
   - Provides clear output

5. **Idempotency**:
   - Cancelling an already `CANCELLED` operation succeeds (no-op)
   - Safe to retry

### Test Plan

**Unit Tests** (`api/tests/test_gate_6_cancel_use_case.py`):

```python
class TestCancelOperationUseCase:
    def test_cancel_active_operation_success():
        """Use case cancels ACTIVE operation."""
        # Arrange: Operation in ACTIVE state
        # Act: Call use case
        # Assert: Status is CANCELLED
        pass

    def test_cancel_planned_operation_success():
        """Use case cancels PLANNED operation."""
        pass

    def test_cancel_closed_operation_fails():
        """Cannot cancel CLOSED operations (terminal state)."""
        pass

    def test_cancel_already_cancelled_succeeds_idempotent():
        """Cancelling CANCELLED operation is safe no-op."""
        pass

    def test_cross_tenant_isolation_enforced():
        """User cannot cancel another user's operation."""
        # Arrange: Operation owned by client A
        # Act: Try to cancel with client B
        # Assert: Permission denied
        pass

    def test_nonexistent_operation_fails():
        """Cancelling non-existent operation fails gracefully."""
        pass

    def test_returns_success_result():
        """Use case returns CancelOperationResult with correct fields."""
        pass

    def test_returns_error_result_for_invalid_state():
        """Use case returns CancelOperationResult with error_message."""
        pass
```

**Integration Tests** (CLI command):
- Verify CLI command calls use case
- Verify output format

**Regression Tests**:
- Gate 4 tests (operation creation)
- Gate 5 tests (lifecycle validation)
- Ensure existing close flows still work

---

## Implementation Plan

### Phase 1: Domain Core (Gate 6)

**File: `api/application/use_cases.py`** (new or append)

```python
from dataclasses import dataclass
from typing import TYPE_CHECKING
import logging

from api.models import Operation
from api.models.trading import InvalidOperationStatusError

if TYPE_CHECKING:
    from api.models import Client

logger = logging.getLogger(__name__)


@dataclass
class CancelOperationCommand:
    """Command to cancel an operation."""
    operation_id: int
    client_id: int


@dataclass
class CancelOperationResult:
    """Result of operation cancellation."""
    success: bool
    operation_id: int
    previous_status: str
    new_status: str
    error_message: str | None = None


class CancelOperationUseCase:
    """
    Use case for cancelling an operation.

    Business Rules:
    - Only PLANNED or ACTIVE operations can be cancelled
    - Only the owner of an operation can cancel it
    - Cancelling an already CANCELLED operation is a safe no-op
    """

    def __init__(self, operation_repository=None):
        """
        Initialize use case.

        Args:
            operation_repository: Optional repository for testability
        """
        self._operation_repository = operation_repository or Operation.objects

    def execute(self, command: CancelOperationCommand) -> CancelOperationResult:
        """
        Execute operation cancellation.

        Args:
            command: Cancellation command with operation_id and client_id

        Returns:
            CancelOperationResult with success status and details
        """
        # Fetch operation (tenant-filtered)
        try:
            operation = self._operation_repository.get(
                id=command.operation_id,
                client_id=command.client_id
            )
        except Operation.DoesNotExist:
            return CancelOperationResult(
                success=False,
                operation_id=command.operation_id,
                previous_status="",
                new_status="",
                error_message="Operation not found or access denied"
            )

        previous_status = operation.status

        # Check if already cancelled (idempotency)
        if previous_status == "CANCELLED":
            return CancelOperationResult(
                success=True,
                operation_id=operation.id,
                previous_status=previous_status,
                new_status="CANCELLED",
                error_message=None
            )

        # Validate cancellable state
        if previous_status not in ("PLANNED", "ACTIVE"):
            return CancelOperationResult(
                success=False,
                operation_id=operation.id,
                previous_status=previous_status,
                new_status=previous_status,
                error_message=f"Cannot cancel operation in {previous_status} state"
            )

        # Perform cancellation
        operation.set_status("CANCELLED")
        operation.save()

        logger.info(
            f"Operation cancelled: id={operation.id}, "
            f"client_id={command.client_id}, previous_status={previous_status}"
        )

        return CancelOperationResult(
            success=True,
            operation_id=operation.id,
            previous_status=previous_status,
            new_status="CANCELLED",
            error_message=None
        )
```

**File: `api/management/commands/cancel_operation.py`**

```python
from django.core.management.base import BaseCommand
from api.application.use_cases import (
    CancelOperationUseCase,
    CancelOperationCommand
)
from clients.models import Client


class Command(BaseCommand):
    help = 'Cancel an operation (internal command for testing/validation)'

    def add_arguments(self, parser):
        parser.add_argument('--operation-id', type=int, required=True)
        parser.add_argument('--client-id', type=int, required=True)
        parser.add_argument('--confirm', action='store_true',
                          help='Required for safety')

    def handle(self, *args, **options):
        operation_id = options['operation_id']
        client_id = options['client_id']
        confirm = options['confirm']

        if not confirm:
            self.stdout.write(self.style.ERROR('--confirm flag is required'))
            return

        # Execute use case
        use_case = CancelOperationUseCase()
        command = CancelOperationCommand(
            operation_id=operation_id,
            client_id=client_id
        )
        result = use_case.execute(command)

        if result.success:
            self.stdout.write(self.style.SUCCESS(
                f"✅ Operation #{result.operation_id} cancelled "
                f"({result.previous_status} → {result.new_status})"
            ))
        else:
            self.stdout.write(self.style.ERROR(
                f"❌ Failed: {result.error_message}"
            ))
```

### Phase 2: Tests (Gate 6)

**File: `api/tests/test_gate_6_cancel_use_case.py`**

```python
import pytest
from api.application.use_cases import (
    CancelOperationUseCase,
    CancelOperationCommand,
)
from api.models import Operation, Symbol, Strategy
from clients.models import Client


@pytest.mark.django_db
class TestCancelOperationUseCase:
    # ... test methods from Acceptance Criteria above ...
    pass
```

### Phase 3: REST API (Gate 7 - FUTURE)

**Deferred**: REST endpoint will be added in Gate 7 as a **driving adapter** that calls the same use case:

```python
# Gate 7: api/views/operation_views.py
from api.application.use_cases import CancelOperationUseCase, CancelOperationCommand

@api_view(['POST'])
@permission_classes([IsAuthenticated])
def cancel_operation(request, operation_id: int):
    use_case = CancelOperationUseCase()
    command = CancelOperationCommand(
        operation_id=operation_id,
        client_id=request.user.id
    )
    result = use_case.execute(command)

    if result.success:
        return Response({'status': 'cancelled'}, status=200)
    else:
        return Response({'error': result.error_message}, status=409)
```

---

## Rollout Plan

### Backwards Compatibility

✅ **Fully Backwards Compatible**: Adding a new use case and CLI command doesn't affect existing code.

### Feature Flags

Not required: New use case is opt-in. Existing flows unchanged.

### Deployment Steps

1. Deploy code (use case + CLI command exist)
2. Run tests to verify
3. Internal validation using CLI
4. (Gate 7) Add REST endpoint
5. (Gate 7) Update API documentation
6. (Gate 7) Frontend team adds cancel button

### Rollback Plan

If issues arise:
1. Delete use case file and CLI command
2. No database rollback needed (no schema changes)

---

## Security Considerations

### Tenant Isolation

**Critical**: Use case MUST enforce tenant isolation via `client_id`:

```python
# ✅ CORRECT: Tenant-filtered query
operation = Operation.objects.get(
    id=command.operation_id,
    client_id=command.client_id
)

# ❌ WRONG: Bypasses tenant check
operation = Operation.objects.get(id=command.operation_id)
```

The use case enforces this invariant at the **domain level**, not at the adapter level.

### Authorization

- Only the **owner** of an operation can cancel it (enforced by `client_id` match)
- Admin users cannot cancel other users' operations

### Idempotency and Safety

- Cancelling an already `CANCELLED` operation is a safe no-op
- No side effects from duplicate requests

---

## Observability

### Logging

Use Django's built-in logging (no new infra):

```python
logger.info(
    f"Operation cancelled: id={operation.id}, "
    f"client_id={command.client_id}, previous_status={previous_status}"
)
```

### Metrics

Use existing Prometheus middleware (no new infra):

```python
from prometheus_client import Counter

cancellation_counter = Counter(
    'operation_cancellation_total',
    'Total operation cancellations',
    ['status']  # label: previous_status
)
```

---

## Consequences

### Positive

✅ **Complete Feature**: State machine now fully usable at domain level
✅ **Hexagonal Compliance**: Business logic in use case, adapters can call it
✅ **Testable**: Pure unit tests for business logic
✅ **Reusable**: REST (Gate 7), CLI (now), WebSocket (future) all use same use case
✅ **Single Source of Truth**: Cancellation rules in one place

### Negative

❌ **No Public API Yet**: User-facing REST endpoint deferred to Gate 7
❌ **Manual Testing**: Only CLI available until Gate 7

### Neutral

⚪ **Extensible**: Easy to add more adapters later
⚪ **Clean Architecture**: Follows ADR-0002 patterns

---

## Related Decisions

- **ADR-0002**: Hexagonal Architecture (domain/application separation)
- **ADR-0007**: Robson is Risk Assistant (user-initiated actions)
- **Gate 4**: Operation Creation (L2 from LIVE TradingIntent)
- **Gate 5**: Operation Lifecycle Validation
- **Gate 7**: Operation Cancellation API (REST endpoint - FUTURE)
- **OPERATION-LIFECYCLE.md**: State machine specification

---

## Open Questions

1. **Exchange Order Handling**: Should cancellation attempt to cancel open exchange orders?
   - **Recommendation**: No, defer to future gate (outside scope)

2. **Audit Trail**: Should cancellation create an `AuditTransaction` entry?
   - **Recommendation**: Yes, if framework supports `AUDIT_TYPE = 'OPERATION_CANCELLED'`

3. **Gate 7 Scope**: Should Gate 7 include WebSocket support or just REST?
   - **Recommendation**: REST only, WebSocket deferred further

---

## Approval

**Status**: AWAITING APPROVAL (Revised)

**Changes from Previous Version**:
- Removed REST endpoint (deferred to Gate 7)
- Focused on domain/application use case
- CLI retained as internal trigger only
- Added hexagonal architecture diagram

**Questions for Product Owner**:
1. Is domain-focused approach acceptable (defer REST to Gate 7)?
2. Should CLI remain available in production, or be internal-only?

**Questions for Engineering**:
1. Does use case design follow existing patterns?
2. Any concerns about tenant isolation enforcement?

---

**Next Steps**:
1. Review and approve this ADR
2. Implement Phase 1 (Use Case + Tests)
3. Implement Phase 2 (CLI Command)
4. Run test suite
5. (Gate 7) Implement REST endpoint

---

**Last Updated**: 2025-01-01 (Revised - Domain Core Focus)
**Author**: Claude Code (Gate 6 Planning)
**Review Cycle**: Before Gate 6 implementation
