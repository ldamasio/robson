"""
API Serializers Package.

Exports all serializers for the API.
"""

from rest_framework.serializers import ModelSerializer

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
        # Lazy import to avoid circular dependency
        from api.models import Strategy
        model = Strategy
        fields = ['id', 'name']


class SymbolSerializer(ModelSerializer):
    """Serializer for Symbol model (trading pairs)."""

    class Meta:
        # Lazy import to avoid circular dependency
        from api.models import Symbol
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
