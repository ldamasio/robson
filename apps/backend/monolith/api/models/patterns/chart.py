"""Chart pattern catalog helpers and instance details."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class ChartPatternCode(models.TextChoices):
    HEAD_AND_SHOULDERS = "HNS", "Head and Shoulders"
    INVERTED_HEAD_AND_SHOULDERS = "IHNS", "Inverted Head and Shoulders"
    DOUBLE_TOP = "DTOP", "Double Top"
    DOUBLE_BOTTOM = "DBOT", "Double Bottom"
    TRIPLE_TOP = "TRTOP", "Triple Top"
    TRIPLE_BOTTOM = "TRBOT", "Triple Bottom"
    ROUNDING_TOP = "RNTOP", "Rounding Top"
    ROUNDING_BOTTOM = "RNBOT", "Rounding Bottom"
    CUP_AND_HANDLE = "CUPH", "Cup and Handle"
    RECTANGLE = "RECT", "Rectangle"
    ASCENDING_TRIANGLE = "ATRIA", "Ascending Triangle"
    DESCENDING_TRIANGLE = "DTRIA", "Descending Triangle"
    SYMMETRICAL_TRIANGLE = "STRIA", "Symmetrical Triangle"
    RISING_WEDGE = "RWEDGE", "Rising Wedge"
    FALLING_WEDGE = "FWEDGE", "Falling Wedge"
    FLAG = "FLAG", "Flag"
    PENNANT = "PENNANT", "Pennant"
    CHANNEL_UP = "CHNU", "Ascending Channel"
    CHANNEL_DOWN = "CHND", "Descending Channel"
    CHANNEL_SIDEWAYS = "CHNS", "Sideways Channel"
    BROADENING = "BROAD", "Broadening Formation"
    DIAMOND_TOP = "DIAT", "Diamond Top"
    DIAMOND_BOTTOM = "DIAB", "Diamond Bottom"


class LineType(models.TextChoices):
    FLAT = "FLAT", "Flat"
    RISING = "RISING", "Rising"
    FALLING = "FALLING", "Falling"


class ChartPatternDetail(TimestampMixin):
    """High-resolution structure metrics for chart patterns."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="chart_detail",
    )
    upper_line_type = models.CharField(
        max_length=8,
        choices=LineType.choices,
        blank=True,
        default="",
    )
    lower_line_type = models.CharField(
        max_length=8,
        choices=LineType.choices,
        blank=True,
        default="",
    )
    apex_ts = models.DateTimeField(null=True, blank=True)
    apex_distance_bars = models.PositiveIntegerField(null=True, blank=True)
    channel_parallelism = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Parallelism score [0-1]",
    )
    height_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    range_compression_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    handle_depth_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    megaphone_divergence_rate = models.DecimalField(
        max_digits=6,
        decimal_places=3,
        null=True,
        blank=True,
    )
    touches_top = models.PositiveSmallIntegerField(default=0)
    touches_bottom = models.PositiveSmallIntegerField(default=0)
    symmetry_score = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Symmetry score [0-1]",
    )
    linearity_r2_top = models.DecimalField(max_digits=5, decimal_places=3, null=True, blank=True)
    linearity_r2_bottom = models.DecimalField(max_digits=5, decimal_places=3, null=True, blank=True)
    slope_top = models.DecimalField(max_digits=10, decimal_places=6, null=True, blank=True)
    slope_bottom = models.DecimalField(max_digits=10, decimal_places=6, null=True, blank=True)
    height_at_breakout = models.DecimalField(max_digits=10, decimal_places=4, null=True, blank=True)
    width_bars = models.PositiveIntegerField(null=True, blank=True)
    contraction_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    neckline_slope = models.DecimalField(max_digits=10, decimal_places=6, null=True, blank=True)
    head_prominence_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    shoulder_symmetry = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    peak_trough_distance_bars = models.PositiveIntegerField(null=True, blank=True)
    variance_between_peaks = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    curvature_score = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    duration_bars = models.PositiveIntegerField(null=True, blank=True)
    flag_pole_return_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    flag_slope = models.DecimalField(max_digits=10, decimal_places=6, null=True, blank=True)
    flag_ratio = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    channel_width_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    divergence_rate = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    swing_amplitude_growth = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    impulse_strength_z = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    trend_len_bars = models.PositiveIntegerField(null=True, blank=True)
    breakout_close_rel_band = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    post_breakout_atr_mult = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)

    class Meta:
        verbose_name = "Chart Pattern Detail"
        verbose_name_plural = "Chart Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Chart detail for instance {self.instance_id}"
