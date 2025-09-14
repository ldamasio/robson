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
    """Operation groups entry/exit orders and tracks pnl."""

    SIDE_CHOICES = [("BUY", "Buy"), ("SELL", "Sell")]
    STATUS_CHOICES = [
        ("PLANNED", "Planned"),
        ("ACTIVE", "Active"),
        ("CLOSED", "Closed"),
        ("CANCELLED", "Cancelled"),
    ]

    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE)
    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default="PLANNED")

    stop_gain_percent = models.DecimalField(max_digits=10, decimal_places=2, null=True, blank=True)
    stop_loss_percent = models.DecimalField(max_digits=10, decimal_places=2, null=True, blank=True)

    entry_orders = models.ManyToManyField(Order, related_name="operation_entries", blank=True)
    exit_orders = models.ManyToManyField(Order, related_name="operation_exits", blank=True)

    class Meta:
        ordering = ["-created_at"]

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
