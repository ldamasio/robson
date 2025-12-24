"""
Audit Trail Models for Complete Transaction Transparency.

Every operation in Robson is recorded here for full auditability:
- Spot trades (buy/sell)
- Margin trades (leveraged buy/sell)
- Transfers (spot <-> margin)
- Stop-loss orders
- Take-profit orders
- Borrows and repayments

The user should be able to see EVERYTHING that happened.
"""

from django.db import models
from django.utils import timezone
from decimal import Decimal


class TransactionType(models.TextChoices):
    """All possible transaction types."""
    # Spot operations
    SPOT_BUY = "SPOT_BUY", "Spot Buy"
    SPOT_SELL = "SPOT_SELL", "Spot Sell"
    
    # Margin operations
    MARGIN_BUY = "MARGIN_BUY", "Margin Buy"
    MARGIN_SELL = "MARGIN_SELL", "Margin Sell"
    MARGIN_BORROW = "MARGIN_BORROW", "Margin Borrow"
    MARGIN_REPAY = "MARGIN_REPAY", "Margin Repay"
    
    # Transfers
    TRANSFER_TO_MARGIN = "TRANSFER_TO_MARGIN", "Transfer to Margin"
    TRANSFER_FROM_MARGIN = "TRANSFER_FROM_MARGIN", "Transfer from Margin"
    
    # Orders
    STOP_LOSS_PLACED = "STOP_LOSS_PLACED", "Stop-Loss Placed"
    STOP_LOSS_TRIGGERED = "STOP_LOSS_TRIGGERED", "Stop-Loss Triggered"
    STOP_LOSS_CANCELLED = "STOP_LOSS_CANCELLED", "Stop-Loss Cancelled"
    TAKE_PROFIT_PLACED = "TAKE_PROFIT_PLACED", "Take-Profit Placed"
    TAKE_PROFIT_TRIGGERED = "TAKE_PROFIT_TRIGGERED", "Take-Profit Triggered"
    
    # System events
    LIQUIDATION = "LIQUIDATION", "Liquidation"
    INTEREST_CHARGED = "INTEREST_CHARGED", "Interest Charged"
    FEE_PAID = "FEE_PAID", "Fee Paid"


class TransactionStatus(models.TextChoices):
    """Transaction status."""
    PENDING = "PENDING", "Pending"
    FILLED = "FILLED", "Filled"
    PARTIALLY_FILLED = "PARTIALLY_FILLED", "Partially Filled"
    CANCELLED = "CANCELLED", "Cancelled"
    FAILED = "FAILED", "Failed"
    EXPIRED = "EXPIRED", "Expired"


