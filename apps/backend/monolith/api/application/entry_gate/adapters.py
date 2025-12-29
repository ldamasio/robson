"""
Adapters (Django implementations) for Entry Gate repositories.

These adapters connect the domain logic to Django ORM.
"""

from datetime import datetime
from decimal import Decimal
from typing import Optional, Tuple

from django.utils import timezone

from api.models.trading import Operation, Trade
from api.models.event_sourcing import StopEvent
from api.models.market_context import MetricPoint
from api.models.audit import BalanceSnapshot

from .domain import EntryGateConfig, EntryGateDecision


class DjangoPositionCountRepository:
    """Django implementation of PositionCountRepository."""

    def count_active_positions(self, client_id: int) -> int:
        """Count currently ACTIVE operations for a client."""
        return Operation.objects.filter(
            client_id=client_id,
            status='ACTIVE'
        ).count()


class DjangoMonthlyPnLRepository:
    """Django implementation of MonthlyPnLRepository."""

    def get_monthly_pnl(self, client_id: int) -> Tuple[Decimal, Decimal]:
        """
        Get monthly P&L and capital for dynamic position limit calculation.

        Returns:
            Tuple of (monthly_pnl, capital)
        """
        # Get monthly P&L (reuse existing logic)
        monthly_pnl = self._get_monthly_pnl_from_trades(client_id)

        # Get capital from latest balance snapshot
        capital = self._get_capital_from_balance_snapshot(client_id)

        return monthly_pnl, capital

    def _get_monthly_pnl_from_trades(self, client_id: int) -> Decimal:
        """
        Calculate current month's realized P&L from closed trades.

        This reuses the logic from risk_managed_trading.py:_get_monthly_pnl()
        """
        now = timezone.now()
        start_of_month = now.replace(day=1, hour=0, minute=0, second=0, microsecond=0)

        # Get all closed trades this month
        closed_trades = Trade.objects.filter(
            client_id=client_id,
            exit_price__isnull=False,
            exit_time__gte=start_of_month,
        )

        # Calculate total P&L
        total_pnl = Decimal("0")
        for trade in closed_trades:
            if trade.pnl:
                total_pnl += trade.pnl

        return total_pnl

    def _get_capital_from_balance_snapshot(self, client_id: int) -> Decimal:
        """
        Get current capital from latest balance snapshot.

        Falls back to a default if no snapshot exists.
        """
        # Get latest balance snapshot for SPOT account
        latest_snapshot = BalanceSnapshot.objects.filter(
            client_id=client_id,
            account_type='SPOT'
        ).order_by('-created_at').first()

        if latest_snapshot and latest_snapshot.total_value_usdc:
            return latest_snapshot.total_value_usdc

        # Fallback: use a default capital value
        # TODO: This should come from a TenantConfig or be calculated differently
        return Decimal("10000.00")  # Default $10k capital


class DjangoStopOutRepository:
    """Django implementation of StopOutRepository."""

    def get_latest_stop_out(self, client_id: int) -> Optional[datetime]:
        """Get timestamp of most recent stop-out event."""
        latest_stop = StopEvent.objects.filter(
            client_id=client_id,
            event_type='STOP_TRIGGERED'
        ).order_by('-occurred_at').first()

        return latest_stop.occurred_at if latest_stop else None


class DjangoMarketDataRepository:
    """Django implementation of MarketDataRepository."""

    def get_latest_funding_rate(self, client_id: int, symbol: str) -> Optional[Decimal]:
        """Get most recent funding rate for a symbol."""
        metric = MetricPoint.latest_for_metric(
            client_id=client_id,
            symbol=symbol,
            metric_name='funding_rate',
            source='binance_futures'
        )

        return metric.value if metric else None

    def get_data_age_seconds(self, client_id: int, symbol: str) -> Optional[int]:
        """Get age of most recent market data in seconds."""
        metric = MetricPoint.latest_for_metric(
            client_id=client_id,
            symbol=symbol,
            metric_name='funding_rate',
            source='binance_futures'
        )

        if metric:
            age = (timezone.now() - metric.created_at).total_seconds()
            return int(age)

        return None


class DjangoConfigRepository:
    """Django implementation of ConfigRepository."""

    def get_config(self, client_id: int) -> EntryGateConfig:
        """
        Get entry gate configuration for a client.

        Returns config from DB or defaults if not found.
        """
        from api.models.entry_gate import EntryGateConfig as EntryGateConfigModel

        try:
            config_model = EntryGateConfigModel.objects.get(client_id=client_id)
            return EntryGateConfig(
                enable_cooldown=config_model.enable_cooldown,
                cooldown_after_stop_seconds=config_model.cooldown_after_stop_seconds,
                enable_funding_rate_gate=config_model.enable_funding_rate_gate,
                funding_rate_threshold=config_model.funding_rate_threshold,
                enable_stale_data_gate=config_model.enable_stale_data_gate,
                max_data_age_seconds=config_model.max_data_age_seconds,
            )
        except EntryGateConfigModel.DoesNotExist:
            # Return default configuration
            return EntryGateConfig()


class DjangoDecisionRepository:
    """Django implementation of DecisionRepository."""

    def save(self, decision: EntryGateDecision) -> None:
        """
        Save entry gate decision to audit trail (append-only).
        """
        from api.models.entry_gate import EntryGateDecisionModel

        EntryGateDecisionModel.objects.create(
            client_id=decision.client_id,
            symbol=decision.symbol,
            allowed=decision.allowed,
            reasons=decision.reasons,
            gate_checks={name: result.to_dict() for name, result in decision.gate_checks.items()},
            context=decision.context,
        )
