# api/models/trading.py

from decimal import Decimal
from django.db import models
from django.utils import timezone
from django.core.exceptions import ValidationError

from .base import BaseModel, ActiveManager, TenantManager


class Symbol(BaseModel):
    """Financial symbols (e.g., BTCUSDT)."""
    name = models.CharField(max_length=255)
    description = models.TextField(blank=True, default="")
    base_asset = models.CharField(max_length=32)
    quote_asset = models.CharField(max_length=32)
    is_active = models.BooleanField(default=True)

    # Quantity constraints (optional)
    min_qty = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0.00000001"))
    max_qty = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)

    # Managers
    objects = TenantManager()
    active = ActiveManager()

    class Meta:
        unique_together = ["id", "client"]
        verbose_name = "Symbol"
        verbose_name_plural = "Symbols"

    def __str__(self):
        return f"{self.name} ({self.client.name if self.client else 'No Client'})"

    # Normalization & basic validation
    def clean(self):
        super().clean()
        if self.name:
            self.name = self.name.upper()
        if self.base_asset:
            self.base_asset = self.base_asset.upper()
        if self.quote_asset:
            self.quote_asset = self.quote_asset.upper()
        if self.min_qty is not None and self.min_qty <= 0:
            raise ValidationError("min_qty must be positive")
        if self.max_qty is not None and self.max_qty <= 0:
            raise ValidationError("max_qty must be positive if set")
        if self.max_qty is not None and self.max_qty <= self.min_qty:
            raise ValidationError("max_qty must be greater than min_qty")

    # Display helpers
    @property
    def display_name(self):
        return (self.name or "").upper()

    @property
    def pair_display(self):
        return f"{self.base_asset}/{self.quote_asset}"

    # Trading helpers
    def is_quantity_valid(self, quantity: Decimal) -> bool:
        if quantity is None:
            return False
        if self.min_qty is not None and quantity < self.min_qty:
            return False
        if self.max_qty is not None and quantity > self.max_qty:
            return False
        return True


class Strategy(BaseModel):
    """Trading strategies with flexible configuration and performance stats."""
    name = models.CharField(max_length=255)
    description = models.TextField(blank=True, default="")
    config = models.JSONField(default=dict, blank=True, help_text="Strategy configuration")
    risk_config = models.JSONField(default=dict, blank=True, help_text="Risk configuration")
    is_active = models.BooleanField(default=True)

    # Performance tracking
    total_trades = models.IntegerField(default=0)
    winning_trades = models.IntegerField(default=0)
    total_pnl = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))

    class Meta:
        verbose_name_plural = "strategies"

    def __str__(self):
        return self.name

    # Config helpers
    def get_config_value(self, key, default=None):
        # support dotted paths but work with flat keys too
        if key in self.config:
            return self.config.get(key, default)
        # dotted path
        current = self.config
        for part in str(key).split("."):
            if isinstance(current, dict) and part in current:
                current = current[part]
            else:
                return default
        return current

    def get_risk_config_value(self, key, default=None):
        return self.risk_config.get(key, default)

    def set_config_value(self, key, value):
        self.config[key] = value
        self.save(update_fields=["config", "updated_at"])

    def update_performance(self, pnl: Decimal, is_winner: bool):
        self.total_trades += 1
        if is_winner:
            self.winning_trades += 1
        self.total_pnl = (self.total_pnl or Decimal("0")) + (pnl or Decimal("0"))
        self.save(update_fields=["total_trades", "winning_trades", "total_pnl", "updated_at"])

    # Derived metrics
    @property
    def win_rate(self) -> float:
        if self.total_trades == 0:
            return 0.0
        return float((self.winning_trades / self.total_trades) * 100)

    @property
    def average_pnl_per_trade(self) -> Decimal:
        if self.total_trades == 0:
            return Decimal("0")
        return (self.total_pnl or Decimal("0")) / Decimal(self.total_trades)


