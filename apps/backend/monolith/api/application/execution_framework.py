"""
Execution Framework wrapper for TradingIntent execution.

This provides a simple API for executing trading intents in dry-run or live mode.
"""

from api.application.execution import (
    ExecutionResult,
    ExecutionStatus,
    ExecutionMode,
    ExecutionGuard,
)
from api.models import TradingIntent


class ExecutionFramework:
    """
    Framework for executing trading intents.

    This is a thin wrapper that provides a simple execute() method
    for TradingIntent objects in dry-run or live mode.
    """

    def __init__(self):
        pass

    def execute(self, intent: TradingIntent, mode: str = "dry-run") -> ExecutionResult:
        """
        Execute a trading intent.

        Args:
            intent: TradingIntent to execute
            mode: "dry-run" (default) or "live"

        Returns:
            ExecutionResult with status and actions
        """
        exec_mode = ExecutionMode.LIVE if mode == "live" else ExecutionMode.DRY_RUN

        result = ExecutionResult(
            status=ExecutionStatus.SUCCESS,
            mode=exec_mode,
        )

        # Guard 1: Check if intent is validated
        if not intent.validation_result or intent.validation_result.get('status') != 'PASS':
            result.status = ExecutionStatus.BLOCKED
            result.add_guard(ExecutionGuard(
                name="Validation Required",
                passed=False,
                message="Intent must be validated before execution",
                details={"intent_id": intent.intent_id}
            ))
            return result

        result.add_guard(ExecutionGuard(
            name="Validation Check",
            passed=True,
            message="Intent validation passed",
            details={}
        ))

        # Execute the order (simulated in dry-run)
        if exec_mode == ExecutionMode.DRY_RUN:
            result.add_action({
                "type": "SIMULATED_ORDER",
                "side": intent.side,
                "symbol": intent.symbol.name,
                "quantity": str(intent.quantity),
                "price": str(intent.entry_price),
                "status": "SIMULATED"
            })
            result.add_action({
                "type": "SIMULATED_STOP",
                "symbol": intent.symbol.name,
                "stop_price": str(intent.stop_price),
                "status": "SIMULATED"
            })
        else:
            # TODO: Integrate with actual Binance order placement
            result.add_action({
                "type": "LIVE_ORDER",
                "side": intent.side,
                "symbol": intent.symbol.name,
                "quantity": str(intent.quantity),
                "price": str(intent.entry_price),
                "status": "PENDING",
                "message": "Live execution not yet implemented"
            })

        return result
