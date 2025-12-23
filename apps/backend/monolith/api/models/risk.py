"""Risk management related models."""

from __future__ import annotations

from decimal import Decimal

from django.db import models

from .base import BaseConfigModel


class BaseRiskRule(BaseConfigModel):
    """Common structure for risk configuration rules."""

    risk_percentage = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=Decimal("0.00"),
        help_text="Percentage of capital impacted by the rule",
    )
    max_capital_amount = models.DecimalField(
        max_digits=20,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Optional hard cap in monetary units",
    )

    class Meta:
        abstract = True
        verbose_name = "Risk Rule"
        verbose_name_plural = "Risk Rules"

    DEFAULT_NAME = "Risk Rule"
    DEFAULT_DESCRIPTION = "Generic risk configuration"

    def clean(self):
        if not self.name:
            self.name = self.DEFAULT_NAME
        if not self.description:
            self.description = self.DEFAULT_DESCRIPTION
        if self.risk_percentage < 0:
            self.risk_percentage = Decimal("0.00")
        super().clean()


class OnePercentOfCapital(BaseRiskRule):
    """Classic 1% of capital risk rule."""

    DEFAULT_NAME = "One Percent Of Capital"
    DEFAULT_DESCRIPTION = "Limit each trade exposure to one percent of available capital."

    def clean(self):
        self.risk_percentage = Decimal("1.00")
        super().clean()


class JustBet4percent(BaseRiskRule):
    """Legacy rule allowing up to 4% per trade (kept for completeness)."""

    DEFAULT_NAME = "Just Bet 4 Percent"
    DEFAULT_DESCRIPTION = "Allow up to four percent of capital to be allocated to a single position."

    def clean(self):
        self.risk_percentage = Decimal("4.00")
        super().clean()