class Order(BaseModel):
    """Trading orders with fill tracking and basic validations."""

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]
    STATUS_CHOICES = [
        ("PENDING", "Pending"),
        ("PARTIALLY_FILLED", "Partially Filled"),
        ("FILLED", "Filled"),
        ("CANCELLED", "Cancelled"),
        ("REJECTED", "Rejected"),
    ]
    ORDER_TYPES = [("MARKET", "Market"), ("LIMIT", "Limit")]

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    strategy = models.ForeignKey(Strategy, null=True, blank=True, on_delete=models.SET_NULL)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    order_type = models.CharField(max_length=20, choices=ORDER_TYPES, default="MARKET")
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default="PENDING")

    # External exchange reference
    binance_order_id = models.CharField(max_length=100, blank=True, null=True, db_index=True)
    
    # Fills
    filled_quantity = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    avg_fill_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    filled_at = models.DateTimeField(null=True, blank=True)

    class Meta:
        ordering = ["-created_at"]

    def __str__(self):
        return f"{self.side} {self.quantity} {self.symbol.name} @ {self.price}"

    def clean(self):
        super().clean()
        # Basic stop loss sanity (if present)
        sl = getattr(self, "stop_loss_price", None)
        if sl is not None:
            if self.side == "BUY" and sl >= self.price:
                raise ValidationError("Stop loss for BUY must be below price")
            if self.side == "SELL" and sl <= self.price:
                raise ValidationError("Stop loss for SELL must be above price")

    # Optional stop loss field (some tests expect it)
    stop_loss_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)

    # Computed properties
    @property
    def remaining_quantity(self) -> Decimal:
        remaining = (self.quantity or Decimal("0")) - (self.filled_quantity or Decimal("0"))
        return remaining if remaining > 0 else Decimal("0")

    @property
    def fill_percentage(self) -> float:
        if not self.quantity or self.quantity == 0:
            return 0.0
        return float((self.filled_quantity / self.quantity) * 100)

    @property
    def is_filled(self) -> bool:
        return self.status == "FILLED"

    @property
    def is_active(self) -> bool:
        return self.status in {"PENDING", "PARTIALLY_FILLED"} and self.remaining_quantity > 0

    def mark_as_filled(self, avg_price: Decimal, filled_qty: Decimal | None = None):
        if filled_qty is None:
            self.filled_quantity = self.quantity
        else:
            self.filled_quantity = Decimal(filled_qty)
        self.avg_fill_price = Decimal(avg_price)
        if self.filled_quantity >= self.quantity:
            self.status = "FILLED"
            self.filled_at = timezone.now()
        else:
            self.status = "PARTIALLY_FILLED"
        self.save(update_fields=[
            "filled_quantity",
            "avg_fill_price",
            "status",
            "filled_at",
            "updated_at",
        ])

    def calculate_pnl(self, current_price: Decimal) -> Decimal:
        fill_price = self.avg_fill_price if self.avg_fill_price is not None else self.price
        qty = self.filled_quantity if self.filled_quantity and self.filled_quantity > 0 else self.quantity
        if self.side == "BUY":
            return (Decimal(current_price) - fill_price) * qty
        else:
            return (fill_price - Decimal(current_price)) * qty


