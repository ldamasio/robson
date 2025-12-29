"""
Dependency Injection wiring for Entry Gate system.

Provides factory functions to construct fully-wired use cases.
"""

from .use_cases import (
    CheckDynamicPositionLimit,
    CheckStopOutCooldown,
    CheckMarketContext,
    EvaluateEntryGate,
)
from .adapters import (
    DjangoPositionCountRepository,
    DjangoMonthlyPnLRepository,
    DjangoStopOutRepository,
    DjangoMarketDataRepository,
    DjangoConfigRepository,
    DjangoDecisionRepository,
)


def get_entry_gate_evaluator() -> EvaluateEntryGate:
    """
    Factory function to create a fully-wired EntryGate evaluator.

    Returns:
        EvaluateEntryGate use case with all dependencies injected

    Usage:
        gate = get_entry_gate_evaluator()
        decision = gate.execute(client_id=1, symbol="BTCUSDT")
    """
    # Create repositories
    position_repo = DjangoPositionCountRepository()
    pnl_repo = DjangoMonthlyPnLRepository()
    stop_repo = DjangoStopOutRepository()
    market_repo = DjangoMarketDataRepository()
    config_repo = DjangoConfigRepository()
    decision_repo = DjangoDecisionRepository()

    # Create gate checks
    check_position_limit = CheckDynamicPositionLimit(position_repo, pnl_repo)
    check_cooldown = CheckStopOutCooldown(stop_repo, config_repo)
    check_market = CheckMarketContext(market_repo, config_repo)

    # Create orchestrator
    evaluator = EvaluateEntryGate(
        check_position_limit=check_position_limit,
        check_cooldown=check_cooldown,
        check_market=check_market,
        decision_repo=decision_repo,
    )

    return evaluator
