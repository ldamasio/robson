"""
API Serializers Package.

Exports all serializers for the API.
"""

from rest_framework.serializers import ModelSerializer

from api.models import Strategy, Symbol
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


class SymbolSerializer(ModelSerializer):
    """Serializer for Symbol model (trading pairs)."""

    class Meta:
        model = Symbol
        fields = [
            'id',
            'name',
            'base_asset',
            'quote_asset',
            'description',
            'is_active',
        ]


__all__ = [
    "StrategySerializer",
    "SymbolSerializer",
    "PatternCatalogSerializer",
    "PatternInstanceSerializer",
    "PatternAlertSerializer",
    "StrategyPatternConfigSerializer",
    "CreateStrategyPatternConfigSerializer",
    "UpdateStrategyPatternConfigSerializer",
]
