"""
Serializers for Pattern Detection Engine.

Includes serializers for:
- PatternCatalog: Pattern metadata
- PatternInstance: Detected pattern occurrences
- PatternAlert: Pattern lifecycle alerts
- PatternPoint: Pattern coordinate points
- StrategyPatternConfig: Strategy-pattern auto-entry configuration
"""

from rest_framework import serializers
from api.models.patterns.base import (
    PatternCatalog,
    PatternInstance,
    PatternAlert,
    PatternPoint,
    PatternStatus,
    PatternCategory,
    PatternDirectionBias,
)
from api.models.patterns.strategy_config import StrategyPatternConfig
from api.models.trading import Strategy


class PatternPointSerializer(serializers.ModelSerializer):
    """Serializer for PatternPoint coordinates."""

    class Meta:
        model = PatternPoint
        fields = ["id", "label", "ts", "price", "bar_index_offset", "role"]


class PatternCatalogSerializer(serializers.ModelSerializer):
    """Serializer for PatternCatalog metadata."""

    category_display = serializers.CharField(source="get_category_display", read_only=True)
    direction_bias_display = serializers.CharField(source="get_direction_bias_display", read_only=True)

    class Meta:
        model = PatternCatalog
        fields = [
            "id",
            "pattern_code",
            "name",
            "category",
            "category_display",
            "direction_bias",
            "direction_bias_display",
            "min_bars",
            "max_bars",
            "min_touches",
            "requires_gap",
            "fib_tolerance_pct",
            "default_confirmation_rule",
            "default_invalidation_rule",
            "theory_notes",
            "references",
            "metadata",
        ]


class PatternInstanceSerializer(serializers.ModelSerializer):
    """Serializer for PatternInstance detected occurrences."""

    pattern_code = serializers.CharField(source="pattern.pattern_code", read_only=True)
    pattern_name = serializers.CharField(source="pattern.name", read_only=True)
    symbol = serializers.CharField(source="symbol.name", read_only=True)
    status_display = serializers.CharField(source="get_status_display", read_only=True)
    direction_bias_display = serializers.CharField(source="pattern.get_direction_bias_display", read_only=True)
    category_display = serializers.CharField(source="pattern.get_category_display", read_only=True)
    pattern_points = PatternPointSerializer(many=True, read_only=True)
    alerts = serializers.SerializerMethodField()

    class Meta:
        model = PatternInstance
        fields = [
            "id",
            "pattern",
            "pattern_code",
            "pattern_name",
            "symbol",
            "timeframe",
            "status",
            "status_display",
            "category_display",
            "direction_bias_display",
            "start_ts",
            "end_ts",
            "breakout_ts",
            "trend_precondition",
            "pre_pattern_return_pct",
            "pre_pattern_atr",
            "vol_regime_z",
            "volume_profile",
            "breakout_direction",
            "breakout_volume_z",
            "climax_volume_flag",
            "confirm_method",
            "confirm_strength",
            "invalidation_level",
            "measured_move",
            "tp1",
            "tp2",
            "tp3",
            "stop_method",
            "r_multiple_plan",
            "pattern_score",
            "reliability_est",
            "historical_winrate_est",
            "edge_z",
            "notes",
            "htf_alignment",
            "htf_trend",
            "lt_tf_confirm",
            "detected_version",
            "features",
            "pattern_points",
            "alerts",
        ]

    def get_alerts(self, obj):
        """Get related alerts for this instance."""
        alerts = obj.alerts.all().order_by("-alert_ts")
        return PatternAlertSerializer(alerts, many=True).data


class PatternAlertSerializer(serializers.ModelSerializer):
    """Serializer for PatternAlert lifecycle events."""

    pattern_code = serializers.CharField(source="instance.pattern.pattern_code", read_only=True)
    pattern_name = serializers.CharField(source="instance.pattern.name", read_only=True)
    symbol = serializers.CharField(source="instance.symbol.name", read_only=True)
    timeframe = serializers.CharField(source="instance.timeframe", read_only=True)
    alert_type_display = serializers.CharField(source="get_alert_type_display", read_only=True)

    class Meta:
        model = PatternAlert
        fields = [
            "id",
            "instance",
            "pattern_code",
            "pattern_name",
            "symbol",
            "timeframe",
            "alert_type",
            "alert_type_display",
            "alert_ts",
            "confidence",
            "payload",
            "created_at",
            "updated_at",
        ]


class StrategyPatternConfigSerializer(serializers.ModelSerializer):
    """Serializer for StrategyPatternConfig auto-entry configuration."""

    strategy_name = serializers.CharField(source="strategy.name", read_only=True)
    pattern_code = serializers.CharField(source="pattern.pattern_code", read_only=True)
    pattern_name = serializers.CharField(source="pattern.name", read_only=True)

    class Meta:
        model = StrategyPatternConfig
        fields = [
            "id",
            "strategy",
            "strategy_name",
            "pattern",
            "pattern_code",
            "pattern_name",
            "auto_entry_enabled",
            "min_confidence",
            "max_entries_per_day",
            "max_entries_per_week",
            "cooldown_minutes",
            "position_size_pct",
            "require_confirmation",
            "timeframes",
            "symbols",
            "require_volume_confirmation",
            "only_with_trend",
            "notes",
            "is_active",
        ]


class CreateStrategyPatternConfigSerializer(serializers.ModelSerializer):
    """Serializer for creating StrategyPatternConfig."""

    class Meta:
        model = StrategyPatternConfig
        fields = [
            "strategy",
            "pattern",
            "auto_entry_enabled",
            "min_confidence",
            "max_entries_per_day",
            "max_entries_per_week",
            "cooldown_minutes",
            "position_size_pct",
            "require_confirmation",
            "timeframes",
            "symbols",
            "require_volume_confirmation",
            "only_with_trend",
            "notes",
        ]

    def validate(self, data):
        """Validate configuration constraints."""
        min_confidence = data.get("min_confidence", 0)
        if min_confidence < 0 or min_confidence > 1:
            raise serializers.ValidationError("min_confidence must be between 0 and 1")

        position_size_pct = data.get("position_size_pct")
        if position_size_pct is not None and (position_size_pct <= 0 or position_size_pct > 100):
            raise serializers.ValidationError("position_size_pct must be between 0 and 100")

        return data


class UpdateStrategyPatternConfigSerializer(serializers.ModelSerializer):
    """Serializer for updating StrategyPatternConfig."""

    class Meta:
        model = StrategyPatternConfig
        fields = [
            "auto_entry_enabled",
            "min_confidence",
            "max_entries_per_day",
            "max_entries_per_week",
            "cooldown_minutes",
            "position_size_pct",
            "require_confirmation",
            "timeframes",
            "symbols",
            "require_volume_confirmation",
            "only_with_trend",
            "notes",
            "is_active",
        ]


class PatternScanRequestSerializer(serializers.Serializer):
    """Serializer for pattern scan request."""

    symbols = serializers.CharField(required=False, default="BTCUSDT")
    timeframes = serializers.CharField(required=False, default="15m,1h")
    candlestick = serializers.BooleanField(required=False, default=False)
    chart = serializers.BooleanField(required=False, default=False)
    all_detectors = serializers.BooleanField(required=False, default=True)


class PatternScanResultSerializer(serializers.Serializer):
    """Serializer for pattern scan result."""

    symbol = serializers.CharField()
    timeframe = serializers.CharField()
    patterns_detected = serializers.IntegerField()
    confirmations_found = serializers.IntegerField()
    invalidations_found = serializers.IntegerField()
