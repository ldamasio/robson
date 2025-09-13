from django.db import models
from .base import BaseTechnicalModel
from .analysis import Argument


class BaseFact(BaseTechnicalModel):
    """Base class for technical analysis facts tied to an Argument."""

    argument = models.ForeignKey(
        Argument, on_delete=models.CASCADE, related_name="%(class)ss"
    )
    name = models.CharField(max_length=255, blank=True, default="")

    class Meta:
        abstract = True


class Resistance(BaseFact):
    """Resistance level identified on the chart."""


class Support(BaseFact):
    """Support level identified on the chart."""


class Line(BaseFact):
    """Generic line fact (e.g., a drawn line of interest)."""


class TrendLine(BaseFact):
    """Trend line fact (uptrend or downtrend line)."""


class Channel(BaseFact):
    """Channel fact (price oscillating between two lines)."""


class Accumulation(BaseFact):
    """Accumulation zone fact."""


class Sideways(BaseFact):
    """Sideways/consolidation fact."""


class Breakout(BaseFact):
    """Breakout event fact."""


class Uptrend(BaseFact):
    """Uptrend fact."""


class Downtrend(BaseFact):
    """Downtrend fact."""

