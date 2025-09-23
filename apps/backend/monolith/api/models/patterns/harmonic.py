"""Harmonic pattern catalog helpers and ratio storage."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class HarmonicPatternCode(models.TextChoices):
    GARTLEY = "GARTLEY", "Gartley"
    BAT = "BAT", "Bat"
    BUTTERFLY = "BUTTERFLY", "Butterfly"
    CRAB = "CRAB", "Crab"
    DEEP_CRAB = "DEEP_CRAB", "Deep Crab"
    SHARK = "SHARK", "Shark"
    CYPHER = "CYPHER", "Cypher"


class HarmonicPatternDetail(TimestampMixin):
    """Precise leg ratios and PRZ data for harmonic detections."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="harmonic_detail",
    )
    ratio_ab_xa = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    ratio_bc_ab = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    ratio_cd_bc = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    ratio_ad_xa = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    ratio_tolerance_pct = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    prz_min = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    prz_max = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    prz_width_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    confluence_score = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    completion_ts = models.DateTimeField(null=True, blank=True)
    completion_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    completion_distance_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    symmetry_score = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    time_ratio_alignment = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    volume_confirmation = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    features = models.JSONField(default=dict, blank=True)

    class Meta:
        verbose_name = "Harmonic Pattern Detail"
        verbose_name_plural = "Harmonic Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Harmonic detail for instance {self.instance_id}"
