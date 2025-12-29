"""
Port definitions (Repository interfaces) for Entry Gate system.

NO Django dependencies. Pure Protocol definitions.
"""

from datetime import datetime
from decimal import Decimal
from typing import Optional, Protocol, Tuple

from .domain import EntryGateConfig, EntryGateDecision


class PositionCountRepository(Protocol):
    """Repository for counting active positions."""

    def count_active_positions(self, client_id: int) -> int:
        """
        Count currently ACTIVE operations for a client.

        Args:
            client_id: Tenant identifier

        Returns:
            Number of active operations (status='ACTIVE')
        """
        ...


class MonthlyPnLRepository(Protocol):
    """Repository for monthly P&L calculation."""

    def get_monthly_pnl(self, client_id: int) -> Tuple[Decimal, Decimal]:
        """
        Get monthly P&L and capital for dynamic position limit calculation.

        Args:
            client_id: Tenant identifier

        Returns:
            Tuple of (monthly_pnl, capital)
            - monthly_pnl: Net P&L for current month (positive = profit, negative = loss)
            - capital: Current capital/balance for percentage calculations
        """
        ...


class StopOutRepository(Protocol):
    """Repository for querying stop-out events."""

    def get_latest_stop_out(self, client_id: int) -> Optional[datetime]:
        """
        Get timestamp of most recent stop-out event for cooldown check.

        Args:
            client_id: Tenant identifier

        Returns:
            Datetime of latest stop-out, or None if no stop-outs exist
        """
        ...


class MarketDataRepository(Protocol):
    """Repository for market context data (funding rate, data freshness)."""

    def get_latest_funding_rate(self, client_id: int, symbol: str) -> Optional[Decimal]:
        """
        Get most recent funding rate for a symbol.

        Args:
            client_id: Tenant identifier
            symbol: Trading pair (e.g., BTCUSDT)

        Returns:
            Latest funding rate, or None if no data available
        """
        ...

    def get_data_age_seconds(self, client_id: int, symbol: str) -> Optional[int]:
        """
        Get age of most recent market data in seconds.

        Args:
            client_id: Tenant identifier
            symbol: Trading pair (e.g., BTCUSDT)

        Returns:
            Age in seconds, or None if no data available
        """
        ...


class ConfigRepository(Protocol):
    """Repository for entry gate configuration."""

    def get_config(self, client_id: int) -> EntryGateConfig:
        """
        Get entry gate configuration for a client.

        Args:
            client_id: Tenant identifier

        Returns:
            EntryGateConfig (from DB or defaults)
        """
        ...


class DecisionRepository(Protocol):
    """Repository for persisting entry gate decisions (audit trail)."""

    def save(self, decision: EntryGateDecision) -> None:
        """
        Save entry gate decision to audit trail.

        Args:
            decision: EntryGateDecision to persist

        Note:
            Decisions are append-only (never updated or deleted).
        """
        ...
