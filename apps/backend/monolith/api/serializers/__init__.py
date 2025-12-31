"""
API Serializers Package.

Exports all serializers for the API.
"""

from rest_framework.serializers import ModelSerializer

from api.models import Strategy
from .patterns import (
    PatternCatalogSerializer,
    PatternInstanceSerializer,
    PatternAlertSerializer,
    StrategyPatternConfigSerializer,
    CreateStrategyPatternConfigSerializer,
    UpdateStrategyPatternConfigSerializer,
)


class StrategySerializer(ModelSerializer):
    """Serializer for Strategy model."""

    class Meta:
        model = Strategy
        fields = ['id', 'name']


__all__ = [
    "StrategySerializer",
    "PatternCatalogSerializer",
    "PatternInstanceSerializer",
    "PatternAlertSerializer",
    "StrategyPatternConfigSerializer",
    "CreateStrategyPatternConfigSerializer",
    "UpdateStrategyPatternConfigSerializer",
]
