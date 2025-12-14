"""
Execution framework with safety-first semantics.

This is where ideas touch reality.
In trading, this is LIVE trading.
In agentic coding, this is production execution.

The system is SAFE BY DEFAULT.

Principles:
- DRY-RUN is the default (no real orders)
- LIVE requires explicit acknowledgement
- Validation required before LIVE execution
- Execution limits enforced
- Audit trail always recorded
"""

from __future__ import annotations
from dataclasses import dataclass, field
from typing import Protocol, Any
from decimal import Decimal
from datetime import datetime
from enum import Enum

from django.utils import timezone


class ExecutionMode(Enum):
    """Execution mode."""

    DRY_RUN = "DRY_RUN"  # Simulation (default, safe)
    LIVE = "LIVE"  # Real orders (requires explicit acknowledgement)


class ExecutionStatus(Enum):
    """Execution result status."""

    SUCCESS = "SUCCESS"
    FAILED = "FAILED"
    BLOCKED = "BLOCKED"  # Execution prevented by safety check


@dataclass
class ExecutionGuard:
    """
    Safety check result.

    Guards prevent unsafe execution before it happens.
    """

    name: str
    passed: bool
    message: str
    details: dict = field(default_factory=dict)

    def __str__(self) -> str:
        status = "✓" if self.passed else "✗"
        return f"{status} {self.name}: {self.message}"


