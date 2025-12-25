# api/models/event_sourcing.py
"""
Event Sourcing models for Stop-Loss Monitor.

ADR-0012: Event-Sourced Stop-Loss Monitor with Rust WebSocket Service

These models implement the Event Sourcing pattern for stop-loss execution:
- stop_events: Append-only event store (source of truth)
- stop_executions: Materialized view (derived state)
- tenant_config: Risk guardrails configuration
- circuit_breaker_state: Per-symbol circuit breaker
- outbox: Transactional outbox pattern for event publishing
"""

from decimal import Decimal
from django.db import models
from django.utils import timezone
import uuid

from .base import BaseModel


# =====================================================================
# EVENT TYPES
# =====================================================================

class StopEventType(models.TextChoices):
    """Types of stop-loss events."""
    STOP_TRIGGERED = 'STOP_TRIGGERED', 'Stop Triggered'
    EXECUTION_SUBMITTED = 'EXECUTION_SUBMITTED', 'Execution Submitted'
    EXECUTED = 'EXECUTED', 'Executed'
    FAILED = 'FAILED', 'Failed'
    BLOCKED = 'BLOCKED', 'Blocked'
    STALE_PRICE = 'STALE_PRICE', 'Stale Price'
    KILL_SWITCH = 'KILL_SWITCH', 'Kill Switch Activated'
    SLIPPAGE_BREACH = 'SLIPPAGE_BREACH', 'Slippage Limit Breached'
    CIRCUIT_BREAKER = 'CIRCUIT_BREAKER', 'Circuit Breaker Tripped'


class ExecutionSource(models.TextChoices):
    """Source of stop-loss execution."""
    WEBSOCKET = 'ws', 'WebSocket Service'
    CRONJOB = 'cron', 'CronJob Backstop'
    MANUAL = 'manual', 'Manual Intervention'


class ExecutionStatus(models.TextChoices):
    """Status of stop-loss execution."""
    PENDING = 'PENDING', 'Pending'
    SUBMITTED = 'SUBMITTED', 'Submitted'
    EXECUTED = 'EXECUTED', 'Executed'
    FAILED = 'FAILED', 'Failed'
    BLOCKED = 'BLOCKED', 'Blocked'


class CircuitBreakerState(models.TextChoices):
    """Circuit breaker states."""
    CLOSED = 'CLOSED', 'Closed (Normal Trading)'
    OPEN = 'OPEN', 'Open (Trading Blocked)'
    HALF_OPEN = 'HALF_OPEN', 'Half-Open (Testing Recovery)'


# =====================================================================
# STOP EVENT (Append-Only Event Store)
# =====================================================================

