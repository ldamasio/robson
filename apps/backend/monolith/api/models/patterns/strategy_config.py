"""
Strategy-Pattern Auto-Entry Configuration.

Links Trading Strategies with Pattern types for automatic entry decisions.
"""

from decimal import Decimal

from django.db import models
from django.core.exceptions import ValidationError

from ..base import BaseModel
from .base import PatternCatalog
from ..trading import Strategy


class StrategyPatternConfig(BaseModel):
    """
    Configuration for auto-entry based on detected patterns.

    Defines which patterns a strategy should respond to and how.

    Example:
        strategy = "Mean Reversion MA99"
        pattern = PatternCatalog.objects.get(pattern_code="HAMMER")
        config = StrategyPatternConfig.objects.create(
            strategy=strategy,
            pattern=pattern,
            auto_entry_enabled=True,
            min_confidence=0.75,
            max_entries_per_day=3,
        )
    """

    class EntryMode(models.TextChoices):
        SUGGEST_ONLY = "SUGGEST", "Suggest Only (User Confirms)"
        AUTO_ENTRY = "AUTO", "Auto-Entry (Requires Acknowledgement)"

    strategy = models.ForeignKey(
        Strategy,
        on_delete=models.CASCADE,
        related_name="pattern_configs",
    )
    pattern = models.ForeignKey(
        PatternCatalog,
        on_delete=models.CASCADE,
        related_name="strategy_configs",
    )
    auto_entry_enabled = models.BooleanField(
        default=False,
        help_text="Enable auto-entry for this pattern+strategy combination",
    )
    entry_mode = models.CharField(
        max_length=16,
        choices=EntryMode.choices,
        default=EntryMode.SUGGEST_ONLY,
        help_text="SUGGEST_ONLY: User must confirm. AUTO_ENTRY: Automatic entry.",
    )
    min_confidence = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=Decimal("0.70"),
        help_text="Minimum pattern confidence [0-1] to trigger entry",
    )
    max_entries_per_day = models.PositiveIntegerField(
        default=3,
        help_text="Maximum entries per day for this pattern+strategy",
    )
    max_entries_per_week = models.PositiveIntegerField(
        default=10,
        help_text="Maximum entries per week for this pattern+strategy",
    )
    cooldown_minutes = models.PositiveIntegerField(
        default=60,
        help_text="Cooldown between entries for same pattern (minutes)",
    )
    position_size_pct = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Position size as % of capital (overrides strategy default)",
    )
    require_confirmation = models.BooleanField(
        default=True,
        help_text="Require pattern confirmation before entry",
    )
    timeframes = models.JSONField(
        default=list,
        blank=True,
        help_text="List of allowed timeframes (empty = all). E.g., ['15m', '1h']",
    )
    symbols = models.JSONField(
        default=list,
        blank=True,
        help_text="List of allowed symbols (empty = all). E.g., ['BTCUSDT', 'ETHUSDT']",
    )
    require_volume_confirmation = models.BooleanField(
        default=False,
        help_text="Require above-average volume on breakout",
    )
    only_with_trend = models.BooleanField(
        default=False,
        help_text="Only enter if pattern aligns with higher timeframe trend",
    )

    class Meta:
        unique_together = ["strategy", "pattern"]
        verbose_name = "Strategy Pattern Config"
        verbose_name_plural = "Strategy Pattern Configs"
        indexes = [
            models.Index(fields=["strategy", "auto_entry_enabled"]),
            models.Index(fields=["pattern", "auto_entry_enabled"]),
        ]

    def __str__(self) -> str:
        return f"{self.strategy.name} + {self.pattern.pattern_code} ({self.get_entry_mode_display()})"

    def clean(self):
        """Validate configuration constraints."""
        if self.auto_entry_enabled and self.entry_mode == self.EntryMode.SUGGEST_ONLY:
            raise ValidationError(
                "Cannot have auto_entry_enabled=True with entry_mode=SUGGEST_ONLY. "
                "Set entry_mode=AUTO_ENTRY or disable auto_entry."
            )

        if self.min_confidence < 0 or self.min_confidence > 1:
            raise ValidationError("min_confidence must be between 0 and 1")

        if self.position_size_pct is not None and (
            self.position_size_pct <= 0 or self.position_size_pct > 100
        ):
            raise ValidationError("position_size_pct must be between 0 and 100")

    def should_trigger_entry(self, pattern_instance: "PatternInstance") -> bool:
        """
        Check if this config should trigger an entry for the given pattern instance.

        Args:
            pattern_instance: Detected pattern instance

        Returns:
            True if entry should be triggered
        """
        from .base import PatternInstance

        # Check auto-entry enabled
        if not self.auto_entry_enabled:
            return False

        # Check confidence threshold
        confidence = pattern_instance.features.get("confidence", 0) if pattern_instance.features else 0
        if float(confidence) < float(self.min_confidence):
            return False

        # Check confirmation requirement
        if self.require_confirmation and pattern_instance.status != "CONFIRMED":
            return False

        # Check timeframe filter
        if self.timeframes and pattern_instance.timeframe not in self.timeframes:
            return False

        # Check symbol filter
        if self.symbols:
            symbol_name = pattern_instance.symbol.name
            if symbol_name not in self.symbols:
                return False

        # Check rate limits (entries per day/week)
        if not self._check_rate_limits(pattern_instance):
            return False

        # Check cooldown
        if not self._check_cooldown(pattern_instance):
            return False

        return True

    def _check_rate_limits(self, pattern_instance: "PatternInstance") -> bool:
        """Check if rate limits allow entry."""
        from django.utils import timezone
        from datetime import timedelta

        now = timezone.now()
        today_start = now.replace(hour=0, minute=0, second=0, microsecond=0)
        week_start = today_start - timedelta(days=today_start.weekday())

        # Count recent entries for this strategy+pattern
        # (This would need to be implemented with actual Operation/Plan tracking)
        # For now, return True as placeholder
        return True

    def _check_cooldown(self, pattern_instance: "PatternInstance") -> bool:
        """Check if cooldown period has passed since last entry."""
        from django.utils import timezone
        from datetime import timedelta

        # Check last entry time for this strategy+pattern
        # For now, return True as placeholder
        return True

    @classmethod
    def get_active_configs_for_pattern(
        cls, pattern_code: str, symbol: str = None, timeframe: str = None
    ) -> "QuerySet[StrategyPatternConfig]":
        """
        Get all active strategy configs for a given pattern.

        Args:
            pattern_code: Pattern code (e.g., "HAMMER")
            symbol: Optional symbol filter
            timeframe: Optional timeframe filter

        Returns:
            QuerySet of active StrategyPatternConfig instances
        """
        from django.db.models import Q

        qs = cls.objects.filter(
            pattern__pattern_code=pattern_code,
            auto_entry_enabled=True,
            strategy__is_active=True,
        )

        # Filter by symbol/timeframe if specified (using JSON field contains)
        if symbol:
            qs = qs.filter(Q(symbols__contains=[]) | Q(symbols__contains=[symbol]))

        if timeframe:
            qs = qs.filter(Q(timeframes__contains=[]) | Q(timeframes__contains=[timeframe]))

        return qs.select_related("strategy", "pattern")
