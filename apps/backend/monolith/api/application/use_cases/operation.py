"""
Operation use cases (Gate 6).

This module contains business logic for Operation entities.
"""

from __future__ import annotations
import logging
from dataclasses import dataclass
from typing import Protocol

logger = logging.getLogger(__name__)


@dataclass
class CancelOperationCommand:
    """Command to cancel an operation (Gate 6).

    Attributes:
        operation_id: ID of the operation to cancel
        client_id: ID of the client requesting cancellation (for tenant isolation)
    """
    operation_id: int
    client_id: int


@dataclass
class CancelOperationResult:
    """Result of operation cancellation (Gate 6).

    Attributes:
        success: Whether cancellation succeeded
        operation_id: ID of the operation
        previous_status: Status before cancellation attempt
        new_status: Status after cancellation attempt
        error_message: Error message if cancellation failed
    """
    success: bool
    operation_id: int
    previous_status: str
    new_status: str
    error_message: str | None = None


class OperationRepository(Protocol):
    """Repository protocol for Operation entities (testable)."""

    def get(self, **kwargs) -> "Operation":  # type: ignore
        """Get operation by filter criteria."""
        ...


class CancelOperationUseCase:
    """
    Use case for cancelling an operation (Gate 6).

    Business Rules:
    - Only PLANNED or ACTIVE operations can be cancelled
    - Only the owner of an operation can cancel it (tenant isolation)
    - Cancelling an already CANCELLED operation is a safe no-op (idempotent)

    This use case encapsulates the business logic for operation cancellation,
    following the hexagonal architecture pattern. It can be called from
    multiple adapters (CLI, REST API, WebSocket) without code duplication.
    """

    def __init__(self, operation_repository: OperationRepository | None = None):
        """
        Initialize use case.

        Args:
            operation_repository: Optional repository for testability.
                                  Defaults to Operation.objects for production.
        """
        if operation_repository is None:
            from api.models import Operation
            self._operation_repository = Operation.objects  # type: ignore
        else:
            self._operation_repository = operation_repository

    def execute(self, command: CancelOperationCommand) -> CancelOperationResult:
        """
        Execute operation cancellation.

        Args:
            command: Cancellation command with operation_id and client_id

        Returns:
            CancelOperationResult with success status and details

        Example:
            >>> use_case = CancelOperationUseCase()
            >>> result = use_case.execute(
            ...     CancelOperationCommand(operation_id=123, client_id=1)
            ... )
            >>> if result.success:
            ...     print(f"Cancelled: {result.previous_status} -> {result.new_status}")
        """
        # Fetch operation (tenant-filtered via client_id)
        try:
            operation = self._operation_repository.get(
                id=command.operation_id,
                client_id=command.client_id
            )
        except Exception:
            # Operation not found or access denied (tenant isolation)
            return CancelOperationResult(
                success=False,
                operation_id=command.operation_id,
                previous_status="",
                new_status="",
                error_message="Operation not found or access denied"
            )

        previous_status = operation.status

        # Idempotency: Already cancelled is a success (no-op)
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

        # Perform cancellation (uses Gate 5 state machine validation)
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
