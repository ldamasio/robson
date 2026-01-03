"""
Serializers for Trading Intent API.

These serializers handle validation and serialization for the agentic workflow
PLAN → VALIDATE → EXECUTE endpoints.
"""

from rest_framework import serializers
from decimal import Decimal
from api.models import TradingIntent, Symbol, Strategy


class CreateTradingIntentSerializer(serializers.Serializer):
    """
    Input serializer for creating a new trading intent (manual mode).

    Validates user input and prepares data for CreateTradingIntentUseCase.
    """

    symbol = serializers.IntegerField(
        help_text="Symbol ID (FK to Symbol model)"
    )
    strategy = serializers.IntegerField(
        help_text="Strategy ID (FK to Strategy model)"
    )
    side = serializers.ChoiceField(
        choices=["BUY", "SELL"],
        help_text="Order side (BUY or SELL)",
        required=False  # Optional for auto-mode
    )
    entry_price = serializers.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Entry price for the trade",
        required=False  # Optional for auto-mode
    )
    stop_price = serializers.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Stop-loss price",
        required=False  # Optional for auto-mode
    )
    capital = serializers.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Capital allocated for this trade",
        required=False  # Optional for auto-mode
    )
    target_price = serializers.DecimalField(
        max_digits=20,
        decimal_places=8,
        required=False,
        allow_null=True,
        help_text="Optional take-profit target price"
    )
    regime = serializers.CharField(
        max_length=50,
        required=False,
        default="manual",
        help_text="Market regime context"
    )
    confidence = serializers.FloatField(
        required=False,
        default=0.5,
        min_value=0.0,
        max_value=1.0,
        help_text="Confidence level (0.0 to 1.0)"
    )
    reason = serializers.CharField(
        required=False,
        default="Manual entry via UI",
        help_text="Reason for this trading intent"
    )

    def validate(self, data):
        """
        Cross-field validation.

        Supports both manual and auto modes:
        - Manual mode: All fields provided, validate them
        - Auto mode: Missing fields will be auto-calculated, skip validation
        """
        entry_price = data.get("entry_price")
        stop_price = data.get("stop_price")
        side = data.get("side")
        capital = data.get("capital")

        # Auto mode detection: if any required field is missing, skip validation
        # The backend will auto-calculate these values
        if not all([entry_price, stop_price, side, capital]):
            return data

        # Manual mode: validate all fields
        # Validate prices are positive
        if capital <= 0:
            raise serializers.ValidationError("Capital must be positive")

        if entry_price <= 0:
            raise serializers.ValidationError("Entry price must be positive")

        if stop_price <= 0:
            raise serializers.ValidationError("Stop price must be positive")

        # Validate entry != stop
        if entry_price == stop_price:
            raise serializers.ValidationError("Entry price and stop price cannot be equal")

        # Validate stop direction
        if side == "BUY" and stop_price >= entry_price:
            raise serializers.ValidationError("For BUY orders, stop price must be below entry price")

        if side == "SELL" and stop_price <= entry_price:
            raise serializers.ValidationError("For SELL orders, stop price must be above entry price")

        # Validate target price if provided
        target_price = data.get("target_price")
        if target_price is not None:
            if target_price <= 0:
                raise serializers.ValidationError("Target price must be positive")

            if side == "BUY" and target_price <= entry_price:
                raise serializers.ValidationError("For BUY orders, target price must be above entry price")

            if side == "SELL" and target_price >= entry_price:
                raise serializers.ValidationError("For SELL orders, target price must be below entry price")

        return data


class SymbolNestedSerializer(serializers.ModelSerializer):
    """Nested serializer for Symbol details."""

    class Meta:
        model = Symbol
        fields = ["id", "name", "base_asset", "quote_asset"]


class StrategyNestedSerializer(serializers.ModelSerializer):
    """Nested serializer for Strategy details."""

    win_rate = serializers.SerializerMethodField()

    class Meta:
        model = Strategy
        fields = ["id", "name", "description", "total_trades", "winning_trades", "win_rate"]

    def get_win_rate(self, obj):
        """Calculate win rate percentage."""
        return obj.win_rate


