# api/serializers/symbol_serializers.py
from rest_framework.serializers import ModelSerializer
from api.models import Symbol


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
