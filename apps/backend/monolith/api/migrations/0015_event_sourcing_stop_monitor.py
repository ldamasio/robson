# Generated migration for Event-Sourced Stop-Loss Monitor
# ADR-0012: Event Sourcing architecture for stop-loss execution

from django.db import migrations, models
import django.db.models.deletion
import uuid


class Migration(migrations.Migration):
    dependencies = [
        ('api', '0014_alter_audittransaction_options_and_more'),
        ('clients', '0001_initial'),  # Required for Client FK
    ]

    operations = [
        # =====================================================================
        # TABLE: stop_events (Append-Only Event Store)
        # =====================================================================
        migrations.CreateModel(
            name='StopEvent',
            fields=[
                # Event identity
                ('event_id', models.UUIDField(
                    primary_key=True,
                    default=uuid.uuid4,
                    editable=False,
                    help_text='Unique event identifier'
                )),
                ('event_seq', models.BigIntegerField(
                    unique=True,
                    editable=False,
                    null=True,
                    blank=True,
                    help_text='Global sequence number for event ordering (auto-populated via DB sequence)'
                )),
                ('occurred_at', models.DateTimeField(
                    auto_now_add=True,
                    db_index=True,
                    help_text='When the event occurred'
                )),

                # Operation context
                ('operation', models.ForeignKey(
                    on_delete=models.CASCADE,
                    to='api.operation',
                    related_name='stop_events',
                    help_text='Related trading operation'
                )),
                ('client', models.ForeignKey(
                    on_delete=models.CASCADE,
                    to='clients.client',
                    related_name='stop_events',
                    help_text='Tenant/client for multi-tenant isolation'
                )),
                ('symbol', models.CharField(
                    max_length=20,
                    db_index=True,
                    help_text='Trading pair (e.g., BTCUSDC)'
                )),

                # Event type (state transition)
                ('event_type', models.CharField(
                    max_length=50,
                    db_index=True,
                    choices=[
                        ('STOP_TRIGGERED', 'Stop Triggered'),
                        ('EXECUTION_SUBMITTED', 'Execution Submitted'),
                        ('EXECUTED', 'Executed'),
                        ('FAILED', 'Failed'),
                        ('BLOCKED', 'Blocked'),
                        ('STALE_PRICE', 'Stale Price'),
                        ('KILL_SWITCH', 'Kill Switch Activated'),
                        ('SLIPPAGE_BREACH', 'Slippage Limit Breached'),
                        ('CIRCUIT_BREAKER', 'Circuit Breaker Tripped'),
                    ],
                    help_text='Type of event'
                )),

                # Stop-loss parameters (captured at event time)
                ('trigger_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Price that triggered the stop'
                )),
                ('stop_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Configured stop level (absolute price)'
                )),
                ('quantity', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Quantity to close'
                )),
                ('side', models.CharField(
                    max_length=10,
                    null=True,
                    blank=True,
                    choices=[('BUY', 'Buy'), ('SELL', 'Sell')],
                    help_text='Order side (closing direction)'
                )),

                # Idempotency - NOT unique because multiple events share same token
                ('execution_token', models.CharField(
                    max_length=64,
                    null=True,
                    blank=True,
                    db_index=True,
                    help_text='Execution token (shared by all events in same execution)'
                )),

                # Payload (full context for debugging)
                ('payload_json', models.JSONField(
                    default=dict,
                    blank=True,
                    help_text='Complete event context (entry_price, slippage_limit, etc.)'
                )),
                ('request_payload_hash', models.CharField(
                    max_length=64,
                    null=True,
                    blank=True,
                    help_text='SHA-256 hash of request payload for deduplication'
                )),

                # Execution results
                ('exchange_order_id', models.CharField(
                    max_length=100,
                    null=True,
                    blank=True,
                    db_index=True,
                    help_text='Binance order ID (if executed)'
                )),
                ('fill_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Actual fill price from exchange'
                )),
                ('slippage_pct', models.DecimalField(
                    max_digits=10,
                    decimal_places=4,
                    null=True,
                    blank=True,
                    help_text='Calculated slippage percentage'
                )),

                # Source attribution
                ('source', models.CharField(
                    max_length=20,
                    db_index=True,
                    choices=[
                        ('ws', 'WebSocket Service'),
                        ('cron', 'CronJob Backstop'),
                        ('manual', 'Manual Intervention'),
                    ],
                    help_text='Which component emitted this event'
                )),

                # Error tracking
                ('error_message', models.TextField(
                    null=True,
                    blank=True,
                    help_text='Error details if execution failed'
                )),
                ('retry_count', models.IntegerField(
                    default=0,
                    help_text='Number of retry attempts'
                )),
            ],
            options={
                'db_table': 'stop_events',
                'ordering': ['event_seq'],
                'verbose_name': 'Stop Event',
                'verbose_name_plural': 'Stop Events',
                'indexes': [
                    models.Index(fields=['operation', 'event_seq'], name='idx_stop_events_op_seq'),
                    models.Index(fields=['client', 'occurred_at'], name='idx_stop_events_tenant'),
                    models.Index(fields=['event_type', 'occurred_at'], name='idx_stop_events_type'),
                    models.Index(fields=['source', 'occurred_at'], name='idx_stop_events_source'),
                    models.Index(fields=['symbol', 'occurred_at'], name='idx_stop_events_symbol'),
                ],
            },
        ),

        # =====================================================================
        # TABLE: stop_executions (Materialized View / Projection)
        # =====================================================================
        migrations.CreateModel(
            name='StopExecution',
            fields=[
                # Primary key
                ('execution_id', models.UUIDField(
                    primary_key=True,
                    default=uuid.uuid4,
                    editable=False,
                    help_text='Unique execution identifier'
                )),

                # Operation reference
                ('operation', models.ForeignKey(
                    on_delete=models.CASCADE,
                    to='api.operation',
                    related_name='stop_executions',
                    help_text='Related trading operation'
                )),
                ('client', models.ForeignKey(
                    on_delete=models.CASCADE,
                    to='clients.client',
                    related_name='stop_executions',
                    help_text='Tenant/client for multi-tenant isolation'
                )),

                # Idempotency token (unique across ALL executions)
                ('execution_token', models.CharField(
                    max_length=64,
                    unique=True,
                    db_index=True,
                    help_text='Global idempotency token'
                )),

                # Execution state (derived from latest event)
                ('status', models.CharField(
                    max_length=50,
                    default='PENDING',
                    db_index=True,
                    choices=[
                        ('PENDING', 'Pending'),
                        ('SUBMITTED', 'Submitted'),
                        ('EXECUTED', 'Executed'),
                        ('FAILED', 'Failed'),
                        ('BLOCKED', 'Blocked'),
                    ],
                    help_text='Current execution status'
                )),

                # Stop parameters (absolute price)
                ('stop_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    help_text='Fixed technical stop level'
                )),
                ('trigger_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Price at detection'
                )),
                ('quantity', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    help_text='Quantity to close'
                )),
                ('side', models.CharField(
                    max_length=10,
                    choices=[('BUY', 'Buy'), ('SELL', 'Sell')],
                    help_text='Order side (closing direction)'
                )),

                # Timestamps (from events)
                ('triggered_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When stop was triggered'
                )),
                ('submitted_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When order was submitted to exchange'
                )),
                ('executed_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When order was filled'
                )),
                ('failed_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When execution failed'
                )),

                # Execution results
                ('exchange_order_id', models.CharField(
                    max_length=100,
                    null=True,
                    blank=True,
                    db_index=True,
                    help_text='Binance order ID'
                )),
                ('fill_price', models.DecimalField(
                    max_digits=20,
                    decimal_places=8,
                    null=True,
                    blank=True,
                    help_text='Actual fill price'
                )),
                ('slippage_pct', models.DecimalField(
                    max_digits=10,
                    decimal_places=4,
                    null=True,
                    blank=True,
                    help_text='Calculated slippage'
                )),

                # Source and error tracking
                ('source', models.CharField(
                    max_length=20,
                    db_index=True,
                    choices=[
                        ('ws', 'WebSocket Service'),
                        ('cron', 'CronJob Backstop'),
                    ],
                    help_text='Which component executed this'
                )),
                ('error_message', models.TextField(
                    null=True,
                    blank=True,
                    help_text='Error details if failed'
                )),
                ('retry_count', models.IntegerField(
                    default=0,
                    help_text='Number of retry attempts'
                )),

                # Audit
                ('created_at', models.DateTimeField(
                    auto_now_add=True,
                    help_text='When execution record was created'
                )),
                ('updated_at', models.DateTimeField(
                    auto_now=True,
                    help_text='When execution record was last updated'
                )),
            ],
            options={
                'db_table': 'stop_executions',
                'verbose_name': 'Stop Execution',
                'verbose_name_plural': 'Stop Executions',
                'ordering': ['-created_at'],
                'indexes': [
                    models.Index(fields=['operation', 'status'], name='idx_stop_exec_op_status'),
                    models.Index(fields=['client', 'status'], name='idx_stop_exec_tenant'),
                    models.Index(fields=['status', 'created_at'], name='idx_stop_exec_status'),
                ],
            },
        ),

        # =====================================================================
        # TABLE: tenant_config (Risk Guardrails Configuration)
        # =====================================================================
        migrations.CreateModel(
            name='TenantConfig',
            fields=[
                # Primary key (one-to-one with client)
                ('client', models.OneToOneField(
                    primary_key=True,
                    on_delete=models.CASCADE,
                    to='clients.client',
                    related_name='risk_config',
                    help_text='Associated client/tenant'
                )),

                # Kill switch
                ('trading_enabled', models.BooleanField(
                    default=True,
                    db_index=True,
                    help_text='Master switch: is trading enabled for this tenant?'
                )),
                ('trading_paused_reason', models.TextField(
                    null=True,
                    blank=True,
                    help_text='Reason why trading was paused'
                )),
                ('trading_paused_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When trading was paused'
                )),

                # Slippage limits
                ('max_slippage_pct', models.DecimalField(
                    max_digits=10,
                    decimal_places=4,
                    default=5.0,
                    help_text='Maximum allowed slippage percentage (default: 5%)'
                )),
                ('slippage_pause_threshold_pct', models.DecimalField(
                    max_digits=10,
                    decimal_places=4,
                    default=10.0,
                    help_text='Slippage that triggers circuit breaker (default: 10%)'
                )),

                # Rate limits
                ('max_executions_per_minute', models.IntegerField(
                    default=10,
                    help_text='Maximum stop executions per minute'
                )),
                ('max_executions_per_hour', models.IntegerField(
                    default=100,
                    help_text='Maximum stop executions per hour'
                )),

                # Audit
                ('created_at', models.DateTimeField(
                    auto_now_add=True,
                    help_text='When config was created'
                )),
                ('updated_at', models.DateTimeField(
                    auto_now=True,
                    help_text='When config was last updated'
                )),
            ],
            options={
                'db_table': 'tenant_config',
                'verbose_name': 'Tenant Risk Configuration',
                'verbose_name_plural': 'Tenant Risk Configurations',
            },
        ),

        # =====================================================================
        # TABLE: circuit_breaker_state (Per-Symbol Circuit Breaker)
        # =====================================================================
        migrations.CreateModel(
            name='CircuitBreakerState',
            fields=[
                # Primary key
                ('symbol', models.CharField(
                    max_length=20,
                    primary_key=True,
                    help_text='Trading pair (e.g., BTCUSDC)'
                )),

                # State
                ('state', models.CharField(
                    max_length=20,
                    default='CLOSED',
                    db_index=True,
                    choices=[
                        ('CLOSED', 'Closed (Normal Trading)'),
                        ('OPEN', 'Open (Trading Blocked)'),
                        ('HALF_OPEN', 'Half-Open (Testing Recovery)'),
                    ],
                    help_text='Current circuit breaker state'
                )),

                # Metrics
                ('failure_count', models.IntegerField(
                    default=0,
                    help_text='Consecutive failure count'
                )),
                ('last_failure_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When last failure occurred'
                )),
                ('opened_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When circuit was opened (blocked)'
                )),
                ('will_retry_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When circuit will try to close again'
                )),

                # Thresholds
                ('failure_threshold', models.IntegerField(
                    default=3,
                    help_text='Number of failures to trip circuit (default: 3)'
                )),
                ('retry_delay_seconds', models.IntegerField(
                    default=300,
                    help_text='Seconds to wait before retry (default: 300 = 5 minutes)'
                )),

                # Audit
                ('updated_at', models.DateTimeField(
                    auto_now=True,
                    help_text='When state was last updated'
                )),
            ],
            options={
                'db_table': 'circuit_breaker_state',
                'verbose_name': 'Circuit Breaker State',
                'verbose_name_plural': 'Circuit Breaker States',
            },
        ),

        # =====================================================================
        # TABLE: outbox (Transactional Outbox Pattern)
        # =====================================================================
        migrations.CreateModel(
            name='Outbox',
            fields=[
                # Primary key
                ('outbox_id', models.UUIDField(
                    primary_key=True,
                    default=uuid.uuid4,
                    editable=False,
                    help_text='Unique outbox entry identifier'
                )),

                # Event reference
                ('event', models.ForeignKey(
                    on_delete=models.CASCADE,
                    to='api.stopevent',
                    related_name='outbox_entries',
                    help_text='Associated stop event'
                )),

                # Routing
                ('routing_key', models.CharField(
                    max_length=255,
                    help_text='RabbitMQ routing key (e.g., stop.executed.tenant1.BTCUSDC)'
                )),
                ('exchange', models.CharField(
                    max_length=100,
                    default='stop_events',
                    help_text='RabbitMQ exchange name'
                )),

                # Payload
                ('payload', models.JSONField(
                    help_text='Event payload to publish'
                )),

                # Publishing state
                ('published', models.BooleanField(
                    default=False,
                    db_index=True,
                    help_text='Has this been published to RabbitMQ?'
                )),
                ('published_at', models.DateTimeField(
                    null=True,
                    blank=True,
                    help_text='When successfully published'
                )),
                ('retry_count', models.IntegerField(
                    default=0,
                    help_text='Number of publish attempts'
                )),
                ('last_error', models.TextField(
                    null=True,
                    blank=True,
                    help_text='Last publish error message'
                )),

                # Audit
                ('created_at', models.DateTimeField(
                    auto_now_add=True,
                    db_index=True,
                    help_text='When outbox entry was created'
                )),
            ],
            options={
                'db_table': 'outbox',
                'verbose_name': 'Outbox Entry',
                'verbose_name_plural': 'Outbox Entries',
                'ordering': ['created_at'],
                'indexes': [
                    models.Index(
                        fields=['published', 'created_at'],
                        name='idx_outbox_unpublished',
                        condition=models.Q(published=False),
                    ),
                ],
            },
        ),
    ]