class StopEvent(models.Model):
    """
    Immutable event representing a stop-loss state transition.

    This is the source of truth for all stop-loss executions.
    Events are NEVER updated, only inserted (append-only log).

    Attributes:
        event_id: Unique event identifier
        event_seq: Global sequence number (for ordering/replay)
        occurred_at: When the event occurred
        operation: Related trading operation
        client: Tenant/client (multi-tenant isolation)
        symbol: Trading pair (e.g., BTCUSDC)
        event_type: Type of event (TRIGGERED, EXECUTED, FAILED, etc.)
        trigger_price: Price that triggered the stop
        stop_price: Configured stop level (absolute)
        quantity: Quantity to close
        side: Order side (BUY/SELL)
        execution_token: Global idempotency token
        payload_json: Complete event context
        exchange_order_id: Binance order ID (if executed)
        fill_price: Actual fill price
        slippage_pct: Calculated slippage
        source: Which component emitted this event (ws/cron/manual)
        error_message: Error details if failed
        retry_count: Number of retry attempts
    """

    # Event identity
    event_id = models.UUIDField(
        primary_key=True,
        default=uuid.uuid4,
        editable=False,
        help_text='Unique event identifier'
    )
    event_seq = models.BigAutoField(
        unique=True,
        editable=False,
        help_text='Global sequence number for event ordering'
    )
    occurred_at = models.DateTimeField(
        auto_now_add=True,
        db_index=True,
        help_text='When the event occurred'
    )

    # Operation context
    operation = models.ForeignKey(
        'Operation',
        on_delete=models.CASCADE,
        related_name='stop_events',
        help_text='Related trading operation'
    )
    client = models.ForeignKey(
        'Client',
        on_delete=models.CASCADE,
        related_name='stop_events',
        help_text='Tenant/client for multi-tenant isolation'
    )
    symbol = models.CharField(
        max_length=20,
        db_index=True,
        help_text='Trading pair (e.g., BTCUSDC)'
    )

    # Event type
    event_type = models.CharField(
        max_length=50,
        choices=StopEventType.choices,
        db_index=True,
        help_text='Type of event'
    )

    # Stop-loss parameters (captured at event time)
    trigger_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Price that triggered the stop'
    )
    stop_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Configured stop level (absolute price)'
    )
    quantity = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Quantity to close'
    )
    side = models.CharField(
        max_length=10,
        null=True,
        blank=True,
        choices=[('BUY', 'Buy'), ('SELL', 'Sell')],
        help_text='Order side (closing direction)'
    )

    # Idempotency
    execution_token = models.CharField(
        max_length=64,
        unique=True,
        null=True,
        blank=True,
        db_index=True,
        help_text='Global idempotency token (prevents duplicate executions)'
    )

    # Payload
    payload_json = models.JSONField(
        default=dict,
        blank=True,
        help_text='Complete event context (entry_price, slippage_limit, etc.)'
    )
    request_payload_hash = models.CharField(
        max_length=64,
        null=True,
        blank=True,
        help_text='SHA-256 hash of request payload for deduplication'
    )

    # Execution results
    exchange_order_id = models.CharField(
        max_length=100,
        null=True,
        blank=True,
        db_index=True,
        help_text='Binance order ID (if executed)'
    )
    fill_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Actual fill price from exchange'
    )
    slippage_pct = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
        help_text='Calculated slippage percentage'
    )

    # Source attribution
    source = models.CharField(
        max_length=20,
        choices=ExecutionSource.choices,
        db_index=True,
        help_text='Which component emitted this event'
    )

    # Error tracking
    error_message = models.TextField(
        null=True,
        blank=True,
        help_text='Error details if execution failed'
    )
    retry_count = models.IntegerField(
        default=0,
        help_text='Number of retry attempts'
    )

    class Meta:
        db_table = 'stop_events'
        ordering = ['event_seq']
        verbose_name = 'Stop Event'
        verbose_name_plural = 'Stop Events'
        indexes = [
            models.Index(fields=['operation', 'event_seq'], name='idx_stop_events_op_seq'),
            models.Index(fields=['client', 'occurred_at'], name='idx_stop_events_tenant'),
            models.Index(fields=['event_type', 'occurred_at'], name='idx_stop_events_type'),
            models.Index(fields=['source', 'occurred_at'], name='idx_stop_events_source'),
            models.Index(fields=['symbol', 'occurred_at'], name='idx_stop_events_symbol'),
        ]

    def __str__(self):
        return f"Event#{self.event_seq}: {self.event_type} - Op#{self.operation_id} ({self.source})"


# =====================================================================
# STOP EXECUTION (Materialized View / Projection)
# =====================================================================

class StopExecution(models.Model):
    """
    Materialized view of latest execution state per operation.

    This is DERIVED from stop_events (event replay).
    Provides a convenient view of current execution status.

    Attributes:
        execution_id: Unique execution identifier
        operation: Related trading operation
        client: Tenant/client
        execution_token: Global idempotency token
        status: Current execution status (PENDING, SUBMITTED, EXECUTED, FAILED, BLOCKED)
        stop_price: Fixed technical stop level
        trigger_price: Price at detection
        quantity: Quantity to close
        side: Order side
        triggered_at: When stop was triggered
        submitted_at: When order was submitted
        executed_at: When order was filled
        failed_at: When execution failed
        exchange_order_id: Binance order ID
        fill_price: Actual fill price
        slippage_pct: Calculated slippage
        source: Which component executed this
        error_message: Error details
        retry_count: Retry attempts
    """

    # Primary key
    execution_id = models.UUIDField(
        primary_key=True,
        default=uuid.uuid4,
        editable=False,
        help_text='Unique execution identifier'
    )

    # Operation reference
    operation = models.ForeignKey(
        'Operation',
        on_delete=models.CASCADE,
        related_name='stop_executions',
        help_text='Related trading operation'
    )
    client = models.ForeignKey(
        'Client',
        on_delete=models.CASCADE,
        related_name='stop_executions',
        help_text='Tenant/client for multi-tenant isolation'
    )

    # Idempotency token (unique across ALL executions)
    execution_token = models.CharField(
        max_length=64,
        unique=True,
        db_index=True,
        help_text='Global idempotency token'
    )

    # Execution state
    status = models.CharField(
        max_length=50,
        default=ExecutionStatus.PENDING,
        choices=ExecutionStatus.choices,
        db_index=True,
        help_text='Current execution status'
    )

    # Stop parameters
    stop_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text='Fixed technical stop level'
    )
    trigger_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Price at detection'
    )
    quantity = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        help_text='Quantity to close'
    )
    side = models.CharField(
        max_length=10,
        choices=[('BUY', 'Buy'), ('SELL', 'Sell')],
        help_text='Order side (closing direction)'
    )

    # Timestamps
    triggered_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When stop was triggered'
    )
    submitted_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When order was submitted to exchange'
    )
    executed_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When order was filled'
    )
    failed_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When execution failed'
    )

    # Execution results
    exchange_order_id = models.CharField(
        max_length=100,
        null=True,
        blank=True,
        db_index=True,
        help_text='Binance order ID'
    )
    fill_price = models.DecimalField(
        max_digits=20,
        decimal_places=8,
        null=True,
        blank=True,
        help_text='Actual fill price'
    )
    slippage_pct = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        null=True,
        blank=True,
        help_text='Calculated slippage'
    )

    # Source and error tracking
    source = models.CharField(
        max_length=20,
        choices=ExecutionSource.choices,
        db_index=True,
        help_text='Which component executed this'
    )
    error_message = models.TextField(
        null=True,
        blank=True,
        help_text='Error details if failed'
    )
    retry_count = models.IntegerField(
        default=0,
        help_text='Number of retry attempts'
    )

    # Audit
    created_at = models.DateTimeField(
        auto_now_add=True,
        help_text='When execution record was created'
    )
    updated_at = models.DateTimeField(
        auto_now=True,
        help_text='When execution record was last updated'
    )

    class Meta:
        db_table = 'stop_executions'
        verbose_name = 'Stop Execution'
        verbose_name_plural = 'Stop Executions'
        ordering = ['-created_at']
        indexes = [
            models.Index(fields=['operation', 'status'], name='idx_stop_exec_op_status'),
            models.Index(fields=['client', 'status'], name='idx_stop_exec_tenant'),
            models.Index(fields=['status', 'created_at'], name='idx_stop_exec_status'),
        ]

    def __str__(self):
        return f"Execution#{self.execution_token[:8]}: {self.status} - Op#{self.operation_id}"