class Operation(BaseModel):
    """
    Operation (Level 2 in transaction hierarchy): Complete trade cycle.

    Represents a trade lifecycle from entry to exit. Can be created from:
    1. TradingIntent LIVE execution (agentic workflow)
    2. Manual user operation (legacy create_user_operation flow)

    Hierarchy:
        Strategy (L1) → Operation (L2) → Movement/AuditTransaction (L3)

    See: docs/architecture/TRANSACTION-HIERARCHY.md
    """

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]
    STATUS_CHOICES = [
        ("PLANNED", "Planned"),
        ("ACTIVE", "Active"),
        ("CLOSED", "Closed"),
        ("CANCELLED", "Cancelled"),
    ]

    # Link to TradingIntent (agentic workflow)
    # OneToOne: One LIVE TradingIntent creates exactly one Operation
    # Nullable: Operation can exist without TradingIntent (manual flow, backward compat)
    trading_intent = models.OneToOneField(
        'TradingIntent',
        on_delete=models.SET_NULL,
        null=True,
        blank=True,
        related_name='operation',
        help_text='TradingIntent that created this operation (agentic workflow only)'
    )

    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE)
    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default="PLANNED")

    # Deprecated percentage fields (kept for reference only)
    stop_gain_percent = models.DecimalField(
        max_digits=10, decimal_places=2, null=True, blank=True,
        help_text='[DEPRECATED] Use target_price instead. Kept for reference only.'
    )
    stop_loss_percent = models.DecimalField(
        max_digits=10, decimal_places=2, null=True, blank=True,
        help_text='[DEPRECATED] Use stop_price instead. Kept for reference only.'
    )

    # ADR-0012: Absolute stop/target prices (FIXED levels, never recalculated)
    stop_price = models.DecimalField(
        max_digits=20, decimal_places=8, null=True, blank=True, db_index=True,
        help_text='Absolute technical stop price (FIXED level, never recalculated)'
    )
    target_price = models.DecimalField(
        max_digits=20, decimal_places=8, null=True, blank=True, db_index=True,
        help_text='Absolute target/take-profit price (FIXED level)'
    )
    stop_execution_token = models.CharField(
        max_length=64, null=True, blank=True, db_index=True,
        help_text='Idempotency token of current/last stop execution'
    )
    last_stop_check_at = models.DateTimeField(
        null=True, blank=True,
        help_text='Last time stop monitor checked this operation'
    )
    stop_check_count = models.IntegerField(
        null=True, blank=True, default=0,
        help_text='Number of times stop monitor has checked this operation'
    )

    entry_orders = models.ManyToManyField(Order, related_name="operation_entries", blank=True)
    exit_orders = models.ManyToManyField(Order, related_name="operation_exits", blank=True)

    class Meta:
        ordering = ["-created_at"]
        indexes = [
            models.Index(fields=['client', 'status', 'created_at'], name='operation_portfolio_idx'),
            models.Index(fields=['trading_intent'], name='operation_intent_idx'),
        ]

    # Aggregations
    @property
    def is_complete(self) -> bool:
        return self.status == "CLOSED"

    @property
    def total_entry_quantity(self) -> Decimal:
        total = Decimal("0")
        for o in self.entry_orders.all():
            total += o.filled_quantity or o.quantity or Decimal("0")
        return total

    @property
    def total_exit_quantity(self) -> Decimal:
        total = Decimal("0")
        for o in self.exit_orders.all():
            total += o.filled_quantity or o.quantity or Decimal("0")
        return total

    @property
    def average_entry_price(self) -> Decimal | None:
        total_qty = Decimal("0")
        total_cost = Decimal("0")
        for o in self.entry_orders.all():
            qty = o.filled_quantity or o.quantity or Decimal("0")
            price = o.avg_fill_price if o.avg_fill_price is not None else o.price
            total_qty += qty
            total_cost += price * qty
        if total_qty == 0:
            return None
        return total_cost / total_qty

    @property
    def average_exit_price(self) -> Decimal | None:
        total_qty = Decimal("0")
        total_value = Decimal("0")
        for o in self.exit_orders.all():
            qty = o.filled_quantity or o.quantity or Decimal("0")
            price = o.avg_fill_price if o.avg_fill_price is not None else o.price
            total_qty += qty
            total_value += price * qty
        if total_qty == 0:
            return None
        return total_value / total_qty

    def calculate_unrealized_pnl(self, current_price: Decimal) -> Decimal:
        avg_entry = self.average_entry_price
        qty = self.total_entry_quantity
        if not avg_entry or qty == 0:
            return Decimal("0")
        if self.side == "BUY":
            return (Decimal(current_price) - avg_entry) * qty
        else:
            return (avg_entry - Decimal(current_price)) * qty


