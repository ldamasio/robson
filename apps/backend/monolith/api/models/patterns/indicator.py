"""Indicator-driven pattern helpers."""

from django.db import models

from ..base import TimestampMixin
from .base import PatternInstance


class IndicatorPatternCode(models.TextChoices):
    RSI_BULLISH_DIVERGENCE = "RSI_BULL_DIV", "RSI Bullish Divergence"
    RSI_BEARISH_DIVERGENCE = "RSI_BEAR_DIV", "RSI Bearish Divergence"
    RSI_FAILURE_SWING = "RSI_FAIL", "RSI Failure Swing"
    MACD_SIGNAL_CROSS_UP = "MACD_SIG_UP", "MACD Signal Cross Up"
    MACD_SIGNAL_CROSS_DOWN = "MACD_SIG_DN", "MACD Signal Cross Down"
    MACD_ZERO_LINE_CROSS = "MACD_ZERO", "MACD Zero-Line Cross"
    MACD_HIST_CONVERGENCE = "MACD_HIST_CONV", "MACD Histogram Convergence"
    MACD_HIST_DIVERGENCE = "MACD_HIST_DIV", "MACD Histogram Divergence"
    STOCH_CROSS_UP = "STOCH_UP", "Stochastic Bullish Cross"
    STOCH_CROSS_DOWN = "STOCH_DN", "Stochastic Bearish Cross"
    OBV_DIVERGENCE_BULL = "OBV_DIV_BULL", "OBV Bullish Divergence"
    OBV_DIVERGENCE_BEAR = "OBV_DIV_BEAR", "OBV Bearish Divergence"
    MOMENTUM_BREAKOUT = "MOMO_BREAK", "Momentum Breakout"


class IndicatorType(models.TextChoices):
    RSI = "RSI", "RSI"
    MACD = "MACD", "MACD"
    STOCH = "STOCH", "Stochastic"
    OBV = "OBV", "On-Balance Volume"
    MOMENTUM = "MOMENTUM", "Momentum"


class IndicatorPatternDetail(TimestampMixin):
    """Indicator snapshots at the moment of detection."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="indicator_detail",
    )
    indicator_type = models.CharField(
        max_length=16,
        choices=IndicatorType.choices,
        blank=True,
        default="",
    )
    rsi_value = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    rsi_divergence = models.CharField(max_length=8, blank=True, default="")
    failure_swing = models.BooleanField(default=False)
    macd = models.DecimalField(max_digits=7, decimal_places=4, null=True, blank=True)
    macd_signal = models.DecimalField(max_digits=7, decimal_places=4, null=True, blank=True)
    macd_hist = models.DecimalField(max_digits=7, decimal_places=4, null=True, blank=True)
    macd_cross = models.CharField(max_length=8, blank=True, default="")
    stoch_k = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    stoch_d = models.DecimalField(max_digits=5, decimal_places=2, null=True, blank=True)
    stoch_cross = models.CharField(max_length=8, blank=True, default="")
    obv_slope = models.DecimalField(max_digits=7, decimal_places=4, null=True, blank=True)
    momentum_z = models.DecimalField(max_digits=6, decimal_places=3, null=True, blank=True)
    features = models.JSONField(default=dict, blank=True)

    class Meta:
        verbose_name = "Indicator Pattern Detail"
        verbose_name_plural = "Indicator Pattern Details"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Indicator detail for instance {self.instance_id}"