# =====================================================================
# TENANT CONFIG (Risk Guardrails)
# =====================================================================

class TenantConfig(models.Model):
    """
    Per-tenant risk management configuration.

    Controls kill switches, slippage limits, and rate limits.

    Attributes:
        client: Associated client/tenant (one-to-one)
        trading_enabled: Master kill switch
        trading_paused_reason: Why trading was paused
        trading_paused_at: When trading was paused
        max_slippage_pct: Maximum allowed slippage
        slippage_pause_threshold_pct: Slippage that triggers circuit breaker
        max_executions_per_minute: Rate limit
        max_executions_per_hour: Rate limit
    """

    # Primary key (one-to-one with client)
    client = models.OneToOneField(
        'Client',
        primary_key=True,
        on_delete=models.CASCADE,
        related_name='risk_config',
        help_text='Associated client/tenant'
    )

    # Kill switch
    trading_enabled = models.BooleanField(
        default=True,
        db_index=True,
        help_text='Master switch: is trading enabled for this tenant?'
    )
    trading_paused_reason = models.TextField(
        null=True,
        blank=True,
        help_text='Reason why trading was paused'
    )
    trading_paused_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When trading was paused'
    )

    # Slippage limits
    max_slippage_pct = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        default=Decimal('5.0'),
        help_text='Maximum allowed slippage percentage (default: 5%)'
    )
    slippage_pause_threshold_pct = models.DecimalField(
        max_digits=10,
        decimal_places=4,
        default=Decimal('10.0'),
        help_text='Slippage that triggers circuit breaker (default: 10%)'
    )

    # Rate limits
    max_executions_per_minute = models.IntegerField(
        default=10,
        help_text='Maximum stop executions per minute'
    )
    max_executions_per_hour = models.IntegerField(
        default=100,
        help_text='Maximum stop executions per hour'
    )

    # Audit
    created_at = models.DateTimeField(
        auto_now_add=True,
        help_text='When config was created'
    )
    updated_at = models.DateTimeField(
        auto_now=True,
        help_text='When config was last updated'
    )

    class Meta:
        db_table = 'tenant_config'
        verbose_name = 'Tenant Risk Configuration'
        verbose_name_plural = 'Tenant Risk Configurations'

    def __str__(self):
        status = "ENABLED" if self.trading_enabled else "PAUSED"
        return f"Config for {self.client.name}: {status}"


# =====================================================================
# CIRCUIT BREAKER STATE (Per-Symbol)
# =====================================================================

