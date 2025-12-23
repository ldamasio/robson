"""
Django models for Isolated Margin Trading.

These models persist margin positions and related data.
Maps domain entities to Django ORM for database storage.

Key Models:
- MarginPosition: Tracks leveraged positions from open to close
- MarginTransfer: Audit trail for Spot <-> Margin transfers
"""

from django.db import models
from django.utils import timezone
from decimal import Decimal


class MarginPosition(models.Model):
    """
    Database model for Isolated Margin positions.
    
    Tracks the full lifecycle of a leveraged position:
    PENDING -> OPEN -> [CLOSING] -> CLOSED/STOPPED_OUT/LIQUIDATED
    
    Key invariants:
    - Stop-loss is mandatory (stop_price cannot be null)
    - Risk parameters are calculated at position creation
    - P&L is tracked throughout position lifetime
    """
    
    class Status(models.TextChoices):
        PENDING = "PENDING", "Pending Fill"
        OPEN = "OPEN", "Open"
        CLOSING = "CLOSING", "Closing"
        CLOSED = "CLOSED", "Closed"
        STOPPED_OUT = "STOPPED_OUT", "Stopped Out"
        TAKE_PROFIT = "TAKE_PROFIT", "Take Profit Hit"
        LIQUIDATED = "LIQUIDATED", "Liquidated"
    
    class Side(models.TextChoices):
        LONG = "LONG", "Long"
        SHORT = "SHORT", "Short"
    
    # ========================================================================
    # Identifiers
    # ========================================================================
    
    position_id = models.CharField(
        max_length=64,
        unique=True,
        db_index=True,
        help_text="Unique position identifier (UUID)",
    )
    
    client = models.ForeignKey(
        'clients.Client',
        on_delete=models.PROTECT,
        related_name='margin_positions',
        help_text="Client (tenant) who owns this position",
    )
    
    correlation_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text="Correlation ID for tracing related operations",
    )
    
    # ========================================================================
    # Position Details
    # ========================================================================
    
    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text="Trading pair (e.g., BTCUSDC)",
    )
    
    side = models.CharField(
        max_length=10,
        choices=Side.choices,
        help_text="Position side: LONG (buy first) or SHORT (sell first)",
    )
    
    status = models.CharField(
        max_length=20,
        choices=Status.choices,
        default=Status.PENDING,
        db_index=True,
        help_text="Current position status",
    )
    
    leverage = models.PositiveSmallIntegerField(
        default=3,
        help_text="Leverage multiplier (1-10)",
    )
    
    # ========================================================================
    # Prices
    # ========================================================================
    
    entry_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Entry/fill price",
    )
    
    stop_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Stop-loss price (MANDATORY)",
    )
    
    target_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Take-profit price (optional)",
    )
    
    current_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Last known price (for P&L calculation)",
    )
    
    close_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Price at position close",
    )
    
    # ========================================================================
    # Quantities
    # ========================================================================
    
    quantity = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Position size in base asset",
    )
    
    position_value = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Total position value (quantity × entry_price)",
    )
    
    # ========================================================================
    # Margin & Risk
    # ========================================================================
    
    margin_allocated = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Margin allocated for this position",
    )
    
    borrowed_amount = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Amount borrowed from exchange",
    )
    
    interest_accrued = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Interest accrued on borrowed amount",
    )
    
    margin_level = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        default=Decimal("999"),
        help_text="Current margin level (higher = safer)",
    )
    
    risk_amount = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Maximum loss if stopped (in quote currency)",
    )
    
    risk_percent = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=Decimal("0"),
        help_text="Risk as percentage of capital",
    )
    
    # ========================================================================
    # P&L
    # ========================================================================
    
    unrealized_pnl = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Unrealized profit/loss",
    )
    
    realized_pnl = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Realized profit/loss (after close)",
    )
    
    fees_paid = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Total trading fees paid",
    )
    
    # ========================================================================
    # Binance Order References
    # ========================================================================
    
    binance_entry_order_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text="Binance order ID for entry",
    )
    
    binance_stop_order_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text="Binance order ID for stop-loss",
    )
    
    binance_target_order_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text="Binance order ID for take-profit",
    )
    
    binance_close_order_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text="Binance order ID for close",
    )
    
    # ========================================================================
    # Timestamps
    # ========================================================================
    
    created_at = models.DateTimeField(
        auto_now_add=True,
        help_text="When position was created",
    )
    
    opened_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text="When entry order was filled",
    )
    
    closed_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text="When position was closed",
    )
    
    updated_at = models.DateTimeField(
        auto_now=True,
        help_text="Last update timestamp",
    )
    
    # ========================================================================
    # Audit
    # ========================================================================
    
    close_reason = models.CharField(
        max_length=255,
        null=True,
        blank=True,
        help_text="Reason for position close",
    )
    
    notes = models.TextField(
        null=True,
        blank=True,
        help_text="Additional notes or context",
    )
    
    class Meta:
        db_table = 'api_margin_position'
        ordering = ['-created_at']
        indexes = [
            models.Index(fields=['client', 'status']),
            models.Index(fields=['symbol', 'status']),
            models.Index(fields=['client', 'symbol', 'status']),
        ]
        verbose_name = "Margin Position"
        verbose_name_plural = "Margin Positions"
    
    def __str__(self):
        return f"{self.side} {self.symbol} @ {self.entry_price} ({self.status})"
    
    # ========================================================================
    # Properties
    # ========================================================================
    
    @property
    def is_open(self) -> bool:
        """Check if position is currently open."""
        return self.status == self.Status.OPEN
    
    @property
    def is_closed(self) -> bool:
        """Check if position is closed (any reason)."""
        return self.status in (
            self.Status.CLOSED,
            self.Status.STOPPED_OUT,
            self.Status.TAKE_PROFIT,
            self.Status.LIQUIDATED,
        )
    
    @property
    def stop_distance(self) -> Decimal:
        """Distance from entry to stop (always positive)."""
        return abs(self.entry_price - self.stop_price)
    
    @property
    def stop_distance_percent(self) -> Decimal:
        """Stop distance as percentage of entry price."""
        if self.entry_price == 0:
            return Decimal("0")
        return (self.stop_distance / self.entry_price) * Decimal("100")
    
    @property
    def total_pnl(self) -> Decimal:
        """Total P&L including fees and interest."""
        return self.realized_pnl + self.unrealized_pnl - self.fees_paid - self.interest_accrued
    
    @property
    def is_profitable(self) -> bool:
        """Check if position is currently profitable."""
        return self.total_pnl > 0
    
    @property
    def is_at_risk(self) -> bool:
        """Check if margin level is in warning zone."""
        return self.margin_level < Decimal("1.3")
    
    @property
    def margin_health(self) -> str:
        """Classify margin health level."""
        if self.margin_level >= Decimal("2.0"):
            return "SAFE"
        elif self.margin_level >= Decimal("1.5"):
            return "CAUTION"
        elif self.margin_level >= Decimal("1.3"):
            return "WARNING"
        elif self.margin_level >= Decimal("1.1"):
            return "CRITICAL"
        else:
            return "DANGER"
    
    # ========================================================================
    # Methods
    # ========================================================================
    
    def update_price(self, current_price: Decimal) -> None:
        """Update current price and recalculate unrealized P&L."""
        self.current_price = current_price
        
        if self.side == self.Side.LONG:
            self.unrealized_pnl = (current_price - self.entry_price) * self.quantity
        else:  # SHORT
            self.unrealized_pnl = (self.entry_price - current_price) * self.quantity
    
    def close(self, close_price: Decimal, reason: str = "Manual close") -> None:
        """Close position and calculate realized P&L."""
        self.status = self.Status.CLOSED
        self.close_price = close_price
        self.closed_at = timezone.now()
        self.close_reason = reason
        
        if self.side == self.Side.LONG:
            self.realized_pnl = (close_price - self.entry_price) * self.quantity
        else:
            self.realized_pnl = (self.entry_price - close_price) * self.quantity
        
        self.unrealized_pnl = Decimal("0")


