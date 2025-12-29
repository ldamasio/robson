"""
Use cases (business logic) for Entry Gate system.

NO Django dependencies. Pure business logic using port abstractions.
"""

from datetime import datetime
from decimal import Decimal
from typing import List
from math import floor

from django.utils import timezone

from .domain import GateCheckResult, EntryGateDecision, EntryGateConfig
from .ports import (
    PositionCountRepository,
    MonthlyPnLRepository,
    StopOutRepository,
    MarketDataRepository,
    ConfigRepository,
    DecisionRepository,
)


class CheckDynamicPositionLimit:
    """
    Gate Check: Dynamic concurrent position limit based on monthly risk budget.

    Formula:
        Available Risk Budget = 4% + Monthly P&L %
        Max Concurrent Positions = floor(Available Risk Budget / 1%)

    Examples:
        - Month start (0% P&L): 4 positions max
        - After +2% profit: 6 positions max
        - After -2% loss: 2 positions max
        - After -4% loss: 0 positions (BLOCKED)
    """

    def __init__(
        self,
        position_repo: PositionCountRepository,
        pnl_repo: MonthlyPnLRepository,
    ):
        self._positions = position_repo
        self._pnl = pnl_repo

    def execute(self, client_id: int) -> GateCheckResult:
        """
        Check if client can open new position based on dynamic risk budget.

        Args:
            client_id: Tenant identifier

        Returns:
            GateCheckResult with PASS/FAIL and details
        """
        # Step 1: Get monthly P&L and capital
        monthly_pnl, capital = self._pnl.get_monthly_pnl(client_id)

        # Step 2: Calculate available risk budget
        BASE_MONTHLY_RISK_PERCENT = Decimal("4.0")
        RISK_PER_POSITION_PERCENT = Decimal("1.0")

        # Convert P&L to percentage
        if capital > 0:
            monthly_pnl_pct = (monthly_pnl / capital) * Decimal("100")
        else:
            monthly_pnl_pct = Decimal("0")

        # Available risk = base + performance
        available_risk_pct = BASE_MONTHLY_RISK_PERCENT + monthly_pnl_pct

        # Step 3: Calculate max concurrent positions
        if available_risk_pct <= 0:
            max_concurrent = 0
        else:
            max_concurrent = int(floor(available_risk_pct / RISK_PER_POSITION_PERCENT))

        # Step 4: Get current active count
        current_count = self._positions.count_active_positions(client_id)

        # Step 5: Gate logic
        passed = current_count < max_concurrent

        if passed:
            message = f"Position limit OK: {current_count}/{max_concurrent} active (budget: {available_risk_pct:.1f}%)"
        else:
            message = f"Max {max_concurrent} concurrent positions allowed (budget: {available_risk_pct:.1f}%). Currently: {current_count}"

        return GateCheckResult(
            gate_name="DynamicPositionLimit",
            passed=passed,
            message=message,
            details={
                "current_count": current_count,
                "max_concurrent": max_concurrent,
                "available_risk_pct": str(available_risk_pct),
                "monthly_pnl": str(monthly_pnl),
                "monthly_pnl_pct": str(monthly_pnl_pct),
                "capital": str(capital),
                "base_risk_pct": str(BASE_MONTHLY_RISK_PERCENT),
                "risk_per_position_pct": str(RISK_PER_POSITION_PERCENT),
            },
        )


class CheckStopOutCooldown:
    """
    Gate Check: Cooldown period after stop-loss execution.

    Prevents revenge trading by enforcing a waiting period (default 15min)
    after any stop-out event.
    """

    def __init__(
        self,
        stop_repo: StopOutRepository,
        config_repo: ConfigRepository,
    ):
        self._stops = stop_repo
        self._config = config_repo

    def execute(self, client_id: int) -> GateCheckResult:
        """
        Check if client is in cooldown period after recent stop-out.

        Args:
            client_id: Tenant identifier

        Returns:
            GateCheckResult with PASS/FAIL and details
        """
        # Get configuration
        config = self._config.get_config(client_id)

        # If cooldown disabled, pass immediately
        if not config.enable_cooldown:
            return GateCheckResult(
                gate_name="StopOutCooldown",
                passed=True,
                message="Cooldown check disabled",
                details={"enabled": False},
            )

        # Get latest stop-out
        latest_stop = self._stops.get_latest_stop_out(client_id)

        # If no stop-outs, pass
        if latest_stop is None:
            return GateCheckResult(
                gate_name="StopOutCooldown",
                passed=True,
                message="No recent stop-outs",
                details={"enabled": True, "latest_stop": None},
            )

        # Check cooldown period
        now = timezone.now()
        seconds_since_stop = (now - latest_stop).total_seconds()
        cooldown_seconds = config.cooldown_after_stop_seconds

        passed = seconds_since_stop >= cooldown_seconds

        if passed:
            message = f"Cooldown period elapsed ({int(seconds_since_stop)}s since last stop)"
        else:
            remaining = cooldown_seconds - int(seconds_since_stop)
            message = f"Cooldown active: {remaining}s remaining (last stop: {latest_stop.isoformat()})"

        return GateCheckResult(
            gate_name="StopOutCooldown",
            passed=passed,
            message=message,
            details={
                "enabled": True,
                "latest_stop": latest_stop.isoformat() if latest_stop else None,
                "seconds_since_stop": int(seconds_since_stop),
                "cooldown_seconds": cooldown_seconds,
                "remaining_seconds": max(0, cooldown_seconds - int(seconds_since_stop)),
            },
        )


