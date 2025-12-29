"""
Entry Gate models for opportunity execution control.

These models store configuration and audit trail for entry gating decisions.
"""

import uuid
from decimal import Decimal
from django.db import models
from django.db.models import Index

from .base import TenantMixin, TimestampMixin


class EntryGateConfig(TenantMixin, TimestampMixin):
    """
    Entry gate configuration per tenant.

    Note: 4% monthly / 1% per operation are CONSTANTS (not configurable).
    These are stored in the domain layer as business rules.

    This model stores only the OPTIONAL/configurable gates:
    - Cooldown settings
    - Market context gate thresholds
    """

    # Cooldown settings
    enable_cooldown = models.BooleanField(
        default=True,
        help_text="Enable cooldown period after stop-out"
    )
    cooldown_after_stop_seconds = models.IntegerField(
        default=900,  # 15 minutes
        help_text="Cooldown period in seconds after a stop-out event"
    )

    # Market context gates
    enable_funding_rate_gate = models.BooleanField(
        default=True,
        help_text="Enable extreme funding rate check"
    )
    funding_rate_threshold = models.DecimalField(
        max_digits=10,
        decimal_places=6,
        default=Decimal('0.0001'),  # 0.01%
        help_text="Funding rate threshold (absolute value)"
    )

    enable_stale_data_gate = models.BooleanField(
        default=True,
        help_text="Enable stale market data check"
    )
    max_data_age_seconds = models.IntegerField(
        default=300,  # 5 minutes
        help_text="Maximum acceptable age of market data in seconds"
    )

    class Meta:
        db_table = 'entry_gate_config'
        verbose_name = 'Entry Gate Configuration'
        verbose_name_plural = 'Entry Gate Configurations'
        # One config per client
        unique_together = [['client']]

    def __str__(self):
        return f"EntryGateConfig(client={self.client_id}, cooldown={self.enable_cooldown})"


class EntryGateDecisionModel(TenantMixin, TimestampMixin):
    """
    Audit trail of entry gate decisions.

    Every gate evaluation is recorded here for transparency and analysis.
    This is an append-only table (decisions are never updated or deleted).
    """

    decision_id = models.UUIDField(
        primary_key=True,
        default=uuid.uuid4,
        editable=False,
        help_text="Unique decision identifier"
    )

    symbol = models.CharField(
        max_length=20,
        help_text="Trading pair (e.g., BTCUSDT)"
    )

    allowed = models.BooleanField(
        help_text="True if entry was allowed, False if denied"
    )

    reasons = models.JSONField(
        help_text="List of human-readable reasons (failures + successes)"
    )

    gate_checks = models.JSONField(
        help_text="Detailed results for each gate check"
    )

    context = models.JSONField(
        default=dict,
        help_text="Additional context for debugging (side, price, etc.)"
    )

    class Meta:
        db_table = 'entry_gate_decisions'
        verbose_name = 'Entry Gate Decision'
        verbose_name_plural = 'Entry Gate Decisions'
        indexes = [
            Index(fields=['client', '-created_at'], name='idx_decisions_client_time'),
            Index(fields=['symbol', '-created_at'], name='idx_decisions_symbol_time'),
            Index(fields=['allowed', '-created_at'], name='idx_decisions_allowed_time'),
        ]
        # Most recent first
        ordering = ['-created_at']

    def __str__(self):
        status = "ALLOWED" if self.allowed else "DENIED"
        return f"EntryGate({self.symbol} @ {self.created_at.strftime('%Y-%m-%d %H:%M')}: {status})"

    @property
    def age_seconds(self):
        """How long ago was this decision made (in seconds)."""
        return (models.functions.Now() - self.created_at).total_seconds()
