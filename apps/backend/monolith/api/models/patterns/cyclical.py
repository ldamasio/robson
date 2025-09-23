"""Cyclical/seasonal pattern helpers."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class CyclicalPatternCode(models.TextChoices):
    SELL_IN_MAY = "SELL_MAY", "Sell in May"
    TURN_OF_MONTH = "TOM", "Turn of Month"
    SANTA_RALLY = "SANTA", "Santa Rally"
    PRESIDENTIAL_CYCLE = "PRES", "Presidential Cycle"
    DAY_OF_WEEK = "DOW", "Day of Week"
    HOLIDAY_DRIFT = "HOL_DRIFT", "Holiday Drift"
    FIB_TIME_ZONE = "FIB_TIME", "Fibonacci Time Zone"


class RegimeDependency(models.TextChoices):
    NONE = "NONE", "None"
    WEAK = "WEAK", "Weak"
    STRONG = "STRONG", "Strong"


class CyclicalPatternDetail(TimestampMixin):
    """Windowing stats for cyclical signals."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="cyclical_detail",
    )
    window_start_ts = models.DateTimeField(null=True, blank=True)
    window_end_ts = models.DateTimeField(null=True, blank=True)
    cycle_length_bars = models.PositiveIntegerField(null=True, blank=True)
    historical_hit_rate = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    avg_return_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    p_value = models.DecimalField(max_digits=6, decimal_places=4, null=True, blank=True)
    regime_dependency = models.CharField(
        max_length=8,
        choices=RegimeDependency.choices,
        default=RegimeDependency.NONE,
    )
    event_tag = models.CharField(max_length=64, blank=True, default="")
    notes = models.TextField(blank=True, default="")

    class Meta:
        verbose_name = "Cyclical Pattern Detail"
        verbose_name_plural = "Cyclical Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Cyclical detail for instance {self.instance_id}"
