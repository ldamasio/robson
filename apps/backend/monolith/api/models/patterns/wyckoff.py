"""Wyckoff/volume pattern helpers."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class WyckoffPatternCode(models.TextChoices):
    ACCUMULATION = "ACC", "Accumulation"
    DISTRIBUTION = "DIST", "Distribution"
    SPRING = "SPRING", "Spring"
    UPTHRUST_AFTER_DISTRIBUTION = "UTAD", "Upthrust After Distribution"
    CLIMAX = "CLIMAX", "Volume Climax"
    EXHAUSTION = "EXHAUST", "Volume Exhaustion"


class WyckoffPhase(models.TextChoices):
    A = "A", "Phase A"
    B = "B", "Phase B"
    C = "C", "Phase C"
    D = "D", "Phase D"
    E = "E", "Phase E"


class WyckoffEvent(models.TextChoices):
    SC = "SC", "Selling Climax"
    AR = "AR", "Automatic Rally"
    ST = "ST", "Secondary Test"
    SPRING = "SPRING", "Spring"
    TEST = "TEST", "Test"
    SOS = "SOS", "Sign of Strength"
    LPS = "LPS", "Last Point of Support"
    UT = "UT", "Upthrust"
    UTAD = "UTAD", "Upthrust After Distribution"
    OTHER = "OTHER", "Other"


class WyckoffPatternDetail(TimestampMixin):
    """Phase, event, and volume context for Wyckoff detections."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="wyckoff_detail",
    )
    phase = models.CharField(
        max_length=1,
        choices=WyckoffPhase.choices,
        blank=True,
        default="",
    )
    event = models.CharField(
        max_length=8,
        choices=WyckoffEvent.choices,
        blank=True,
        default="",
    )
    trading_range_top = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    trading_range_bottom = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    spring_depth_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    upthrust_depth_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    climactic_volume = models.BooleanField(default=False)
    effort_vs_result_z = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    pf_count_projection = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    notes = models.TextField(blank=True, default="")

    class Meta:
        verbose_name = "Wyckoff Pattern Detail"
        verbose_name_plural = "Wyckoff Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Wyckoff detail for instance {self.instance_id}"
