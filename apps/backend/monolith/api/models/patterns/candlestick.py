"""Candlestick patterns metadata and detail storage."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class CandlestickPatternCode(models.TextChoices):
    HAMMER = "HAMMER", "Hammer"
    INVERTED_HAMMER = "INV_HAMMER", "Inverted Hammer"
    HANGING_MAN = "HANG_MAN", "Hanging Man"
    SHOOTING_STAR = "SHOOTING", "Shooting Star"
    DOJI_STANDARD = "DOJI_STD", "Doji"
    DOJI_LONG_LEGGED = "DOJI_LL", "Long-Legged Doji"
    DOJI_DRAGONFLY = "DOJI_DF", "Dragonfly Doji"
    DOJI_GRAVESTONE = "DOJI_GS", "Gravestone Doji"
    MARUBOZU = "MARUBOZU", "Marubozu"
    SPINNING_TOP = "SP_TOP", "Spinning Top"
    ENGULFING_BULL = "BULL_ENG", "Bullish Engulfing"
    ENGULFING_BEAR = "BEAR_ENG", "Bearish Engulfing"
    HARAMI_BULL = "BULL_HARAMI", "Bullish Harami"
    HARAMI_BEAR = "BEAR_HARAMI", "Bearish Harami"
    HARAMI_CROSS_BULL = "BULL_HX", "Bullish Harami Cross"
    HARAMI_CROSS_BEAR = "BEAR_HX", "Bearish Harami Cross"
    PIERCING_LINE = "PIERCING", "Piercing Line"
    DARK_CLOUD_COVER = "DARK_CLOUD", "Dark Cloud Cover"
    TWEEZER_TOP = "TW_TOP", "Tweezer Top"
    TWEEZER_BOTTOM = "TW_BOT", "Tweezer Bottom"
    KICKER_BULL = "KICK_BULL", "Bullish Kicker"
    KICKER_BEAR = "KICK_BEAR", "Bearish Kicker"
    THRUSTING = "THRUSTING", "Thrusting"
    MORNING_STAR = "MORNING", "Morning Star"
    EVENING_STAR = "EVENING", "Evening Star"
    THREE_WHITE_SOLDIERS = "3WS", "Three White Soldiers"
    THREE_BLACK_CROWS = "3BC", "Three Black Crows"
    THREE_INSIDE_UP = "3INS_UP", "Three Inside Up"
    THREE_INSIDE_DOWN = "3INS_DN", "Three Inside Down"
    RISING_THREE_METHODS = "R3M", "Rising Three Methods"
    FALLING_THREE_METHODS = "F3M", "Falling Three Methods"
    ABANDONED_BABY_BULL = "AB_BULL", "Bullish Abandoned Baby"
    ABANDONED_BABY_BEAR = "AB_BEAR", "Bearish Abandoned Baby"


class GapDirection(models.TextChoices):
    UP = "UP", "Gap Up"
    DOWN = "DOWN", "Gap Down"
    NONE = "NONE", "No Gap"


class RangeLocation(models.TextChoices):
    TOP = "TOP", "Top"
    MIDDLE = "MIDDLE", "Middle"
    BOTTOM = "BOTTOM", "Bottom"


class CandlestickPatternDetail(TimestampMixin):
    """Detailed candle metrics for candlestick detections."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="candlestick_detail",
    )
    candle_count = models.PositiveSmallIntegerField(default=1)
    body_pct_main = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    upper_wick_pct_main = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    lower_wick_pct_main = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    real_body_vs_atr = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    gap_direction = models.CharField(
        max_length=8,
        choices=GapDirection.choices,
        default=GapDirection.NONE,
    )
    prior_trend_len_bars = models.PositiveIntegerField(null=True, blank=True)
    prior_return_pct = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    location_in_range = models.CharField(
        max_length=8,
        choices=RangeLocation.choices,
        blank=True,
        default="",
    )
    volume_z = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    engulf_ratio = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    open_within_prev_body = models.BooleanField(default=False)
    close_vs_midpoint_prev = models.DecimalField(max_digits=6, decimal_places=2, null=True, blank=True)
    pattern_sequence = models.JSONField(default=list, blank=True)
    wick_ratio_sequence = models.JSONField(default=list, blank=True)
    color_sequence = models.JSONField(default=list, blank=True)

    class Meta:
        verbose_name = "Candlestick Pattern Detail"
        verbose_name_plural = "Candlestick Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Candlestick detail for instance {self.instance_id}"
