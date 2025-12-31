"""
Pattern to Plan Integration.

Bridges CONFIRMED patterns with the Agentic Workflow (PLAN → VALIDATE → EXECUTE).

When a pattern is CONFIRMED:
1. Check StrategyPatternConfig for auto-entry rules
2. Create TradingIntent with pattern-derived parameters
3. Submit to agentic workflow for validation and execution
"""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from typing import TYPE_CHECKING

from api.models.patterns.base import PatternInstance, PatternStatus

if TYPE_CHECKING:
    from api.models.patterns.strategy_config import StrategyPatternConfig


@dataclass(frozen=True)
class PatternToPlanCommand:
    """Command to convert a CONFIRMED pattern to a trading plan."""

    pattern_instance_id: int
    strategy_config_id: int | None = None  # If None, finds all matching configs
    force_create: bool = False  # Create plan even if auto_entry disabled


@dataclass(frozen=True)
class PatternToPlanResult:
    """Result of pattern-to-plan conversion."""

    success: bool
    plans_created: int
    strategies_matched: int
    errors: list[str]
    details: list[dict]


class PatternToPlanUseCase:
    """
    Convert CONFIRMED patterns to trading plans via agentic workflow.

    Flow:
    1. Find active StrategyPatternConfig entries for the pattern
    2. Validate pattern meets config criteria (confidence, confirmation, etc.)
    3. Create TradingIntent for each matching strategy
    4. Return results (does NOT execute - only creates plans)
    """

    def __init__(self):
        """Initialize use case."""
        pass

    def execute(self, command: PatternToPlanCommand) -> PatternToPlanResult:
        """
        Execute pattern-to-plan conversion.

        Args:
            command: PatternToPlanCommand

        Returns:
            PatternToPlanResult with details of created plans
        """
        from api.models.patterns.strategy_config import StrategyPatternConfig

        # Get pattern instance
        try:
            pattern_instance = PatternInstance.objects.select_related(
                "pattern", "symbol"
            ).get(id=command.pattern_instance_id)
        except PatternInstance.DoesNotExist:
            return PatternToPlanResult(
                success=False,
                plans_created=0,
                strategies_matched=0,
                errors=["PatternInstance not found"],
                details=[],
            )

        # Only process CONFIRMED patterns
        if pattern_instance.status != PatternStatus.CONFIRMED:
            return PatternToPlanResult(
                success=False,
                plans_created=0,
                strategies_matched=0,
                errors=[f"Pattern status is {pattern_instance.status}, not CONFIRMED"],
                details=[],
            )

        # Get matching strategy configs
        pattern_code = pattern_instance.pattern.pattern_code
        symbol_name = pattern_instance.symbol.name
        timeframe = pattern_instance.timeframe

        if command.strategy_config_id:
            # Use specific config
            configs = StrategyPatternConfig.objects.filter(
                id=command.strategy_config_id,
                pattern__pattern_code=pattern_code,
                strategy__is_active=True,
            )
        else:
            # Find all matching configs
            configs = StrategyPatternConfig.get_active_configs_for_pattern(
                pattern_code=pattern_code,
                symbol=symbol_name,
                timeframe=timeframe,
            )

        if not configs:
            return PatternToPlanResult(
                success=True,
                plans_created=0,
                strategies_matched=0,
                errors=["No active strategy configs found for this pattern"],
                details=[],
            )

        # Process each config
        plans_created = 0
        errors = []
        details = []

        for config in configs:
            # Check if config should trigger entry
            if not command.force_create and not config.auto_entry_enabled:
                errors.append(
                    f"Strategy '{config.strategy.name}': auto_entry disabled"
                )
                continue

            if not config.should_trigger_entry(pattern_instance):
                errors.append(
                    f"Strategy '{config.strategy.name}': does not meet entry criteria"
                )
                continue

            # Create trading plan
            try:
                plan = self._create_trading_plan(pattern_instance, config)
                plans_created += 1
                details.append({
                    "strategy_id": config.strategy.id,
                    "strategy_name": config.strategy.name,
                    "pattern_instance_id": pattern_instance.id,
                    "pattern_code": pattern_code,
                    "plan_id": plan.id if hasattr(plan, 'id') else None,
                    "entry_mode": config.entry_mode,
                })
            except Exception as e:
                errors.append(
                    f"Strategy '{config.strategy.name}': {str(e)}"
                )

        return PatternToPlanResult(
            success=plans_created > 0,
            plans_created=plans_created,
            strategies_matched=len(configs),
            errors=errors,
            details=details,
        )

    def _create_trading_plan(
        self,
        pattern_instance: PatternInstance,
        config: StrategyPatternConfig,
    ):
        """
        Create a trading plan from pattern instance.

        Extracts trade parameters from pattern evidence and creates TradingIntent.

        Args:
            pattern_instance: CONFIRMED pattern instance
            config: StrategyPatternConfig

        Returns:
            Created plan/intent object
        """
        from api.application.domain import TradingIntent, Side, Symbol
        from api.models.patterns.base import PatternDirectionBias

        # Determine side from pattern bias
        bias = pattern_instance.pattern.direction_bias

        if bias == PatternDirectionBias.BULLISH:
            side = Side.BUY
        elif bias == PatternDirectionBias.BEARISH:
            side = Side.SELL
        else:
            # Neutral patterns default to BUY (can be overridden in config)
            side = Side.BUY

        # Extract entry/stop/target from pattern evidence
        evidence = pattern_instance.features.get("evidence", {}) if pattern_instance.features else {}

        # Entry price: current close (or breakout price for chart patterns)
        entry_price = evidence.get("confirmation_price") or evidence.get("breakout_price")

        # Stop loss: pattern invalidation level
        stop_price = evidence.get("invalidation_price")

        # Take profit: measured move target (if available)
        target_price = evidence.get("target_price")

        # Create TradingIntent
        # Note: This integrates with the existing agentic workflow
        # The actual Plan creation happens in the agentic workflow layer
        intent = TradingIntent(
            symbol=Symbol.from_pair(pattern_instance.symbol.name),
            side=side,
            entry_price=Decimal(str(entry_price)) if entry_price else None,
            stop_price=Decimal(str(stop_price)) if stop_price else None,
            target_price=Decimal(str(target_price)) if target_price else None,
            strategy_id=config.strategy.id,
            confidence=Decimal(str(
                pattern_instance.features.get("confidence", 0.7)
            )) if pattern_instance.features else Decimal("0.7"),
            metadata={
                "source": "pattern_detection",
                "pattern_instance_id": pattern_instance.id,
                "pattern_code": pattern_instance.pattern.pattern_code,
                "timeframe": pattern_instance.timeframe,
                "entry_mode": config.entry_mode,
            },
        )

        # Store intent for later processing
        # (In a complete implementation, this would be persisted)
        return intent