class Position(BaseModel):
    """Open position with average price and unrealized pnl."""

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]
    STATUS_CHOICES = [("OPEN", "Open"), ("CLOSED", "Closed")]

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    strategy = models.ForeignKey(Strategy, null=True, blank=True, on_delete=models.SET_NULL)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    average_price = models.DecimalField(max_digits=20, decimal_places=8)
    status = models.CharField(max_length=10, choices=STATUS_CHOICES, default="OPEN")
    closed_at = models.DateTimeField(null=True, blank=True)
    unrealized_pnl = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))

    class Meta:
        ordering = ["-created_at"]

    @property
    def is_long(self) -> bool:
        return self.side == "BUY"

    @property
    def is_short(self) -> bool:
        return self.side == "SELL"

    @property
    def is_open(self) -> bool:
        return self.status == "OPEN"

    @property
    def cost_basis(self) -> Decimal:
        return (self.average_price or Decimal("0")) * (self.quantity or Decimal("0"))

    def update_unrealized_pnl(self, current_price: Decimal):
        if self.is_long:
            self.unrealized_pnl = (Decimal(current_price) - self.average_price) * self.quantity
        else:
            self.unrealized_pnl = (self.average_price - Decimal(current_price)) * self.quantity
        self.save(update_fields=["unrealized_pnl", "updated_at"])

    def add_order(self, order: Order):
        if order.avg_fill_price is None:
            return
        new_qty = self.quantity + (order.filled_quantity or order.quantity or Decimal("0"))
        if new_qty == 0:
            return
        total_cost = (self.average_price * self.quantity) + (order.avg_fill_price * (order.filled_quantity or order.quantity or Decimal("0")))
        self.average_price = total_cost / new_qty
        self.quantity = new_qty
        self.save(update_fields=["average_price", "quantity", "updated_at"])

    def close_position(self, current_price: Decimal) -> Decimal:
        if self.is_long:
            pnl = (Decimal(current_price) - self.average_price) * self.quantity
        else:
            pnl = (self.average_price - Decimal(current_price)) * self.quantity
        self.status = "CLOSED"
        self.closed_at = timezone.now()
        self.save(update_fields=["status", "closed_at", "updated_at"])
        return pnl


class Trade(BaseModel):
    """Executed trade summary with fees and duration."""

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]

    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    strategy = models.ForeignKey(Strategy, null=True, blank=True, on_delete=models.SET_NULL)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    quantity = models.DecimalField(max_digits=20, decimal_places=8)

    entry_price = models.DecimalField(max_digits=20, decimal_places=8)
    exit_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    entry_fee = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    exit_fee = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))

    entry_time = models.DateTimeField()
    exit_time = models.DateTimeField(null=True, blank=True)

    # Calculated on save when possible
    pnl = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))

    class Meta:
        ordering = ["-created_at"]

    @property
    def total_fees(self) -> Decimal:
        return (self.entry_fee or Decimal("0")) + (self.exit_fee or Decimal("0"))

    @property
    def is_closed(self) -> bool:
        # Consider closed if exit_price is present, even if exit_time not set
        return self.exit_price is not None

    @property
    def is_winner(self) -> bool:
        if self.exit_price is None:
            return False
        if self.side == "BUY":
            return self.exit_price > self.entry_price
        return self.exit_price < self.entry_price

    @property
    def duration_hours(self) -> float | None:
        if not self.exit_time or not self.entry_time:
            return None
        delta = self.exit_time - self.entry_time
        return delta.total_seconds() / 3600.0

    @property
    def duration(self):
        if not self.exit_time or not self.entry_time:
            return None
        return self.exit_time - self.entry_time

    @property
    def pnl_percentage(self) -> Decimal | None:
        if not self.is_closed:
            return None
        cost_basis = self.entry_price * self.quantity
        if cost_basis == 0:
            return None
        return (self.pnl / cost_basis) * Decimal("100")

    def save(self, *args, **kwargs):
        # auto compute P&L on close
        if self.exit_price is not None:
            if self.side == "BUY":
                gross = (self.exit_price - self.entry_price) * self.quantity
            else:
                gross = (self.entry_price - self.exit_price) * self.quantity
            self.pnl = gross - self.total_fees
        super().save(*args, **kwargs)