class AuditTransaction(models.Model):
    """
    Unified audit trail for ALL transactions.
    
    Every operation that moves money or creates obligations is recorded here.
    This provides complete transparency for the user.
    """
    
    # Identifiers
    transaction_id = models.CharField(
        max_length=64,
        unique=True,
        db_index=True,
        help_text="Unique transaction ID (UUID or Binance ID)",
    )
    
    binance_order_id = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        db_index=True,
        help_text="Binance order/transaction ID",
    )
    
    client = models.ForeignKey(
        'clients.Client',
        on_delete=models.PROTECT,
        related_name='audit_transactions',
        help_text="Client who owns this transaction",
    )
    
    # Transaction details
    transaction_type = models.CharField(
        max_length=30,
        choices=TransactionType.choices,
        db_index=True,
        help_text="Type of transaction",
    )
    
    status = models.CharField(
        max_length=20,
        choices=TransactionStatus.choices,
        default=TransactionStatus.PENDING,
        db_index=True,
        help_text="Transaction status",
    )
    
    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text="Trading pair (e.g., BTCUSDC)",
    )
    
    asset = models.CharField(
        max_length=10,
        help_text="Asset involved (e.g., BTC, USDC)",
    )
    
    # Quantities
    quantity = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Quantity of asset",
    )
    
    price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Price per unit",
    )
    
    total_value = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Total value (quantity × price)",
    )
    
    fee = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
        help_text="Transaction fee",
    )
    
    fee_asset = models.CharField(
        max_length=10,
        null=True,
        blank=True,
        help_text="Asset used for fee payment",
    )
    
    # Context
    side = models.CharField(
        max_length=10,
        null=True,
        blank=True,
        help_text="BUY or SELL",
    )
    
    leverage = models.PositiveSmallIntegerField(
        null=True,
        blank=True,
        help_text="Leverage used (for margin trades)",
    )
    
    is_isolated_margin = models.BooleanField(
        default=False,
        help_text="Whether this is an isolated margin transaction",
    )
    
    # Related records
    related_position = models.ForeignKey(
        'api.MarginPosition',
        on_delete=models.SET_NULL,
        null=True,
        blank=True,
        related_name='transactions',
        help_text="Related margin position",
    )
    
    related_order = models.ForeignKey(
        'api.Order',
        on_delete=models.SET_NULL,
        null=True,
        blank=True,
        related_name='audit_transactions',
        help_text="Related order",
    )
    
    # Risk management
    stop_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Stop-loss price (if applicable)",
    )
    
    risk_amount = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Risk amount (if applicable)",
    )
    
    risk_percent = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Risk as percentage of capital",
    )
    
    # Audit metadata
    description = models.TextField(
        help_text="Human-readable description of the transaction",
    )
    
    raw_response = models.JSONField(
        null=True,
        blank=True,
        help_text="Raw API response from Binance",
    )
    
    created_at = models.DateTimeField(
        auto_now_add=True,
        db_index=True,
        help_text="When this record was created",
    )
    
    executed_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text="When the transaction was executed on exchange",
    )
    
    source = models.CharField(
        max_length=50,
        default="robson",
        help_text="Source of transaction (robson, binance_sync, manual)",
    )
    
    class Meta:
        db_table = 'api_audit_transaction'
        ordering = ['-created_at']
        indexes = [
            models.Index(fields=['client', 'created_at']),
            models.Index(fields=['transaction_type', 'created_at']),
            models.Index(fields=['symbol', 'created_at']),
            models.Index(fields=['status', 'created_at']),
        ]
        verbose_name = "Audit Transaction"
        verbose_name_plural = "Audit Transactions"
    
    def __str__(self):
        return f"{self.transaction_type} {self.quantity} {self.asset} @ {self.price} ({self.status})"
    
    @property
    def is_buy(self) -> bool:
        return self.side == "BUY"
    
    @property
    def is_sell(self) -> bool:
        return self.side == "SELL"
    
    @property
    def is_margin(self) -> bool:
        return self.transaction_type in [
            TransactionType.MARGIN_BUY,
            TransactionType.MARGIN_SELL,
            TransactionType.MARGIN_BORROW,
            TransactionType.MARGIN_REPAY,
        ]
    
    @property
    def is_transfer(self) -> bool:
        return self.transaction_type in [
            TransactionType.TRANSFER_TO_MARGIN,
            TransactionType.TRANSFER_FROM_MARGIN,
        ]


class BalanceSnapshot(models.Model):
    """
    Periodic snapshots of account balances for historical tracking.
    
    Helps reconstruct account state at any point in time.
    """
    
    client = models.ForeignKey(
        'clients.Client',
        on_delete=models.PROTECT,
        related_name='balance_snapshots',
    )
    
    snapshot_time = models.DateTimeField(
        db_index=True,
        help_text="When this snapshot was taken",
    )
    
    # Spot balances
    spot_usdc = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    spot_btc = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    # Margin balances
    margin_btc_free = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    margin_btc_borrowed = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    margin_usdc_free = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    margin_usdc_borrowed = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        default=Decimal("0"),
    )
    
    # Calculated values
    btc_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="BTC price at snapshot time",
    )
    
    total_equity = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text="Total equity in USDC",
    )
    
    margin_level = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
        help_text="Margin level at snapshot",
    )
    
    created_at = models.DateTimeField(auto_now_add=True)
    
    class Meta:
        db_table = 'api_balance_snapshot'
        ordering = ['-snapshot_time']
        indexes = [
            models.Index(fields=['client', 'snapshot_time']),
        ]
        verbose_name = "Balance Snapshot"
        verbose_name_plural = "Balance Snapshots"
    
    def __str__(self):
        return f"{self.client} @ {self.snapshot_time}: ${self.total_equity}"

