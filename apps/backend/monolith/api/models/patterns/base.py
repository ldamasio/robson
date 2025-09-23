"""Core pattern catalog and instance models."""

from django.db import models

from ..base import BaseModel
from ..trading import Symbol


class PatternCategory(models.TextChoices):
    CHART = "CHART", "Chart"
    CANDLESTICK = "CANDLESTICK", "Candlestick"
    HARMONIC = "HARMONIC", "Harmonic"
    ELLIOTT = "ELLIOTT", "Elliott Wave"
    WYCKOFF = "WYCKOFF", "Wyckoff"
    INDICATOR = "INDICATOR", "Indicator"
    CYCLE = "CYCLE", "Cyclical"
    HYBRID = "HYBRID", "Hybrid"


class PatternDirectionBias(models.TextChoices):
    BULLISH = "BULLISH", "Bullish"
    BEARISH = "BEARISH", "Bearish"
    CONTINUATION = "CONTINUATION", "Continuation"
    NEUTRAL = "NEUTRAL", "Neutral"


class PatternStatus(models.TextChoices):
    FORMING = "FORMING", "Forming"
    CONFIRMED = "CONFIRMED", "Confirmed"
    FAILED = "FAILED", "Failed"
    INVALIDATED = "INVALIDATED", "Invalidated"
    TARGET_HIT = "TARGET_HIT", "Target Hit"
    EXPIRED = "EXPIRED", "Expired"


class BreakoutDirection(models.TextChoices):
    UP = "UP", "Up"
    DOWN = "DOWN", "Down"
    NONE = "NONE", "None"


class VolumeProfile(models.TextChoices):
    RISING = "RISING", "Rising"
    FALLING = "FALLING", "Falling"
    MIXED = "MIXED", "Mixed"


class ConfirmationMethod(models.TextChoices):
    CLOSE = "CLOSE", "Close"
    INTRABAR = "INTRABAR", "Intrabar"
    VOLUME = "VOLUME", "Volume"
    RETEST = "RETEST", "Retest"
    STRUCTURE = "STRUCTURE", "Structure"


class StopMethod(models.TextChoices):
    SWING = "SWING", "Swing"
    STRUCTURE = "STRUCTURE", "Structure"
    ATR = "ATR", "ATR"
    VWAP = "VWAP", "VWAP"
    CUSTOM = "CUSTOM", "Custom"


class TrendPrecondition(models.TextChoices):
    UPTREND = "UPTREND", "Uptrend"
    DOWNTREND = "DOWNTREND", "Downtrend"
    SIDEWAYS = "SIDEWAYS", "Sideways"


class HighTimeframeAlignment(models.TextChoices):
    NONE = "NONE", "None"
    WEAK = "WEAK", "Weak"
    STRONG = "STRONG", "Strong"


class PatternCatalog(BaseModel):
    """Formal catalog of supported technical patterns."""

    pattern_code = models.CharField(max_length=32, unique=True)
    name = models.CharField(max_length=128)
    category = models.CharField(max_length=16, choices=PatternCategory.choices)
    direction_bias = models.CharField(
        max_length=16,
        choices=PatternDirectionBias.choices,
        default=PatternDirectionBias.NEUTRAL,
    )
    min_bars = models.PositiveIntegerField(default=1)
    max_bars = models.PositiveIntegerField(null=True, blank=True)
    min_touches = models.PositiveIntegerField(default=0)
    requires_gap = models.BooleanField(default=False)
    fib_tolerance_pct = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Tolerance percentage for Fibonacci-derived rules",
    )
    default_confirmation_rule = models.TextField(blank=True, default="")
    default_invalidation_rule = models.TextField(blank=True, default="")
    theory_notes = models.TextField(blank=True, default="")
    references = models.TextField(blank=True, default="")
    metadata = models.JSONField(default=dict, blank=True)

    class Meta:
        ordering = ["pattern_code"]
        verbose_name = "Pattern Catalog Entry"
        verbose_name_plural = "Pattern Catalog"

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"{self.pattern_code} ({self.get_category_display()})"


