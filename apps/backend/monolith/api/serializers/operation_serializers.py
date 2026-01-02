"""
Serializers for Operation model.

Provides read-only serialization for Operation (Level 2) entities.
"""

from rest_framework import serializers
from api.models import Operation


class OperationSerializer(serializers.ModelSerializer):
    """
    Operation serializer (read-only).

    Represents a complete trade cycle (Level 2 in hierarchy).
    """

    # Nested fields for readability
    strategy_name = serializers.CharField(source='strategy.name', read_only=True)
    symbol_name = serializers.CharField(source='symbol.name', read_only=True)
    trading_intent_id = serializers.CharField(source='trading_intent.intent_id', read_only=True, allow_null=True)

    # Computed fields
    movements_count = serializers.SerializerMethodField()
    total_entry_quantity = serializers.DecimalField(max_digits=20, decimal_places=8, read_only=True)
    total_exit_quantity = serializers.DecimalField(max_digits=20, decimal_places=8, read_only=True)
    average_entry_price = serializers.DecimalField(max_digits=20, decimal_places=8, read_only=True, allow_null=True)
    average_exit_price = serializers.DecimalField(max_digits=20, decimal_places=8, read_only=True, allow_null=True)

    class Meta:
        model = Operation
        fields = [
            'id',
            'trading_intent_id',
            'strategy_name',
            'symbol_name',
            'side',
            'status',
            'stop_price',
            'target_price',
            'created_at',
            'updated_at',
            'movements_count',
            'total_entry_quantity',
            'total_exit_quantity',
            'average_entry_price',
            'average_exit_price',
        ]
        read_only_fields = fields

    def get_movements_count(self, obj):
        """Get count of related AuditTransaction movements."""
        return obj.movements.count()