class TradingIntent(BaseModel):
    """
    Systematic trading intent - records what the algorithm decided to do.

    This is the audit trail for systematic trading decisions, separate from
    user-initiated operations. Captures the full context of WHY a decision
    was made, including market regime, confidence, and risk calculations.

    Status flow: PENDING → VALIDATED → EXECUTING → EXECUTED (or FAILED/CANCELLED)
    """

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]
    STATUS_CHOICES = [
        ("PENDING", "Pending"),
        ("VALIDATED", "Validated"),
        ("EXECUTING", "Executing"),
        ("EXECUTED", "Executed"),
        ("FAILED", "Failed"),
        ("CANCELLED", "Cancelled"),
    ]

    # Unique identifier for this intent
    intent_id = models.CharField(max_length=255, unique=True, db_index=True)

    # Trading parameters
    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default="PENDING", db_index=True)

    # Quantities and prices
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    entry_price = models.DecimalField(max_digits=20, decimal_places=8)
    stop_price = models.DecimalField(max_digits=20, decimal_places=8)
    target_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)

    # Decision context (WHY this intent was created)
    regime = models.CharField(max_length=50, help_text="Market regime: bull, bear, sideways")
    confidence = models.FloatField(help_text="Confidence level 0.0 to 1.0")
    reason = models.TextField(help_text="Human-readable explanation of decision")

    # Timestamps
    validated_at = models.DateTimeField(null=True, blank=True)
    executed_at = models.DateTimeField(null=True, blank=True)

    # Execution results
    order = models.ForeignKey(Order, null=True, blank=True, on_delete=models.SET_NULL, related_name="intents")
    exchange_order_id = models.CharField(max_length=100, blank=True, null=True)
    actual_fill_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    actual_fill_quantity = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)

    # Risk calculations
    capital = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"), help_text="Capital allocated for this intent")
    risk_amount = models.DecimalField(max_digits=20, decimal_places=8, default=Decimal("0"))
    risk_percent = models.DecimalField(max_digits=10, decimal_places=2, default=Decimal("0"))

    # Agentic workflow results (PLAN → VALIDATE → EXECUTE)
    validation_result = models.JSONField(null=True, blank=True, help_text="ValidationReport.to_dict() result")
    execution_result = models.JSONField(null=True, blank=True, help_text="ExecutionResult.to_dict() result")

    # Event correlation (for distributed tracing)
    correlation_id = models.CharField(max_length=255, blank=True, null=True, db_index=True)

    # Error tracking
    error_message = models.TextField(blank=True, null=True)

    # Pattern trigger metadata (Phase 5 MVP)
    # These fields are populated when the intent is created by pattern auto-trigger
    pattern_code = models.CharField(max_length=50, blank=True, null=True, db_index=True, help_text="Pattern code that triggered this intent (e.g., HAMMER, MA_CROSSOVER)")
    pattern_source = models.CharField(max_length=50, blank=True, null=True, default="manual", help_text="Source: 'pattern' or 'manual'")
    pattern_event_id = models.CharField(max_length=255, blank=True, null=True, db_index=True, help_text="Unique event ID from pattern engine for idempotency")
    pattern_triggered_at = models.DateTimeField(blank=True, null=True, help_text="When the pattern triggered this intent")

    class Meta:
        ordering = ["-created_at"]
        verbose_name = "Trading Intent"
        verbose_name_plural = "Trading Intents"
        indexes = [
            models.Index(fields=["client", "status", "created_at"]),
            models.Index(fields=["symbol", "created_at"]),
            models.Index(fields=["strategy", "created_at"]),
        ]

    def __str__(self):
        return f"Intent {self.intent_id}: {self.side} {self.quantity} {self.symbol.name} ({self.status})"

    # Computed properties
    @property
    def is_pending(self) -> bool:
        return self.status == "PENDING"

    @property
    def is_executed(self) -> bool:
        return self.status == "EXECUTED"

    @property
    def is_failed(self) -> bool:
        return self.status in {"FAILED", "CANCELLED"}

    @property
    def position_value(self) -> Decimal:
        """Total value of position (entry_price * quantity)."""
        return self.entry_price * self.quantity

    @property
    def stop_distance(self) -> Decimal:
        """Distance from entry to stop (always positive)."""
        return abs(self.entry_price - self.stop_price)

    @property
    def stop_distance_percent(self) -> Decimal:
        """Stop distance as % of entry price."""
        if self.entry_price == 0:
            return Decimal("0")
        return (self.stop_distance / self.entry_price) * Decimal("100")

    # State transitions
    def mark_as_validated(self):
        """Mark intent as validated."""
        self.status = "VALIDATED"
        self.validated_at = timezone.now()
        self.save(update_fields=["status", "validated_at", "updated_at"])

    def mark_as_executing(self):
        """Mark intent as executing."""
        self.status = "EXECUTING"
        self.save(update_fields=["status", "updated_at"])

    def mark_as_executed(self, order: Order, fill_price: Decimal, fill_quantity: Decimal):
        """Mark intent as successfully executed."""
        self.status = "EXECUTED"
        self.executed_at = timezone.now()
        self.order = order
        self.exchange_order_id = order.binance_order_id
        self.actual_fill_price = fill_price
        self.actual_fill_quantity = fill_quantity
        self.save(update_fields=[
            "status",
            "executed_at",
            "order",
            "exchange_order_id",
            "actual_fill_price",
            "actual_fill_quantity",
            "updated_at",
        ])

    def mark_as_failed(self, error_message: str):
        """Mark intent as failed."""
        self.status = "FAILED"
        self.error_message = error_message
        self.save(update_fields=["status", "error_message", "updated_at"])

    def clean(self):
        """Validate business rules."""
        super().clean()

        # Validate quantity is positive
        if self.quantity <= 0:
            raise ValidationError("Quantity must be positive")

        # Validate prices are positive
        if self.entry_price <= 0:
            raise ValidationError("Entry price must be positive")
        if self.stop_price <= 0:
            raise ValidationError("Stop price must be positive")

        # Validate stop price direction
        if self.side == "BUY" and self.stop_price >= self.entry_price:
            raise ValidationError("Stop price for BUY must be below entry price")
        if self.side == "SELL" and self.stop_price <= self.entry_price:
            raise ValidationError("Stop price for SELL must be above entry price")

        # Validate confidence range
        if not (0.0 <= self.confidence <= 1.0):
            raise ValidationError("Confidence must be between 0.0 and 1.0")