class CheckMarketContext:
    """
    Gate Check: Market context constraints (funding rate, data freshness).

    Optional checks that can be disabled via configuration:
    1. Extreme funding rate (potential squeeze risk)
    2. Stale market data (outdated context)
    """

    def __init__(
        self,
        market_repo: MarketDataRepository,
        config_repo: ConfigRepository,
    ):
        self._market = market_repo
        self._config = config_repo

    def execute(self, client_id: int, symbol: str) -> List[GateCheckResult]:
        """
        Check market context constraints.

        Args:
            client_id: Tenant identifier
            symbol: Trading pair (e.g., BTCUSDT)

        Returns:
            List of GateCheckResults (one per enabled check)
        """
        config = self._config.get_config(client_id)
        results = []

        # Check 1: Extreme funding rate
        if config.enable_funding_rate_gate:
            results.append(self._check_funding_rate(client_id, symbol, config))

        # Check 2: Stale market data
        if config.enable_stale_data_gate:
            results.append(self._check_data_freshness(client_id, symbol, config))

        # If no checks enabled, return a PASS result
        if not results:
            results.append(
                GateCheckResult(
                    gate_name="MarketContext",
                    passed=True,
                    message="Market context checks disabled",
                    details={"enabled": False},
                )
            )

        return results

    def _check_funding_rate(
        self, client_id: int, symbol: str, config: EntryGateConfig
    ) -> GateCheckResult:
        """Check if funding rate is within acceptable range."""
        funding_rate = self._market.get_latest_funding_rate(client_id, symbol)

        if funding_rate is None:
            # No data available - fail safe (deny entry)
            return GateCheckResult(
                gate_name="FundingRate",
                passed=False,
                message="No funding rate data available",
                details={
                    "funding_rate": None,
                    "threshold": str(config.funding_rate_threshold),
                },
            )

        # Check if extreme
        threshold = config.funding_rate_threshold
        is_extreme = abs(funding_rate) > threshold
        passed = not is_extreme

        if passed:
            message = f"Funding rate OK: {funding_rate:.6f} (threshold: ±{threshold})"
        else:
            message = f"Extreme funding rate detected: {funding_rate:.6f} (threshold: ±{threshold})"

        return GateCheckResult(
            gate_name="FundingRate",
            passed=passed,
            message=message,
            details={
                "funding_rate": str(funding_rate),
                "threshold": str(threshold),
                "is_extreme": is_extreme,
            },
        )

    def _check_data_freshness(
        self, client_id: int, symbol: str, config: EntryGateConfig
    ) -> GateCheckResult:
        """Check if market data is fresh enough."""
        data_age = self._market.get_data_age_seconds(client_id, symbol)

        if data_age is None:
            # No data available - fail safe (deny entry)
            return GateCheckResult(
                gate_name="DataFreshness",
                passed=False,
                message="No market data available",
                details={
                    "data_age_seconds": None,
                    "max_age_seconds": config.max_data_age_seconds,
                },
            )

        # Check if stale
        max_age = config.max_data_age_seconds
        is_stale = data_age > max_age
        passed = not is_stale

        if passed:
            message = f"Market data fresh: {int(data_age)}s old (max: {max_age}s)"
        else:
            message = f"Stale market data: {int(data_age)}s old (max: {max_age}s)"

        return GateCheckResult(
            gate_name="DataFreshness",
            passed=passed,
            message=message,
            details={
                "data_age_seconds": int(data_age),
                "max_age_seconds": max_age,
                "is_stale": is_stale,
            },
        )


class EvaluateEntryGate:
    """
    Orchestrator use case: Evaluates all gate checks and makes final decision.

    Decision logic:
    - ALL gates must PASS for ALLOW_ENTRY
    - ANY gate FAIL → DENY_ENTRY
    - Decision is saved to audit trail
    """

    def __init__(
        self,
        check_position_limit: CheckDynamicPositionLimit,
        check_cooldown: CheckStopOutCooldown,
        check_market: CheckMarketContext,
        decision_repo: DecisionRepository,
    ):
        self._check_position_limit = check_position_limit
        self._check_cooldown = check_cooldown
        self._check_market = check_market
        self._decisions = decision_repo

    def execute(self, client_id: int, symbol: str, context: dict = None) -> EntryGateDecision:
        """
        Evaluate all entry gates and make final decision.

        Args:
            client_id: Tenant identifier
            symbol: Trading pair (e.g., BTCUSDT)
            context: Additional context for audit (optional)

        Returns:
            EntryGateDecision with allowed flag and reasons
        """
        if context is None:
            context = {}

        # Run all gate checks
        gate_checks = {}
        all_results = []

        # Check 1: Dynamic position limit
        position_result = self._check_position_limit.execute(client_id)
        gate_checks["DynamicPositionLimit"] = position_result
        all_results.append(position_result)

        # Check 2: Stop-out cooldown
        cooldown_result = self._check_cooldown.execute(client_id)
        gate_checks["StopOutCooldown"] = cooldown_result
        all_results.append(cooldown_result)

        # Check 3: Market context (may return multiple results)
        market_results = self._check_market.execute(client_id, symbol)
        for result in market_results:
            gate_checks[result.gate_name] = result
            all_results.append(result)

        # Aggregate decision: ALL must pass
        allowed = all(result.passed for result in all_results)

        # Collect reasons (failures + informational messages)
        reasons = []
        for result in all_results:
            if not result.passed:
                reasons.append(f"❌ {result.message}")
            else:
                reasons.append(f"✅ {result.message}")

        # Create decision
        decision = EntryGateDecision(
            allowed=allowed,
            reasons=reasons,
            gate_checks=gate_checks,
            timestamp=timezone.now(),
            symbol=symbol,
            client_id=client_id,
            context=context,
        )

        # Save to audit trail
        self._decisions.save(decision)

        return decision