class PatternInstance(BaseModel):
    """Concrete detection of a pattern on an instrument/timeframe."""

    pattern = models.ForeignKey(
        PatternCatalog,
        on_delete=models.PROTECT,
        related_name="instances",
    )
    symbol = models.ForeignKey(
        Symbol,
        on_delete=models.CASCADE,
        related_name="pattern_instances",
    )
    timeframe = models.CharField(max_length=16, default="1h")
    start_ts = models.DateTimeField()
    end_ts = models.DateTimeField(null=True, blank=True)
    breakout_ts = models.DateTimeField(null=True, blank=True)
    status = models.CharField(
        max_length=16,
        choices=PatternStatus.choices,
        default=PatternStatus.FORMING,
    )
    trend_precondition = models.CharField(
        max_length=16,
        choices=TrendPrecondition.choices,
        blank=True,
        default="",
    )
    pre_pattern_return_pct = models.DecimalField(
        max_digits=7,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Percentage return leading into the pattern",
    )
    pre_pattern_atr = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
        help_text="ATR level before the pattern started",
    )
    vol_regime_z = models.DecimalField(
        max_digits=6,
        decimal_places=3,
        null=True,
        blank=True,
        help_text="Z-score of volume regime prior to detection",
    )
    volume_profile = models.CharField(
        max_length=8,
        choices=VolumeProfile.choices,
        blank=True,
        default="",
    )
    breakout_direction = models.CharField(
        max_length=8,
        choices=BreakoutDirection.choices,
        default=BreakoutDirection.NONE,
    )
    breakout_volume_z = models.DecimalField(
        max_digits=6,
        decimal_places=3,
        null=True,
        blank=True,
        help_text="Volume Z-score at breakout",
    )
    climax_volume_flag = models.BooleanField(default=False)
    confirm_method = models.CharField(
        max_length=16,
        choices=ConfirmationMethod.choices,
        blank=True,
        default="",
    )
    confirm_strength = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Strength score [0-1] for confirmation",
    )
    invalidation_level = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
    )
    measured_move = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text="Measured move projection in price units",
    )
    tp1 = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    tp2 = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    tp3 = models.DecimalField(max_digits=20, decimal_places=8, null=True, blank=True)
    stop_method = models.CharField(
        max_length=16,
        choices=StopMethod.choices,
        blank=True,
        default="",
    )
    r_multiple_plan = models.JSONField(
        default=dict,
        blank=True,
        help_text="Serialized plan for R multiple management",
    )
    pattern_score = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=0.0,
        help_text="Composite pattern quality score (0-100)",
    )
    reliability_est = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
        help_text="Estimated reliability (0-1)",
    )
    historical_winrate_est = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        null=True,
        blank=True,
    )
    edge_z = models.DecimalField(
        max_digits=6,
        decimal_places=3,
        null=True,
        blank=True,
        help_text="Statistical edge score (Z)",
    )
    notes = models.TextField(blank=True, default="")
    htf_alignment = models.CharField(
        max_length=8,
        choices=HighTimeframeAlignment.choices,
        default=HighTimeframeAlignment.NONE,
    )
    htf_trend = models.CharField(max_length=32, blank=True, default="")
    lt_tf_confirm = models.BooleanField(default=False)
    detected_version = models.CharField(max_length=32, blank=True, default="")
    features = models.JSONField(
        default=dict,
        blank=True,
        help_text="Flexible storage for detector-specific metrics",
    )

    class Meta:
        ordering = ["-created_at"]
        indexes = [
            models.Index(fields=["symbol", "status", "breakout_ts"]),
            models.Index(fields=["pattern", "status"]),
        ]

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"{self.pattern.pattern_code} @ {self.symbol.name} ({self.timeframe})"


class PatternPoint(BaseModel):
    """Normalized pivot or leg point for reproducibility."""

    instance = models.ForeignKey(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="points",
    )
    label = models.CharField(max_length=16)
    ts = models.DateTimeField()
    price = models.DecimalField(max_digits=20, decimal_places=8)
    bar_index_offset = models.IntegerField(
        null=True,
        blank=True,
        help_text="Offset relative to pattern start",
    )
    role = models.CharField(max_length=32, blank=True, default="")

    class Meta:
        ordering = ["ts"]

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"{self.label}@{self.ts.isoformat()}"


class PatternOutcome(BaseModel):
    """Post-detection analytics for model calibration."""

    instance = models.OneToOneField(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="outcome",
    )
    evaluation_window_bars = models.PositiveIntegerField(default=0)
    max_favorable_excursion = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
    )
    max_adverse_excursion = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
    )
    hit_tp1 = models.BooleanField(default=False)
    hit_tp2 = models.BooleanField(default=False)
    hit_tp3 = models.BooleanField(default=False)
    stopped_out = models.BooleanField(default=False)
    peak_return_pct = models.DecimalField(
        max_digits=7,
        decimal_places=2,
        null=True,
        blank=True,
    )
    time_to_target_bars = models.PositiveIntegerField(null=True, blank=True)

    class Meta:
        verbose_name = "Pattern Outcome"
        verbose_name_plural = "Pattern Outcomes"


class PatternAlert(BaseModel):
    """Alerts emitted during pattern lifecycle."""

    class AlertType(models.TextChoices):
        FORMING = "FORMING", "Forming"
        CONFIRM = "CONFIRM", "Confirm"
        RETEST = "RETEST", "Retest"
        INVALIDATE = "INVALIDATE", "Invalidate"
        TARGET_HIT = "TARGET_HIT", "Target Hit"

    instance = models.ForeignKey(
        PatternInstance,
        on_delete=models.CASCADE,
        related_name="alerts",
    )
    alert_ts = models.DateTimeField(auto_now_add=True)
    alert_type = models.CharField(max_length=16, choices=AlertType.choices)
    confidence = models.DecimalField(
        max_digits=5,
        decimal_places=2,
        default=0.0,
        help_text="Confidence at alert generation",
    )
    payload = models.JSONField(default=dict, blank=True)

    class Meta:
        ordering = ["-alert_ts"]

    def __str__(self) -> str:  # pragma: no cover - readability helper
        return f"Alert {self.alert_type} for {self.instance_id}"