@dataclass
class ExecutionResult:
    """
    Result of an execution attempt.

    Contains:
    - Status (success/failed/blocked)
    - Mode (dry-run/live)
    - Guards executed
    - Actions taken (simulated or real)
    - Audit information
    """

    status: ExecutionStatus
    mode: ExecutionMode
    guards: list[ExecutionGuard] = field(default_factory=list)
    actions: list[dict] = field(default_factory=list)
    metadata: dict = field(default_factory=dict)
    executed_at: datetime = field(default_factory=timezone.now)
    error: str | None = None

    def add_guard(self, guard: ExecutionGuard) -> None:
        """Add a guard result."""
        self.guards.append(guard)

    def add_action(self, action: dict) -> None:
        """Record an action (simulated or real)."""
        self.actions.append(action)

    def is_blocked(self) -> bool:
        """Check if execution was blocked by guards."""
        return self.status == ExecutionStatus.BLOCKED

    def is_success(self) -> bool:
        """Check if execution succeeded."""
        return self.status == ExecutionStatus.SUCCESS

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON serialization."""
        return {
            "status": self.status.value,
            "mode": self.mode.value,
            "guards": [
                {
                    "name": g.name,
                    "passed": g.passed,
                    "message": g.message,
                    "details": g.details,
                }
                for g in self.guards
            ],
            "actions": self.actions,
            "metadata": self.metadata,
            "executed_at": self.executed_at.isoformat(),
            "error": self.error,
            "summary": {
                "total_guards": len(self.guards),
                "passed_guards": sum(1 for g in self.guards if g.passed),
                "failed_guards": sum(1 for g in self.guards if not g.passed),
                "total_actions": len(self.actions),
            },
        }

    def to_human_readable(self) -> str:
        """Convert to human-readable text."""
        lines = []
        lines.append("=" * 60)
        lines.append("EXECUTION REPORT")
        lines.append("=" * 60)
        lines.append("")

        # Mode
        if self.mode == ExecutionMode.DRY_RUN:
            lines.append("MODE: DRY-RUN (Simulation)")
            lines.append("⚠️  No real orders were placed")
        else:
            lines.append("MODE: LIVE (Real Orders)")
            lines.append("✓ Real orders executed on exchange")

        lines.append("")

        # Status
        if self.status == ExecutionStatus.SUCCESS:
            lines.append("STATUS: ✓ SUCCESS")
        elif self.status == ExecutionStatus.FAILED:
            lines.append("STATUS: ✗ FAILED")
            if self.error:
                lines.append(f"Error: {self.error}")
        else:
            lines.append("STATUS: ✗ BLOCKED")
            lines.append("Execution prevented by safety checks")

        lines.append("")

        # Metadata
        if self.metadata:
            lines.append("Context:")
            for key, value in self.metadata.items():
                lines.append(f"  {key}: {value}")
            lines.append("")

        # Guards
        if self.guards:
            lines.append("SAFETY CHECKS:")
            for guard in self.guards:
                lines.append(f"  {guard}")
            lines.append("")

        # Actions
        if self.actions:
            lines.append(f"ACTIONS TAKEN ({len(self.actions)}):")
            for idx, action in enumerate(self.actions, 1):
                action_type = action.get("type", "unknown")
                description = action.get("description", "")
                lines.append(f"  {idx}. [{action_type}] {description}")
                if action.get("result"):
                    lines.append(f"     Result: {action['result']}")
            lines.append("")

        # Summary
        lines.append("Summary:")
        lines.append(f"  Executed at: {self.executed_at.strftime('%Y-%m-%d %H:%M:%S')}")
        lines.append(f"  Safety checks: {len(self.guards)} ({sum(1 for g in self.guards if g.passed)} passed)")
        lines.append(f"  Actions: {len(self.actions)}")
        lines.append("")

        lines.append("=" * 60)

        return "\n".join(lines)


# ==========================================
# EXECUTION GUARDS (Safety Checks)
# ==========================================


class ExecutionGuardProtocol(Protocol):
    """Protocol for execution guards."""

    def check(self, context: dict) -> ExecutionGuard:
        """
        Perform safety check.

        Args:
            context: Execution context

        Returns:
            ExecutionGuard with pass/fail result
        """
        ...


class ValidationRequiredGuard:
    """
    Guard that requires prior validation.

    LIVE execution MUST be validated first.
    DRY-RUN can skip validation.
    """

    def check(self, context: dict) -> ExecutionGuard:
        """Check if validation is required and present."""
        mode = context.get("mode", ExecutionMode.DRY_RUN)

        # DRY-RUN doesn't require validation
        if mode == ExecutionMode.DRY_RUN:
            return ExecutionGuard(
                name="ValidationRequired",
                passed=True,
                message="Validation not required for DRY-RUN",
            )

        # LIVE requires validation
        validated = context.get("validated", False)
        validation_passed = context.get("validation_passed", False)

        if not validated:
            return ExecutionGuard(
                name="ValidationRequired",
                passed=False,
                message="LIVE execution requires prior validation",
                details={
                    "requirement": "Run 'robson validate' first",
                    "mode": "LIVE",
                },
            )

        if not validation_passed:
            return ExecutionGuard(
                name="ValidationRequired",
                passed=False,
                message="Validation failed - cannot execute",
                details={
                    "requirement": "Fix validation issues before LIVE execution",
                },
            )

        return ExecutionGuard(
            name="ValidationRequired",
            passed=True,
            message="Plan validated successfully",
        )


class TenantContextGuard:
    """
    Guard that requires explicit tenant context.

    ALL executions must have client_id.
    """

    def check(self, context: dict) -> ExecutionGuard:
        """Check if client_id is present and valid."""
        client_id = context.get("client_id")

        if client_id is None:
            return ExecutionGuard(
                name="TenantContext",
                passed=False,
                message="client_id is required for all executions",
                details={
                    "requirement": "Specify --client-id flag",
                },
            )

        try:
            client_id_int = int(client_id)
            if client_id_int <= 0:
                return ExecutionGuard(
                    name="TenantContext",
                    passed=False,
                    message=f"client_id must be positive, got: {client_id}",
                )
        except (ValueError, TypeError):
            return ExecutionGuard(
                name="TenantContext",
                passed=False,
                message=f"client_id must be an integer, got: {type(client_id).__name__}",
            )

        return ExecutionGuard(
            name="TenantContext",
            passed=True,
            message=f"Tenant context valid (client_id={client_id})",
        )


class ExecutionLimitsGuard:
    """
    Guard that enforces execution limits.

    Limits:
    - max_orders: Maximum number of orders per day
    - max_notional: Maximum total value of orders
    - max_loss: Maximum allowed loss

    Only enforced for LIVE execution.
    """

    def check(self, context: dict) -> ExecutionGuard:
        """Check execution limits."""
        mode = context.get("mode", ExecutionMode.DRY_RUN)

        # DRY-RUN doesn't enforce limits
        if mode == ExecutionMode.DRY_RUN:
            return ExecutionGuard(
                name="ExecutionLimits",
                passed=True,
                message="Limits not enforced for DRY-RUN",
            )

        # Get limits
        limits = context.get("limits", {})
        max_orders = limits.get("max_orders_per_day")
        max_notional = limits.get("max_notional_per_day")
        max_loss = limits.get("max_loss_per_day")

        # Get current stats
        stats = context.get("stats", {})
        current_orders = stats.get("orders_today", 0)
        current_notional = Decimal(str(stats.get("notional_today", 0)))
        current_loss = Decimal(str(stats.get("loss_today", 0)))

        # Check max_orders
        if max_orders is not None and current_orders >= max_orders:
            return ExecutionGuard(
                name="ExecutionLimits",
                passed=False,
                message=f"Max orders limit reached: {current_orders} >= {max_orders}",
                details={
                    "current": current_orders,
                    "limit": max_orders,
                    "type": "max_orders_per_day",
                },
            )

        # Check max_notional
        if max_notional is not None:
            max_notional_decimal = Decimal(str(max_notional))
            if current_notional >= max_notional_decimal:
                return ExecutionGuard(
                    name="ExecutionLimits",
                    passed=False,
                    message=f"Max notional limit reached: {current_notional} >= {max_notional}",
                    details={
                        "current": str(current_notional),
                        "limit": str(max_notional),
                        "type": "max_notional_per_day",
                    },
                )

        # Check max_loss
        if max_loss is not None:
            max_loss_decimal = Decimal(str(max_loss))
            if current_loss >= max_loss_decimal:
                return ExecutionGuard(
                    name="ExecutionLimits",
                    passed=False,
                    message=f"Max loss limit reached: {current_loss} >= {max_loss}",
                    details={
                        "current": str(current_loss),
                        "limit": str(max_loss),
                        "type": "max_loss_per_day",
                    },
                )

        return ExecutionGuard(
            name="ExecutionLimits",
            passed=True,
            message="All execution limits within bounds",
            details={
                "orders": f"{current_orders}/{max_orders or 'unlimited'}",
                "notional": f"{current_notional}/{max_notional or 'unlimited'}",
                "loss": f"{current_loss}/{max_loss or 'unlimited'}",
            },
        )


class AcknowledgementGuard:
    """
    Guard that requires explicit acknowledgement for LIVE mode.

    LIVE execution must have --live AND --acknowledge-risk flags.
    """

    def check(self, context: dict) -> ExecutionGuard:
        """Check if LIVE mode is properly acknowledged."""
        mode = context.get("mode", ExecutionMode.DRY_RUN)

        # DRY-RUN doesn't need acknowledgement
        if mode == ExecutionMode.DRY_RUN:
            return ExecutionGuard(
                name="Acknowledgement",
                passed=True,
                message="Acknowledgement not required for DRY-RUN",
            )

        # LIVE requires acknowledgement
        acknowledged = context.get("acknowledge_risk", False)

        if not acknowledged:
            return ExecutionGuard(
                name="Acknowledgement",
                passed=False,
                message="LIVE execution requires explicit risk acknowledgement",
                details={
                    "requirement": "Add --acknowledge-risk flag",
                    "warning": "Real orders will be placed on the exchange",
                },
            )

        return ExecutionGuard(
            name="Acknowledgement",
            passed=True,
            message="Risk acknowledged for LIVE execution",
        )


# ==========================================
# EXECUTION USE CASE
# ==========================================


class ExecutePlanUseCase:
    """
    Use case for executing a plan.

    This is the final step of the agentic workflow: PLAN → VALIDATE → EXECUTE

    Safety features:
    - DRY-RUN by default (safe)
    - LIVE requires explicit acknowledgement
    - Guards prevent unsafe execution
    - Audit trail always recorded
    """

    def __init__(self, guards: list[ExecutionGuardProtocol] | None = None):
        self.guards = guards or [
            TenantContextGuard(),
            ValidationRequiredGuard(),
            AcknowledgementGuard(),
            ExecutionLimitsGuard(),
        ]

    def execute(self, context: dict) -> ExecutionResult:
        """
        Execute a plan with safety checks.

        Args:
            context: Execution context containing:
                - mode: ExecutionMode (DRY_RUN or LIVE)
                - client_id: Tenant ID
                - plan_id: Plan identifier
                - validated: Whether plan was validated
                - validation_passed: Whether validation passed
                - acknowledge_risk: Risk acknowledgement for LIVE
                - limits: Execution limits
                - stats: Current execution stats
                - operation: Operation to execute

        Returns:
            ExecutionResult with status and audit trail
        """
        mode = context.get("mode", ExecutionMode.DRY_RUN)
        plan_id = context.get("plan_id", "unknown")
        client_id = context.get("client_id")

        # Create result
        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=mode,
        )
        result.metadata["plan_id"] = plan_id
        result.metadata["client_id"] = client_id
        result.metadata["mode"] = mode.value

        # Run all guards
        for guard in self.guards:
            guard_result = guard.check(context)
            result.add_guard(guard_result)

            # If any guard fails, block execution
            if not guard_result.passed:
                result.status = ExecutionStatus.BLOCKED
                result.error = f"Guard failed: {guard_result.name}"
                return result

        # All guards passed - proceed with execution
        operation = context.get("operation", {})

        if mode == ExecutionMode.DRY_RUN:
            # Simulate execution
            result.add_action({
                "type": "SIMULATED_ORDER",
                "description": f"Simulated {operation.get('type', 'order')}",
                "details": operation,
                "result": "Simulated successfully (no real order placed)",
            })
        else:
            # LIVE execution
            # TODO: Integrate with actual exchange execution
            result.add_action({
                "type": "REAL_ORDER",
                "description": f"LIVE {operation.get('type', 'order')}",
                "details": operation,
                "result": "Order placed on exchange (TODO: implement actual execution)",
            })

        return result
