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
