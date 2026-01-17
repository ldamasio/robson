"""
Trading Thesis Django Models.

Thesis = Hypothesis about the market (NO execution)
Strategy = Executable trading logic

A thesis can exist alone forever. Some become strategies, most don't.
"""

from decimal import Decimal
from django.db import models
from django.core.exceptions import ValidationError

from .base import BaseModel, TenantManager


class TradingThesisModel(BaseModel):
    """
    A trading hypothesis expressed in natural language.

    Unlike a Strategy, a Thesis does NOT execute trades.
    It is a structured observation/diary entry that can be
    monitored and optionally converted to a Strategy later.

    Core Philosophy:
    - THESIS = "I think X might happen" (observation)
    - STRATEGY = "Execute order when Y occurs" (automation)
    - Not every thesis should become a strategy
    """

    STATUS_CHOICES = [
        ("draft", "Draft - Being created/refined"),
        ("active", "Active - Monitoring for trigger/invalidation"),
        ("validated", "Validated - Trigger occurred, thesis was correct"),
        ("rejected", "Rejected - Invalidation occurred, thesis was wrong"),
        ("expired", "Expired - Time-based expiry"),
        ("converted", "Converted - Converted to executable strategy"),
    ]

    # Basic info
    title = models.CharField(max_length=255)
    symbol = models.CharField(max_length=20, help_text="Trading symbol (e.g., BTCUSDT)")
    timeframe = models.CharField(max_length=10, help_text="Timeframe (e.g., 4h, 1d)")

    # The 4 required elements (in natural language)
    market_context = models.TextField(
        help_text="What is happening in the market"
    )
    rationale = models.TextField(
        help_text="Why this opportunity might exist"
    )
    expected_trigger = models.TextField(
        help_text="What needs to happen to confirm"
    )
    invalidation = models.TextField(
        help_text="What proves the thesis wrong"
    )

    # Optional metadata
    hypothesis_type = models.CharField(
        max_length=50,
        blank=True,
        null=True,
        help_text="Type: breakout, mean_reversion, trend_following, etc."
    )
    confidence_level = models.CharField(
        max_length=20,
        blank=True,
        null=True,
        choices=[("low", "Low"), ("medium", "Medium"), ("high", "High")],
        help_text="Confidence level in the thesis"
    )
    tags = models.JSONField(
        default=list,
        blank=True,
        help_text="Tags for categorization"
    )
    notes = models.TextField(
        blank=True,
        null=True,
        help_text="Additional notes or observations"
    )

    # System fields
    status = models.CharField(
        max_length=20,
        choices=STATUS_CHOICES,
        default="draft",
        db_index=True,
        help_text="Current status of the thesis"
    )

    # Tracking
    validated_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text="When the thesis was validated"
    )
    converted_to_strategy_id = models.CharField(
        max_length=100,
        blank=True,
        null=True,
        help_text="ID of strategy if converted"
    )

    # Managers
    objects = TenantManager()

    class Meta:
        verbose_name = "Trading Thesis"
        verbose_name_plural = "Trading Theses"
        ordering = ["-created_at"]
        indexes = [
            models.Index(fields=["client", "status", "created_at"], name="thesis_client_status_idx"),
            models.Index(fields=["symbol", "status"], name="thesis_symbol_status_idx"),
        ]

    def __str__(self):
        return f"{self.title} ({self.symbol} {self.timeframe}) - {self.status}"

    def clean(self):
        """Validate that all 4 required elements are present."""
        super().clean()
        errors = {}

        if not self.market_context or not self.market_context.strip():
            errors["market_context"] = "Market context is required"
        if not self.rationale or not self.rationale.strip():
            errors["rationale"] = "Rationale is required"
        if not self.expected_trigger or not self.expected_trigger.strip():
            errors["expected_trigger"] = "Expected trigger is required"
        if not self.invalidation or not self.invalidation.strip():
            errors["invalidation"] = "Invalidation is required"

        if errors:
            raise ValidationError(errors)

    def save(self, *args, **kwargs):
        self.full_clean()
        super().save(*args, **kwargs)

    # State transitions
    def activate(self):
        """Move thesis to active monitoring state."""
        if self.status != "draft":
            raise ValidationError(f"Cannot activate thesis from status: {self.status}")
        self.status = "active"
        self.save(update_fields=["status", "updated_at"])

    def validate(self):
        """Mark thesis as validated (trigger occurred)."""
        if self.status != "active":
            raise ValidationError(f"Cannot validate thesis from status: {self.status}")
        from django.utils import timezone
        self.status = "validated"
        self.validated_at = timezone.now()
        self.save(update_fields=["status", "validated_at", "updated_at"])

    def reject(self):
        """Mark thesis as rejected (invalidation occurred)."""
        if self.status != "active":
            raise ValidationError(f"Cannot reject thesis from status: {self.status}")
        self.status = "rejected"
        self.save(update_fields=["status", "updated_at"])

    def convert_to_strategy(self, strategy_id: str):
        """Mark thesis as converted to strategy."""
        if self.status != "validated":
            raise ValidationError(f"Cannot convert thesis from status: {self.status}")
        self.status = "converted"
        self.converted_to_strategy_id = strategy_id
        self.save(update_fields=["status", "converted_to_strategy_id", "updated_at"])

    # Computed properties
    @property
    def is_monitoring(self) -> bool:
        """Check if thesis is in monitoring state."""
        return self.status == "active"

    @property
    def is_terminal(self) -> bool:
        """Check if thesis is in terminal state."""
        return self.status in {
            "validated",
            "rejected",
            "expired",
            "converted",
        }

    @property
    def display_name(self) -> str:
        """Get display name for UI."""
        return f"{self.title} ({self.symbol} {self.timeframe})"

    def to_domain(self) -> "core.domain.thesis.TradingThesis":
        """Convert to domain entity."""
        from core.domain.thesis import TradingThesis, ThesisStatus

        return TradingThesis(
            id=str(self.id),
            tenant_id=str(self.client.id),
            title=self.title,
            symbol=self.symbol,
            timeframe=self.timeframe,
            market_context=self.market_context,
            rationale=self.rationale,
            expected_trigger=self.expected_trigger,
            invalidation=self.invalidation,
            hypothesis_type=self.hypothesis_type,
            confidence_level=self.confidence_level,
            tags=self.tags or [],
            notes=self.notes,
            status=ThesisStatus(self.status),
            created_at=self.created_at,
            updated_at=self.updated_at,
            validated_at=self.validated_at,
            converted_to_strategy_id=self.converted_to_strategy_id,
        )
