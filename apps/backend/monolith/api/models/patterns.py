# api/models/patterns.py

from django.db import models

from .base import BaseTechnicalModel


class ChartPattern(BaseTechnicalModel):
    """Base class for chart patterns."""

    name = models.CharField(max_length=100)
    reliability = models.DecimalField(max_digits=5, decimal_places=2)

    class Meta:
        abstract = True


class Rectangle(ChartPattern):
    """Rectangle chart pattern."""

    width = models.DecimalField(max_digits=10, decimal_places=4)
    height = models.DecimalField(max_digits=10, decimal_places=4)


class Triangle(ChartPattern):
    """Triangle chart pattern."""

    TRIANGLE_TYPES = [
        ("ASCENDING", "Ascending"),
        ("DESCENDING", "Descending"),
        ("SYMMETRICAL", "Symmetrical"),
    ]

    triangle_type = models.CharField(max_length=20, choices=TRIANGLE_TYPES)


DEFAULT_STAR_SEQUENCE = ["CLOSE", "OPEN", "CLOSE"]


class CandlestickPattern(ChartPattern):
    """Base class for single and multi-candle candlestick patterns."""

    CATEGORIES = [
        ("REVERSAL", "Reversal"),
        ("CONTINUATION", "Continuation"),
        ("NEUTRAL", "Neutral"),
    ]

    pattern_category = models.CharField(max_length=20, choices=CATEGORIES, default="NEUTRAL")
    confirmation_required = models.BooleanField(default=False)
    notes = models.TextField(blank=True, default="")

    pattern_name: str = ""
    DEFAULT_CATEGORY = "NEUTRAL"
    DEFAULT_DESCRIPTION = ""
    DEFAULT_CONFIRMATION_REQUIRED = False

    class Meta:
        abstract = True

    def clean(self):
        if not self.name:
            self.name = self.pattern_name or self.__class__.__name__.replace("_", " ")
        if not self.description:
            default_desc = self.DEFAULT_DESCRIPTION or f"Details for the {self.name} pattern."
            self.description = default_desc
        if not self.pattern_category:
            self.pattern_category = self.DEFAULT_CATEGORY
        if (
            getattr(self, "_state", None)
            and getattr(self._state, "adding", False)
            and not self.confirmation_required
            and self.DEFAULT_CONFIRMATION_REQUIRED
        ):
            self.confirmation_required = True
        super().clean()


class ReversalPattern(CandlestickPattern):
    """Specialisation for reversal candlestick patterns."""

    DEFAULT_CATEGORY = "REVERSAL"


class Hammer(ReversalPattern):
    """Hammer candlestick pattern."""

    pattern_name = "Hammer"
    DEFAULT_DESCRIPTION = "Single-candle reversal pattern where price rejects lower prices."


class InvertedHammer(ReversalPattern):
    """Inverted hammer candlestick pattern."""

    pattern_name = "Inverted Hammer"
    DEFAULT_DESCRIPTION = "Single-candle reversal pattern indicating bullish reversal after a downtrend."


class HangingMan(ReversalPattern):
    """Hanging man candlestick pattern."""

    pattern_name = "Hanging Man"
    DEFAULT_DESCRIPTION = "Bearish reversal signal after an advance, visually similar to a hammer."


class Piercing(ReversalPattern):
    """Piercing line two-candle reversal pattern."""

    pattern_name = "Piercing"
    DEFAULT_DESCRIPTION = "Two-candle bullish reversal where the second candle closes above the midpoint of the first."
    DEFAULT_CONFIRMATION_REQUIRED = True


class Engulfing(ReversalPattern):
    """Engulfing reversal pattern."""

    pattern_name = "Engulfing"
    DEFAULT_DESCRIPTION = "Two-candle reversal where the second candle fully engulfs the previous body."
    DEFAULT_CONFIRMATION_REQUIRED = True


class ShootingStar(ReversalPattern):
    """Shooting star candlestick pattern."""

    pattern_name = "Shooting Star"
    DEFAULT_DESCRIPTION = "Bearish reversal pattern characterised by a small body and long upper wick."
    candle_sequence = models.JSONField(
        default=list,
        blank=True,
        help_text="Optional candle sequence details (open/high/low/close order).",
    )

    def clean(self):
        if not self.candle_sequence:
            self.candle_sequence = ["OPEN", "HIGH", "LOW", "CLOSE"]
        super().clean()


class MorningStar(ReversalPattern):
    """Morning star three-candle reversal pattern."""

    pattern_name = "Morning Star"
    DEFAULT_DESCRIPTION = "Three-candle bullish reversal pattern occurring after a downtrend."
    candle_sequence = models.JSONField(
        default=list,
        help_text="Chronological candle sequence key points.",
    )

    def clean(self):
        if not self.candle_sequence:
            self.candle_sequence = DEFAULT_STAR_SEQUENCE.copy()
        super().clean()


class EveningStar(ReversalPattern):
    """Evening star three-candle reversal pattern."""

    pattern_name = "Evening Star"
    DEFAULT_DESCRIPTION = "Three-candle bearish reversal pattern mirroring the morning star."
    candle_sequence = models.JSONField(
        default=list,
        help_text="Chronological candle sequence key points.",
    )
    tip = models.TextField(
        default=(
            "Observe evening star on the 5-minute chart. The Evening Star pattern is a reversal pattern "
            "that forms when a bearish candlestick is followed by two bullish candlesticks, with the "
            "second bullish candlestick closing higher than the first bullish candlestick."
        ),
        help_text="Additional educational guidance for the pattern.",
    )

    def clean(self):
        if not self.candle_sequence:
            self.candle_sequence = DEFAULT_STAR_SEQUENCE.copy()
        super().clean()