class PatternTrigger(BaseModel):
    """
    Idempotency tracking for pattern auto-triggers (Phase 5 MVP).

    Ensures that each pattern event creates at most one trading intent.
    Key: (client, pattern_event_id) is unique.

    This is a minimal MVP implementation for idempotency only.
    Full audit logging with AutoTriggerEvent model is post-MVP (see ADR-0019).
    """

    # Unique event identifier from pattern engine
    pattern_event_id = models.CharField(max_length=255, unique=True, db_index=True, help_text="Unique event ID from pattern engine")

    # Pattern identification
    pattern_code = models.CharField(max_length=50, db_index=True, help_text="Pattern code (e.g., HAMMER, MA_CROSSOVER)")

    # Reference to created intent
    intent = models.ForeignKey(TradingIntent, on_delete=models.CASCADE, related_name="pattern_triggers", null=True, blank=True)

    # Status tracking
    status = models.CharField(
        max_length=20,
        choices=[
            ("processed", "Processed"),
            ("failed", "Failed"),
        ],
        default="processed",
    )

    # Error message (if failed)
    error_message = models.TextField(blank=True, null=True)

    # Timestamps
    processed_at = models.DateTimeField(auto_now_add=True)

    class Meta:
        verbose_name = "Pattern Trigger"
        verbose_name_plural = "Pattern Triggers"
        indexes = [
            models.Index(fields=["client", "pattern_event_id"]),
            models.Index(fields=["pattern_code", "processed_at"]),
        ]

    def __str__(self):
        return f"PatternTrigger {self.pattern_event_id}: {self.pattern_code} ({self.status})"

    @classmethod
    def has_been_processed(cls, client_id, pattern_event_id):
        """Check if a pattern event has already been processed."""
        return cls.objects.filter(
            client_id=client_id,
            pattern_event_id=pattern_event_id
        ).exists()

    @classmethod
    def record_trigger(cls, client_id, pattern_event_id, pattern_code, intent=None, error_message=None):
        """Record a pattern trigger event."""
        return cls.objects.create(
            client_id=client_id,
            pattern_event_id=pattern_event_id,
            pattern_code=pattern_code,
            intent=intent,
            status="failed" if error_message else "processed",
            error_message=error_message,
        )

