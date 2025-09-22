# api/models/indicators.py

from django.db import models
from decimal import Decimal
from .base import BaseTechnicalModel
from .trading import Symbol


class StatisticalIndicator(BaseTechnicalModel):
    """Base class for statistical indicators (e.g., MA, RSI, MACD)."""

    timeframe = models.CharField(max_length=8, default="1h")

    class Meta:
        abstract = True


class MovingAverage(StatisticalIndicator):
    """Simple moving average (SMA) or generic MA representation."""

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE, related_name="indicators")
    period = models.IntegerField()
    value = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))


class RSIIndicator(StatisticalIndicator):
    """Relative Strength Index."""

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE, related_name="rsi_indicators")
    period = models.IntegerField()
    value = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))


class MACDIndicator(StatisticalIndicator):
    """Moving Average Convergence Divergence."""

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE, related_name="macd_indicators")
    fast_period = models.IntegerField()
    slow_period = models.IntegerField()
    signal_period = models.IntegerField()
    macd = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    signal = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    histogram = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))


class RelativeStrengthIndex(RSIIndicator):
    """Compatibility proxy for the legacy RelativeStrengthIndex model."""

    class Meta:
        proxy = True
        verbose_name = "Relative Strength Index"
        verbose_name_plural = "Relative Strength Index"


class MovingAverageConvergenceDivergence(MACDIndicator):
    """Compatibility proxy for the legacy MACD model name."""

    class Meta:
        proxy = True
        verbose_name = "Moving Average Convergence Divergence"
        verbose_name_plural = "Moving Average Convergence Divergence"


class BollingerBands(StatisticalIndicator):
    """Bollinger Bands indicator values."""

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE, related_name="bollinger_bands")
    period = models.IntegerField()
    standard_deviations = models.DecimalField(max_digits=5, decimal_places=2, default=Decimal("2.00"))
    upper_band = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    middle_band = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    lower_band = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))


class StochasticOscillator(StatisticalIndicator):
    """Stochastic Oscillator indicator (fast %K/%D values)."""

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE, related_name="stochastic_oscillators")
    period = models.IntegerField(default=14)
    k_value = models.DecimalField(max_digits=5, decimal_places=2, default=Decimal("0"))
    d_value = models.DecimalField(max_digits=5, decimal_places=2, default=Decimal("0"))
    slow_d_value = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=Decimal("0"),
        help_text="Optional slow %D value",
    )