class PatternAlertProcessor:
    """
    Processes pattern alerts and triggers pattern-to-plan conversion.

    Listens for CONFIRM alerts and initiates plan creation.
    """

    def __init__(self):
        """Initialize processor."""
        self._use_case = PatternToPlanUseCase()

    def process_confirmed_alert(self, alert_id: int) -> PatternToPlanResult:
        """
        Process a CONFIRM alert and create trading plans.

        Args:
            alert_id: PatternAlert ID

        Returns:
            PatternToPlanResult
        """
        from api.models.patterns.base import PatternAlert

        try:
            alert = PatternAlert.objects.select_related(
                "instance__pattern", "instance__symbol"
            ).get(id=alert_id)
        except PatternAlert.DoesNotExist:
            return PatternToPlanResult(
                success=False,
                plans_created=0,
                strategies_matched=0,
                errors=["Alert not found"],
                details=[],
            )

        if alert.alert_type != PatternAlert.AlertType.CONFIRM:
            return PatternToPlanResult(
                success=False,
                plans_created=0,
                strategies_matched=0,
                errors=[f"Alert type is {alert.alert_type}, not CONFIRM"],
                details=[],
            )

        # Create command and execute
        command = PatternToPlanCommand(
            pattern_instance_id=alert.instance_id,
        )

        return self._use_case.execute(command)

    def process_recent_confirmed_alerts(self, minutes: int = 5) -> list[PatternToPlanResult]:
        """
        Process all recent CONFIRM alerts within time window.

        Args:
            minutes: Time window in minutes

        Returns:
            List of PatternToPlanResult objects
        """
        from django.utils import timezone
        from datetime import timedelta

        cutoff = timezone.now() - timedelta(minutes=minutes)

        alerts = PatternAlert.objects.filter(
            alert_type=PatternAlert.AlertType.CONFIRM,
            alert_ts__gte=cutoff,
            processed_for_plan=False,  # Assuming we add this flag
        )

        results = []
        for alert in alerts:
            result = self.process_confirmed_alert(alert.id)
            results.append(result)

            # Mark as processed (if flag exists)
            # alert.processed_for_plan = True
            # alert.save(update_fields=["processed_for_plan"])

        return results