class TradingIntentSerializer(serializers.ModelSerializer):
    """
    Output serializer for TradingIntent with all fields and computed properties.

    Includes nested symbol and strategy details, formatted timestamps,
    and calculated fields like risk-reward ratio.
    """

    symbol = SymbolNestedSerializer(read_only=True)
    strategy = StrategyNestedSerializer(read_only=True)

    # Computed fields
    risk_reward_ratio = serializers.SerializerMethodField()
    distance_to_stop_percent = serializers.SerializerMethodField()
    position_value = serializers.SerializerMethodField()

    # Formatted timestamps
    created_at_formatted = serializers.SerializerMethodField()
    validated_at_formatted = serializers.SerializerMethodField()
    executed_at_formatted = serializers.SerializerMethodField()

    class Meta:
        model = TradingIntent
        fields = [
            # IDs
            "id",
            "intent_id",

            # Related entities
            "symbol",
            "strategy",

            # Trading parameters
            "side",
            "status",
            "quantity",
            "entry_price",
            "stop_price",
            "target_price",

            # Decision context
            "regime",
            "confidence",
            "reason",

            # Risk calculations
            "capital",
            "risk_amount",
            "risk_percent",

            # Computed fields
            "risk_reward_ratio",
            "distance_to_stop_percent",
            "position_value",

            # Execution results
            "order",
            "exchange_order_id",
            "actual_fill_price",
            "actual_fill_quantity",

            # Agentic workflow results
            "validation_result",
            "execution_result",

            # Metadata
            "correlation_id",
            "error_message",

            # Pattern trigger metadata (Phase 5 MVP)
            "pattern_code",
            "pattern_source",
            "pattern_event_id",
            "pattern_triggered_at",

            # Timestamps
            "created_at",
            "updated_at",
            "validated_at",
            "executed_at",
            "created_at_formatted",
            "validated_at_formatted",
            "executed_at_formatted",
        ]

    def get_risk_reward_ratio(self, obj):
        """Calculate risk-reward ratio if target price is set."""
        if not obj.target_price or not obj.entry_price or not obj.stop_price:
            return None

        risk = abs(obj.entry_price - obj.stop_price)
        reward = abs(obj.target_price - obj.entry_price)

        if risk == 0:
            return None

        return float(reward / risk)

    def get_distance_to_stop_percent(self, obj):
        """Calculate distance to stop as percentage of entry price."""
        if not obj.entry_price or not obj.stop_price:
            return None

        distance = abs(obj.entry_price - obj.stop_price)
        return float((distance / obj.entry_price) * Decimal("100"))

    def get_position_value(self, obj):
        """Calculate total position value."""
        if not obj.quantity or not obj.entry_price:
            return None

        return float(obj.quantity * obj.entry_price)

    def get_created_at_formatted(self, obj):
        """Format created_at timestamp."""
        return obj.created_at.isoformat() if obj.created_at else None

    def get_validated_at_formatted(self, obj):
        """Format validated_at timestamp."""
        return obj.validated_at.isoformat() if obj.validated_at else None

    def get_executed_at_formatted(self, obj):
        """Format executed_at timestamp."""
        return obj.executed_at.isoformat() if obj.executed_at else None


class ValidationReportSerializer(serializers.Serializer):
    """
    Serializer for validation report output.

    This serializes the ValidationReport from the validation framework.
    """

    status = serializers.ChoiceField(
        choices=["PASS", "FAIL", "WARNING"],
        help_text="Overall validation status"
    )
    issues = serializers.ListField(
        child=serializers.DictField(),
        help_text="List of validation issues"
    )
    metadata = serializers.DictField(
        help_text="Additional metadata"
    )
    summary = serializers.DictField(
        required=False,
        help_text="Summary statistics"
    )


