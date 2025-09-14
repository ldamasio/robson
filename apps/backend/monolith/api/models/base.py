# api/models/base.py
"""
Base mixins and classes for all Robson Bot models.
Centralizes common functionality and promotes code reuse.
"""

from django.db import models
from django.core.exceptions import ValidationError
from django.utils import timezone
from clients.models import Client
import logging

logger = logging.getLogger(__name__)

# ==========================================
# MIXINS BASE
# ==========================================

class TimestampMixin(models.Model):
    """
    Mixin that adds automatic timestamp fields.
    Used by virtually all models in the system.
    """
    created_at = models.DateTimeField(
        auto_now_add=True,
        help_text="Record creation timestamp"
    )
    updated_at = models.DateTimeField(
        auto_now=True,
        help_text="Last update timestamp"
    )
    
    class Meta:
        abstract = True
    
    @property
    def age(self):
        """Returns the age of the record"""
        return timezone.now() - self.created_at
    
    @property
    def time_since_last_update(self):
        """Returns how long ago the record was last updated"""
        return timezone.now() - self.updated_at

class TenantMixin(models.Model):
    """
    Mixin for multi-tenancy based on Client.
    Ensures data isolation between different clients.
    """
    client = models.ForeignKey(
        Client,
        blank=True,
        null=True,
        on_delete=models.SET_NULL,
        help_text="Client that owns this record"
    )
    tenant_id = 'client_id'  # Used by django-multitenant if implemented
    
    class Meta:
        abstract = True
    
    def clean(self):
        """Custom validation for tenant"""
        super().clean()
        if not self.client:
            logger.warning(f"Record created without client: {self.__class__.__name__}")
    
    @property
    def client_name(self):
        """Client name in a safe way"""
        return self.client.name if self.client else "No Client"

class MarketTypeMixin(models.Model):
    """
    Mixin for entities that have market direction (Bullish/Bearish).
    Used in technical analysis, signals, patterns, etc.
    """
    MARKET_TYPES = [
        ('BULLISH', 'Bullish'),
        ('BEARISH', 'Bearish'),
        ('NEUTRAL', 'Neutral'),  # Added for flexibility
    ]
    
    type = models.CharField(
        max_length=10,
        choices=MARKET_TYPES,
        help_text="Market direction (Bull, Bear, Neutral)"
    )
    
    class Meta:
        abstract = True
    
    @property
    def is_bullish(self):
        """Check if it's bullish"""
        return self.type == 'BULLISH'
    
    @property
    def is_bearish(self):
        """Check if it's bearish"""
        return self.type == 'BEARISH'
    
    @property
    def market_direction_icon(self):
        """Visual icon for market direction"""
        icons = {
            'BULLISH': '📈',
            'BEARISH': '📉', 
            'NEUTRAL': '➡️'
        }
        return icons.get(self.type, '❓')

class DescriptionMixin(models.Model):
    """
    Mixin for entities that need detailed description.
    """
    description = models.TextField(
        help_text="Detailed description"
    )
    
    class Meta:
        abstract = True
    
    @property
    def short_description(self):
        """Shortened version of description (first 100 chars)"""
        if len(self.description) <= 100:
            return self.description
        return f"{self.description[:97]}..."

class ExperienceMixin(models.Model):
    """
    Mixin for entities that have experience/complexity level.
    Used in strategies, technical patterns, etc.
    """
    EXPERIENCE_LEVELS = [
        (1, 'Beginner'),
        (2, 'Basic'),
        (3, 'Intermediate'),
        (4, 'Advanced'),
        (5, 'Expert'),
    ]
    
    experience = models.IntegerField(
        choices=EXPERIENCE_LEVELS,
        default=1,
        help_text="Required experience level (1-5)"
    )
    
    class Meta:
        abstract = True
    
    @property
    def experience_label(self):
        """Experience level label"""
        return dict(self.EXPERIENCE_LEVELS).get(self.experience, 'Unknown')
    
    @property
    def difficulty_stars(self):
        """Visual representation of difficulty"""
        return '⭐' * self.experience

class StatusMixin(models.Model):
    """
    Mixin for entities that have active/inactive status.
    """
    is_active = models.BooleanField(
        default=True,
        help_text="Indicates if this record is active"
    )
    
    class Meta:
        abstract = True
    
    @property
    def status_icon(self):
        """Status icon"""
        return '✅' if self.is_active else '❌'

# ==========================================
# COMBINED BASE CLASSES
# ==========================================

