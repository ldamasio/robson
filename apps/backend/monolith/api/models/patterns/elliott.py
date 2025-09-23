"""Elliott wave catalog helpers and validation flags."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class ElliottPatternCode(models.TextChoices):
    IMPULSE = "IMPULSE", "Impulse"
    LEADING_DIAGONAL = "LEAD_DIAG", "Leading Diagonal"
    ENDING_DIAGONAL = "END_DIAG", "Ending Diagonal"
    ZIGZAG = "ZIGZAG", "Zigzag"
    FLAT_REGULAR = "FLAT_REG", "Flat (Regular)"
    FLAT_EXPANDED = "FLAT_EXP", "Flat (Expanded)"
    CONTRACTING_TRIANGLE = "CTR_TRI", "Contracting Triangle"
    EXPANDING_TRIANGLE = "EXP_TRI", "Expanding Triangle"
    COMBINATION = "COMBO", "Combination"


class ElliottDegree(models.TextChoices):
    PRIMARY = "PRIMARY", "Primary"
    INTERMEDIATE = "INTERMEDIATE", "Intermediate"
    MINOR = "MINOR", "Minor"
    MINUETTE = "MINUETTE", "Minuette"
    SUB_MINUETTE = "SUB_MINUETTE", "Sub-Minuette"


class ElliottPatternDetail(TimestampMixin):
    """Elliott wave rule checks and ratio tracking."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="elliott_detail",
    )
    degree = models.CharField(
        max_length=16,
        choices=ElliottDegree.choices,
        blank=True,
        default="",
    )
    wave_labels = models.JSONField(
        default=list,
        blank=True,
        help_text="Ordered labels for detected waves (1-5, A-C)",
    )
    rule_2_not_exceed_1_start = models.BooleanField(default=True)
    rule_3_not_shortest = models.BooleanField(default=True)
    overlap_rule_ok = models.BooleanField(default=True)
    wave3_extension = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    wave5_extension = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    abc_ratios = models.JSONField(default=dict, blank=True)
    subwave_confidence = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    count_score = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    features = models.JSONField(default=dict, blank=True)

    class Meta:
        verbose_name = "Elliott Pattern Detail"
        verbose_name_plural = "Elliott Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Elliott detail for instance {self.instance_id}"