class CircuitBreakerStateModel(models.Model):
    """
    Circuit breaker state per trading pair.

    Implements the Circuit Breaker pattern to prevent cascading failures.

    State Machine:
        CLOSED -> OPEN (after 3 consecutive failures)
        OPEN -> HALF_OPEN (after 5-minute cooldown)
        HALF_OPEN -> CLOSED (if next execution succeeds)
        HALF_OPEN -> OPEN (if next execution fails)

    Attributes:
        symbol: Trading pair (primary key)
        state: Current circuit breaker state (CLOSED, OPEN, HALF_OPEN)
        failure_count: Consecutive failure count
        last_failure_at: When last failure occurred
        opened_at: When circuit was opened
        will_retry_at: When circuit will try to close again
        failure_threshold: Failures needed to trip circuit
        retry_delay_seconds: Cooldown period before retry
    """

    # Primary key
    symbol = models.CharField(
        max_length=20,
        primary_key=True,
        help_text='Trading pair (e.g., BTCUSDC)'
    )

    # State
    state = models.CharField(
        max_length=20,
        default=CircuitBreakerState.CLOSED,
        choices=CircuitBreakerState.choices,
        db_index=True,
        help_text='Current circuit breaker state'
    )

    # Metrics
    failure_count = models.IntegerField(
        default=0,
        help_text='Consecutive failure count'
    )
    last_failure_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When last failure occurred'
    )
    opened_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When circuit was opened (blocked)'
    )
    will_retry_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When circuit will try to close again'
    )

    # Thresholds
    failure_threshold = models.IntegerField(
        default=3,
        help_text='Number of failures to trip circuit (default: 3)'
    )
    retry_delay_seconds = models.IntegerField(
        default=300,
        help_text='Seconds to wait before retry (default: 300 = 5 minutes)'
    )

    # Audit
    updated_at = models.DateTimeField(
        auto_now=True,
        help_text='When state was last updated'
    )

    class Meta:
        db_table = 'circuit_breaker_state'
        verbose_name = 'Circuit Breaker State'
        verbose_name_plural = 'Circuit Breaker States'

    def __str__(self):
        return f"Circuit[{self.symbol}]: {self.state} (failures: {self.failure_count})"


# =====================================================================
# OUTBOX (Transactional Outbox Pattern)
# =====================================================================

class Outbox(models.Model):
    """
    Outbox for reliable event publishing to RabbitMQ.

    Implements the Transactional Outbox pattern:
    1. Event inserted into stop_events (within transaction)
    2. Outbox entry inserted (same transaction)
    3. Transaction commits (atomic)
    4. Background worker publishes to RabbitMQ
    5. Marks as published after successful publish

    This guarantees at-least-once delivery of events.

    Attributes:
        outbox_id: Unique outbox entry identifier
        event: Associated stop event
        routing_key: RabbitMQ routing key
        exchange: RabbitMQ exchange name
        payload: Event payload to publish
        published: Has this been published?
        published_at: When successfully published
        retry_count: Number of publish attempts
        last_error: Last publish error message
    """

    # Primary key
    outbox_id = models.UUIDField(
        primary_key=True,
        default=uuid.uuid4,
        editable=False,
        help_text='Unique outbox entry identifier'
    )

    # Event reference
    event = models.ForeignKey(
        StopEvent,
        on_delete=models.CASCADE,
        related_name='outbox_entries',
        help_text='Associated stop event'
    )

    # Routing
    routing_key = models.CharField(
        max_length=255,
        help_text='RabbitMQ routing key (e.g., stop.executed.tenant1.BTCUSDC)'
    )
    exchange = models.CharField(
        max_length=100,
        default='stop_events',
        help_text='RabbitMQ exchange name'
    )

    # Payload
    payload = models.JSONField(
        help_text='Event payload to publish'
    )

    # Publishing state
    published = models.BooleanField(
        default=False,
        db_index=True,
        help_text='Has this been published to RabbitMQ?'
    )
    published_at = models.DateTimeField(
        null=True,
        blank=True,
        help_text='When successfully published'
    )
    retry_count = models.IntegerField(
        default=0,
        help_text='Number of publish attempts'
    )
    last_error = models.TextField(
        null=True,
        blank=True,
        help_text='Last publish error message'
    )

    # Audit
    created_at = models.DateTimeField(
        auto_now_add=True,
        db_index=True,
        help_text='When outbox entry was created'
    )

    class Meta:
        db_table = 'outbox'
        verbose_name = 'Outbox Entry'
        verbose_name_plural = 'Outbox Entries'
        ordering = ['created_at']
        indexes = [
            models.Index(
                fields=['published', 'created_at'],
                name='idx_outbox_unpublished',
                condition=models.Q(published=False),
            ),
        ]

    def __str__(self):
        status = "✅ Published" if self.published else f"⏳ Pending (retry: {self.retry_count})"
        return f"Outbox[{self.routing_key}]: {status}"