class ExecutionResultSerializer(serializers.Serializer):
    """
    Serializer for execution result output.

    This serializes the ExecutionResult from the execution framework.
    """

    status = serializers.ChoiceField(
        choices=["SUCCESS", "FAILED", "BLOCKED"],
        help_text="Execution status"
    )
    mode = serializers.ChoiceField(
        choices=["DRY_RUN", "LIVE"],
        help_text="Execution mode"
    )
    guards = serializers.ListField(
        child=serializers.DictField(),
        help_text="List of safety guards executed"
    )
    actions = serializers.ListField(
        child=serializers.DictField(),
        help_text="List of actions taken (simulated or real)"
    )
    metadata = serializers.DictField(
        help_text="Additional metadata"
    )
    executed_at = serializers.DateTimeField(
        help_text="Execution timestamp"
    )
    error = serializers.CharField(
        required=False,
        allow_null=True,
        help_text="Error message if execution failed"
    )


class PatternTriggerSerializer(serializers.Serializer):
    """
    Input serializer for pattern trigger endpoint (Phase 5 MVP).

    Validates pattern trigger requests and creates TradingIntents with idempotency.
    """

    # Pattern identification
    pattern_code = serializers.CharField(
        max_length=50,
        help_text="Pattern code (e.g., HAMMER, MA_CROSSOVER)"
    )
    pattern_event_id = serializers.CharField(
        max_length=255,
        help_text="Unique event ID from pattern engine for idempotency"
    )

    # Trading parameters
    symbol = serializers.IntegerField(help_text="Symbol ID")
    side = serializers.ChoiceField(choices=["BUY", "SELL"])
    entry_price = serializers.DecimalField(max_digits=20, decimal_places=8)
    stop_price = serializers.DecimalField(max_digits=20, decimal_places=8)
    capital = serializers.DecimalField(max_digits=20, decimal_places=8)

    # Optional fields
    strategy = serializers.IntegerField(
        required=False,
        default=None,
        allow_null=True,
        help_text="Strategy ID (optional)"
    )
    target_price = serializers.DecimalField(
        required=False,
        allow_null=True,
        max_digits=20,
        decimal_places=8,
        help_text="Optional take-profit target price"
    )

    # Auto-trigger flags
    auto_validate = serializers.BooleanField(
        required=False,
        default=True,
        help_text="Automatically validate the intent (default: true)"
    )
    auto_execute = serializers.BooleanField(
        required=False,
        default=False,
        help_text="Automatically execute the intent (MVP: must be false)"
    )
    execution_mode = serializers.ChoiceField(
        choices=["dry-run", "live"],
        required=False,
        default="dry-run",
        help_text="Execution mode (MVP: only dry-run allowed)"
    )

    def validate(self, data):
        """Cross-field validation and MVP safety checks."""
        # MVP: Hard block on LIVE auto-execution
        if data.get("auto_execute", False) and data.get("execution_mode", "dry-run") == "live":
            raise serializers.ValidationError(
                "LIVE auto-execution is not enabled in MVP. Use manual execution."
            )

        # Validate entry != stop
        entry_price = data.get("entry_price")
        stop_price = data.get("stop_price")
        side = data.get("side")

        if entry_price == stop_price:
            raise serializers.ValidationError("Entry price and stop price cannot be equal")

        if side == "BUY" and stop_price >= entry_price:
            raise serializers.ValidationError("For BUY orders, stop price must be below entry price")

        if side == "SELL" and stop_price <= entry_price:
            raise serializers.ValidationError("For SELL orders, stop price must be above entry price")

        return data


class PatternTriggerResponseSerializer(serializers.Serializer):
    """Response serializer for pattern trigger endpoint."""

    status = serializers.CharField(help_text="Status: PROCESSED, ALREADY_PROCESSED, or FAILED")
    intent_id = serializers.CharField(required=False, allow_null=True, help_text="TradingIntent intent_id if created")
    message = serializers.CharField(help_text="Human-readable message")
    pattern_code = serializers.CharField(help_text="Pattern code that was triggered")