class MarginTransfer(models.Model):
    """
    Audit trail for transfers between Spot and Isolated Margin.
    
    Records all fund movements for accountability and debugging.
    """
    
    class Direction(models.TextChoices):
        TO_MARGIN = "TO_MARGIN", "Spot → Margin"
        FROM_MARGIN = "FROM_MARGIN", "Margin → Spot"
    
    # Identifiers
    transaction_id = models.CharField(
        max_length=64,
        unique=True,
        db_index=True,
        help_text="Binance transaction ID",
    )
    
    client = models.ForeignKey(
        'clients.Client',
        on_delete=models.PROTECT,
        related_name='margin_transfers',
    )
    
    # Transfer details
    symbol = models.CharField(
        max_length=20,
        help_text="Trading pair (e.g., BTCUSDC)",
    )
    
    asset = models.CharField(
        max_length=10,
        help_text="Asset transferred (e.g., USDC)",
    )
    
    amount = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Amount transferred",
    )
    
    direction = models.CharField(
        max_length=20,
        choices=Direction.choices,
        help_text="Transfer direction",
    )
    
    # Status
    success = models.BooleanField(
        default=True,
        help_text="Whether transfer succeeded",
    )
    
    error_message = models.TextField(
        null=True,
        blank=True,
        help_text="Error message if failed",
    )
    
    # Timestamps
    created_at = models.DateTimeField(
        auto_now_add=True,
    )
    
    # Related position (if transfer was for opening position)
    position = models.ForeignKey(
        MarginPosition,
        on_delete=models.SET_NULL,
        null=True,
        blank=True,
        related_name='transfers',
        help_text="Related margin position",
    )
    
    class Meta:
        db_table = 'api_margin_transfer'
        ordering = ['-created_at']
        verbose_name = "Margin Transfer"
        verbose_name_plural = "Margin Transfers"
    
    def __str__(self):
        direction = "→ Margin" if self.direction == self.Direction.TO_MARGIN else "→ Spot"
        return f"{self.amount} {self.asset} {direction} ({self.symbol})"

