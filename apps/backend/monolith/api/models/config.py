"""Configuration models for trading rules and platform settings."""

from __future__ import annotations

from django.db import models

from .base import BaseConfigModel


class OnlyTradeReversal(BaseConfigModel):
    """Toggle that enforces trading only when reversal patterns confirm."""

    DEFAULT_NAME = "Only Trade Reversal"
    DEFAULT_DESCRIPTION = (
        "Reversals reinforce the trend of the opposing technical event within the chart pattern."
    )

    is_enabled = models.BooleanField(default=True)
    minimum_confirmation = models.PositiveIntegerField(
        default=1,
        help_text="Number of confirmations required before allowing the trade",
    )

    class Meta:
        verbose_name = "Only Trade Reversal Rule"
        verbose_name_plural = "Only Trade Reversal Rules"

    def clean(self):
        if not self.name:
            self.name = self.DEFAULT_NAME
        if not self.description:
            self.description = self.DEFAULT_DESCRIPTION
        super().clean()


class MaxTradePerDay(BaseConfigModel):
    """Defines how many trades a client can execute per day."""

    DEFAULT_NAME = "Max Trades Per Day"
    DEFAULT_DESCRIPTION = "Maximum number of trades allowed in a rolling 24h window."

    max_trades = models.PositiveIntegerField(default=3)

    class Meta:
        verbose_name = "Daily Trade Limit"
        verbose_name_plural = "Daily Trade Limits"

    def clean(self):
        if not self.name:
            self.name = self.DEFAULT_NAME
        if not self.description:
            self.description = self.DEFAULT_DESCRIPTION
        if self.max_trades == 0:
            self.max_trades = 1
        super().clean()
