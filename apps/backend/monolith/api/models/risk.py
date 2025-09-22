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
