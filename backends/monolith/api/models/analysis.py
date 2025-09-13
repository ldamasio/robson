# api/models/analysis.py

from django.db import models
from .base import BaseTechnicalModel
from .trading import Strategy


class TechnicalAnalysisInterpretation(BaseTechnicalModel):
    """Technical analysis interpretation definition."""

    name = models.CharField(max_length=255)
    # Optional complexity indicator (1-5)
    experience = models.IntegerField(default=1, help_text="Required experience level (1-5)")

    def __str__(self) -> str:  # pragma: no cover
        return self.name


class TechnicalEvent(BaseTechnicalModel):
    """Concrete technical event detected in market data."""

    interpretation = models.ForeignKey(
        TechnicalAnalysisInterpretation, on_delete=models.CASCADE, related_name="events"
    )
    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE, related_name="technical_events")
    timeframe = models.CharField(
        max_length=8, default="1h", help_text="Timeframe of the event (e.g., 1h, 4h, 1d)"
    )

    def __str__(self) -> str:  # pragma: no cover
        return f"{self.interpretation.name} @ {self.timeframe}"


class Argument(BaseTechnicalModel):
    """Supporting argument attached to a technical event."""

    technical_event = models.ForeignKey(
        TechnicalEvent, on_delete=models.CASCADE, related_name="arguments"
    )
    name = models.CharField(max_length=255, blank=True, default="")

    def __str__(self) -> str:  # pragma: no cover
        return self.name or f"Argument #{self.pk}"


class Reason(BaseTechnicalModel):
    """Reasoning linked to an argument (rationale/explanation)."""

    argument = models.ForeignKey(Argument, on_delete=models.CASCADE, related_name="reasons")
    name = models.CharField(max_length=255, blank=True, default="")

    def __str__(self) -> str:  # pragma: no cover
        return self.name or f"Reason #{self.pk}"

