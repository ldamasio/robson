# api/models/trading.py

from django.db import models
from .base import BaseModel

class Symbol(BaseModel):
    """Símbolos financeiros"""
    name = models.CharField(max_length=255)
    description = models.TextField()
    is_active = models.BooleanField(default=True)
    
    class Meta:
        unique_together = ["id", "client"]
        verbose_name = "Symbol"
        verbose_name_plural = "Symbols"
    
    def __str__(self):
        return f"{self.name} ({self.client.name if self.client else 'No Client'})"

class Order(BaseModel):
    """Ordens de trading"""
    SIDE_CHOICES = [
        ('BUY', 'Buy'),
        ('SELL', 'Sell'),
    ]
    
    STATUS_CHOICES = [
        ('PENDING', 'Pending'),
        ('FILLED', 'Filled'),
        ('CANCELLED', 'Cancelled'),
        ('REJECTED', 'Rejected'),
    ]
    
    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    side = models.CharField(max_length=10, choices=SIDE_CHOICES)
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    price = models.DecimalField(max_digits=20, decimal_places=8)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default='PENDING')
    
    class Meta:
        ordering = ['-created_at']

class Strategy(BaseModel):
    """Estratégias de trading"""
    name = models.CharField(max_length=255)
    config = models.JSONField(default=dict, help_text="Configurações da estratégia")
    is_active = models.BooleanField(default=True)
    
    class Meta:
        verbose_name_plural = 'strategies'
    
    def __str__(self):
        return self.name
    
class Operation(BaseModel):
    """Operações de trading"""
    OPERATION_TYPES = [
        ('LONG', 'Long Position'),
        ('SHORT', 'Short Position'),
        ('HEDGE', 'Hedge Position'),
    ]
    
    STATUS_CHOICES = [
        ('ACTIVE', 'Active'),
        ('CLOSED', 'Closed'),
        ('CANCELLED', 'Cancelled'),
    ]
    
    strategy = models.ForeignKey(Strategy, on_delete=models.CASCADE)
    symbol = models.ForeignKey(Symbol, on_delete=models.CASCADE)
    operation_type = models.CharField(max_length=10, choices=OPERATION_TYPES)
    status = models.CharField(max_length=20, choices=STATUS_CHOICES, default='ACTIVE')
    
    # Position details
    entry_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    exit_price = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    quantity = models.DecimalField(max_digits=20, decimal_places=8)
    
    # Risk management
    stop_loss = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    take_profit = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    
    # Timing
    opened_at = models.DateTimeField(auto_now_add=True)
    closed_at = models.DateTimeField(null=True, blank=True)
    
    class Meta:
        ordering = ['-opened_at']
    
    def __str__(self):
        return f"Operation {self.id} - {self.symbol.name} ({self.operation_type})"

class Position(BaseModel):
    """Posições ativas"""
    operation = models.ForeignKey(Operation, on_delete=models.CASCADE)
    current_price = models.DecimalField(max_digits=20, decimal_places=8)
    unrealized_pnl = models.DecimalField(max_digits=20, decimal_places=8, default=0)
    
    class Meta:
        ordering = ['-created_at']
    
    def __str__(self):
        return f"Position {self.id} - Operation {self.operation.id}"

class Trade(BaseModel):
    """Histórico de trades executados"""
    operation = models.ForeignKey(Operation, on_delete=models.CASCADE)
    order = models.ForeignKey(Order, on_delete=models.CASCADE)
    executed_price = models.DecimalField(max_digits=20, decimal_places=8)
    executed_quantity = models.DecimalField(max_digits=20, decimal_places=8)
    commission = models.DecimalField(max_digits=20, decimal_places=8, default=0)
    
    class Meta:
        ordering = ['-created_at']
    
    def __str__(self):
        return f"Trade {self.id} - {self.executed_quantity} @ {self.executed_price}"
