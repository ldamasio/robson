"""Reporting models that capture aggregated analytics."""

from __future__ import annotations

from decimal import Decimal

from django.db import models

from .base import BaseTechnicalModel


class BaseReportModel(BaseTechnicalModel):
    """Shared behaviour for analytical reports."""

    name = models.CharField(max_length=255)
    generated_at = models.DateTimeField(auto_now_add=True)
    metadata = models.JSONField(
        default=dict,
        blank=True,
        help_text="Arbitrary data used to build the report",
    )

    class Meta:
        abstract = True
        ordering = ["-generated_at"]
        verbose_name = "Report"
        verbose_name_plural = "Reports"

    DEFAULT_NAME = "Report"

    def clean(self):
        if not self.name:
            self.name = self.DEFAULT_NAME
        if not self.type:
            self.type = "NEUTRAL"
        super().clean()


class AlocatedCapitalPercent(BaseReportModel):
    """Report that tracks the percentage of capital allocated to strategies."""

    DEFAULT_NAME = "Allocated Capital Percent"

    percentage = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=Decimal("0.00"),
        help_text="Percentage of capital currently allocated",
    )
    capital_amount = models.DecimalField(
        max_digits=20,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Capital amount represented by the percentage",
    )

    def clean(self):
        if self.percentage < 0:
            self.percentage = Decimal("0.00")
        if self.percentage > 100:
            self.percentage = Decimal("100.00")
        self.type = "NEUTRAL"
        self.name = self.name or self.DEFAULT_NAME
        super().clean()
