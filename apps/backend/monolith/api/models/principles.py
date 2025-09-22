"""Domain principles and qualitative attributes used across the trading assistant."""

from __future__ import annotations

from django.db import models

from .base import BaseModel, DescriptionMixin, ExperienceMixin


class BasePrinciple(BaseModel, DescriptionMixin, ExperienceMixin):
    """Common fields for qualitative knowledge records."""

    name = models.CharField(max_length=255)

    class Meta:
        abstract = True
        ordering = ["name"]
        verbose_name = "Principle"
        verbose_name_plural = "Principles"

    def __str__(self) -> str:  # pragma: no cover
        return self.name


class OddsYourFavor(BasePrinciple):
    """Key principle to keep odds stacked in the trader's favour."""


class LimitLosses(BasePrinciple):
    """Principle focused on loss mitigation best practises."""


class Attribute(BasePrinciple):
    """Qualitative attribute used when describing strategies or events."""

    def context(self) -> str:
        return self.name

    def primary_implication(self) -> str:
        return self.name

    def underlying_objective(self) -> str:
        return self.name

    def volume(self) -> str:
        return self.name

    def perspective(self) -> str:
        return self.name

    class Meta(BasePrinciple.Meta):
        verbose_name = "Attribute"
        verbose_name_plural = "Attributes"