class PolicyState(BaseConfigModel):
    """
    Tracks risk policy state for a client in a given month.

    This model maintains the monthly risk limits and trading statistics,
    implementing the 4% monthly drawdown rule and daily trade limits.

    Key Principle: One PolicyState per client per month.
    Each month starts fresh with reset limits.

    Status flow:
    - ACTIVE: Trading allowed
    - PAUSED: Automatically paused due to risk limit hit
    - SUSPENDED: Manually suspended by administrator
    """

    STATUS_CHOICES = [
        ("ACTIVE", "Active"),
        ("PAUSED", "Paused"),
        ("SUSPENDED", "Suspended"),
    ]

    # Month identifier (format: "YYYY-MM")
    month = models.CharField(
        max_length=7,
        db_index=True,
        help_text="Month in YYYY-MM format (e.g., 2025-12)",
    )

    # Policy status
    status = models.CharField(
        max_length=20,
        choices=STATUS_CHOICES,
        default="ACTIVE",
        db_index=True,
    )

    # Capital tracking
    starting_capital = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Capital at the start of the month",
    )
    current_capital = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Current capital (includes unrealized P&L)",
    )

    # P&L tracking
    realized_pnl = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Realized profit/loss for the month",
    )
    unrealized_pnl = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Unrealized profit/loss from open positions",
    )

    # Trade statistics
    total_trades = models.IntegerField(
        default=0,
        help_text="Total trades executed this month",
    )
    winning_trades = models.IntegerField(
        default=0,
        help_text="Number of winning trades",
    )
    losing_trades = models.IntegerField(
        default=0,
        help_text="Number of losing trades",
    )

    # Risk limits
    max_drawdown_percent = models.DecimalField(
        max_digits=10,
        decimal_places=2,
        default=Decimal("4.0"),
        help_text="Maximum monthly drawdown percentage",
    )
    max_trades_per_day = models.IntegerField(
        default=50,
        help_text="Maximum trades per day (medium-frequency limit)",
    )

    # Pause tracking
    paused_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text="When the policy was paused",
    )
    pause_reason = models.TextField(
        blank=True,
        null=True,
        help_text="Reason for pausing trading",
    )

    class Meta:
        verbose_name = "Policy State"
        verbose_name_plural = "Policy States"
        unique_together = [["client", "month"]]
        indexes = [
            models.Index(fields=["client", "month"]),
            models.Index(fields=["status", "month"]),
        ]
        ordering = ["-month", "client"]

    def __str__(self):
        return f"Policy {self.client.name if self.client else 'No Client'} - {self.month} ({self.status})"

    # Computed properties
    @property
    def total_pnl(self) -> Decimal:
        """Total P&L (realized + unrealized)."""
        return self.realized_pnl + self.unrealized_pnl

    @property
    def drawdown_percent(self) -> Decimal:
        """Current drawdown as % of starting capital."""
        if self.starting_capital == 0:
            return Decimal("0")
        drawdown = self.starting_capital - self.current_capital
        return (drawdown / self.starting_capital) * Decimal("100")

    @property
    def win_rate(self) -> float:
        """Win rate as percentage."""
        if self.total_trades == 0:
            return 0.0
        return float((self.winning_trades / self.total_trades) * 100)

    @property
    def is_active(self) -> bool:
        """Check if trading is currently allowed."""
        return self.status == "ACTIVE"

    @property
    def is_paused(self) -> bool:
        """Check if trading is paused."""
        return self.status == "PAUSED"

    @property
    def has_hit_drawdown_limit(self) -> bool:
        """Check if monthly drawdown limit has been exceeded."""
        return self.drawdown_percent >= self.max_drawdown_percent

    # State management
    def pause_trading(self, reason: str):
        """Pause trading due to risk limit violation."""
        from django.utils import timezone

        self.status = "PAUSED"
        self.paused_at = timezone.now()
        self.pause_reason = reason
        self.save(update_fields=["status", "paused_at", "pause_reason", "updated_at"])

    def resume_trading(self):
        """Resume trading (manually)."""
        self.status = "ACTIVE"
        self.paused_at = None
        self.pause_reason = None
        self.save(update_fields=["status", "paused_at", "pause_reason", "updated_at"])

    def suspend_trading(self, reason: str):
        """Manually suspend trading (admin action)."""
        from django.utils import timezone

        self.status = "SUSPENDED"
        self.paused_at = timezone.now()
        self.pause_reason = reason
        self.save(update_fields=["status", "paused_at", "pause_reason", "updated_at"])

    # Trade recording
    def record_trade(self, pnl: Decimal, is_winner: bool):
        """
        Record a completed trade and update statistics.

        Args:
            pnl: Realized profit/loss from the trade
            is_winner: Whether the trade was profitable
        """
        self.total_trades += 1
        if is_winner:
            self.winning_trades += 1
        else:
            self.losing_trades += 1

        self.realized_pnl += pnl
        self.current_capital += pnl

        # Auto-pause if drawdown limit exceeded
        if self.has_hit_drawdown_limit and self.is_active:
            self.pause_trading(
                f"Monthly drawdown limit exceeded: {self.drawdown_percent:.2f}% >= {self.max_drawdown_percent}%"
            )
        else:
            self.save(update_fields=[
                "total_trades",
                "winning_trades",
                "losing_trades",
                "realized_pnl",
                "current_capital",
                "updated_at",
            ])

    def update_unrealized_pnl(self, unrealized_pnl: Decimal):
        """Update unrealized P&L from open positions."""
        self.unrealized_pnl = unrealized_pnl
        self.current_capital = self.starting_capital + self.realized_pnl + unrealized_pnl

        # Check if drawdown limit exceeded
        if self.has_hit_drawdown_limit and self.is_active:
            self.pause_trading(
                f"Monthly drawdown limit exceeded: {self.drawdown_percent:.2f}% >= {self.max_drawdown_percent}%"
            )
        else:
            self.save(update_fields=["unrealized_pnl", "current_capital", "updated_at"])

    def clean(self):
        """Validate business rules."""
        super().clean()

        from django.core.exceptions import ValidationError

        # Validate month format
        if self.month:
            import re
            if not re.match(r"^\d{4}-\d{2}$", self.month):
                raise ValidationError("Month must be in YYYY-MM format")

        # Validate capital is positive
        if self.starting_capital and self.starting_capital <= 0:
            raise ValidationError("Starting capital must be positive")

        # Validate drawdown percentage
        if self.max_drawdown_percent < 0:
            raise ValidationError("Max drawdown percentage must be non-negative")

        # Validate trades per day
        if self.max_trades_per_day < 1:
            raise ValidationError("Max trades per day must be at least 1")