class BaseModel(TenantMixin, TimestampMixin, StatusMixin):
    """
    Main base model that combines the most common mixins.
    Use for most models in the system.
    """
    
    class Meta:
        abstract = True
    
    def save(self, *args, **kwargs):
        """Override save for logging and validations"""
        # Call clean() automatically
        self.full_clean()
        
        # Log operation
        operation = "UPDATE" if self.pk else "CREATE"
        logger.info(
            f"{operation} {self.__class__.__name__} "
            f"for client {self.client_name}"
        )
        
        super().save(*args, **kwargs)

class BaseTechnicalModel(BaseModel, MarketTypeMixin, DescriptionMixin):
    """
    Base model for technical analysis entities.
    Combines common functionality for patterns, indicators, signals, etc.
    """
    
    confidence = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=0.00,
        help_text="Confidence level (0-100%)"
    )
    
    class Meta:
        abstract = True
    
    def clean(self):
        """Specific validations for technical analysis"""
        super().clean()
        
        # Validate confidence
        if self.confidence < 0 or self.confidence > 100:
            raise ValidationError("Confidence must be between 0 and 100")
    
    @property
    def confidence_percentage(self):
        """Confidence formatted as percentage"""
        return f"{self.confidence}%"
    
    @property
    def confidence_level(self):
        """Confidence level in words"""
        if self.confidence >= 80:
            return "Very High"
        elif self.confidence >= 60:
            return "High"
        elif self.confidence >= 40:
            return "Medium"
        elif self.confidence >= 20:
            return "Low"
        else:
            return "Very Low"

class BaseConfigModel(BaseModel, DescriptionMixin):
    """
    Base model for system configurations and rules.
    Used for risk management, trading rules, etc.
    """
    
    name = models.CharField(
        max_length=255,
        help_text="Configuration name"
    )
    
    class Meta:
        abstract = True
    
    def __str__(self):
        return f"{self.name} ({self.client_name})"

class BaseFinancialModel(BaseModel):
    """
    Base model for financial entities (orders, positions, etc).
    Includes common fields for monetary values.
    """
    
    # Common financial fields
    amount = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Value/quantity"
    )
    
    price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Unit price"
    )
    
    class Meta:
        abstract = True
    
    @property
    def total_value(self):
        """Total value (amount * price)"""
        if self.amount and self.price:
            return self.amount * self.price
        return None
    
    def clean(self):
        """Financial validations"""
        super().clean()
        
        if self.amount is not None and self.amount < 0:
            raise ValidationError("Amount cannot be negative")
        
        if self.price is not None and self.price < 0:
            raise ValidationError("Price cannot be negative")

# ==========================================
# CUSTOM MANAGERS
# ==========================================

class ActiveManager(models.Manager):
    """Manager that returns only active records"""
    
    def get_queryset(self):
        return super().get_queryset().filter(is_active=True)

class TenantManager(models.Manager):
    """Manager for filtering by client"""
    
    def for_client(self, client_id):
        """Returns records for a specific client"""
        return self.get_queryset().filter(client_id=client_id)
    
    def active_for_client(self, client_id):
        """Returns active records for a specific client"""
        return self.get_queryset().filter(
            client_id=client_id,
            is_active=True
        )

# ==========================================
# UTILITIES
# ==========================================

class ModelChoices:
    """Utility class for common choices"""
    
    # Side choices for orders
    ORDER_SIDES = [
        ('BUY', 'Buy'),
        ('SELL', 'Sell'),
    ]
    
    # Status choices for orders
    ORDER_STATUS = [
        ('PENDING', 'Pending'),
        ('FILLED', 'Filled'),
        ('PARTIALLY_FILLED', 'Partially Filled'),
        ('CANCELLED', 'cancelled'),
        ('REJECTED', 'Rejected'),
        ('EXPIRED', 'Expired'),
    ]
    
    # Timeframes for analysis
    TIMEFRAMES = [
        ('1m', '1 Minute'),
        ('5m', '5 Minutes'),
        ('15m', '15 Minutes'),
        ('30m', '30 Minutes'),
        ('1h', '1 Hour'),
        ('2h', '2 Hours'),
        ('4h', '4 Hours'),
        ('6h', '6 Hours'),
        ('8h', '8 Hours'),
        ('12h', '12 Hours'),
        ('1d', '1 Day'),
        ('3d', '3 Days'),
        ('1w', '1 Week'),
        ('1M', '1 Month'),
    ]
    
    # Order types
    ORDER_TYPES = [
        ('MARKET', 'Market'),
        ('LIMIT', 'Limit'),
        ('STOP_LOSS', 'Stop Loss'),
        ('STOP_LOSS_LIMIT', 'Stop Loss Limit'),
        ('TAKE_PROFIT', 'Take Profit'),
        ('TAKE_PROFIT_LIMIT', 'Take Profit Limit'),
    ]