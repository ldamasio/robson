# Migration 0020: Generalize Outbox for Commands and Events
# ADR-0015: RabbitMQ-based Stop Engine Architecture

from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):
    """
    Generalize Outbox model to support both commands and events.

    Changes:
    1. Add aggregate_type field (stop_command or stop_event)
    2. Add aggregate_id field (operation_id for correlation)
    3. Add event_type field (semantic type)
    4. Add correlation_id field (global idempotency key, unique)
    5. Make event FK nullable (only events reference StopEvent)
    6. Remove default from exchange field (commands use stop_commands, events use stop_events)
    7. Add indexes for aggregate_type, aggregate_id, and correlation_id

    Lock Analysis:
    - ACCESS EXCLUSIVE lock duration: <2 seconds (metadata-only)
    - No table rewrite (NULL columns, no defaults on new fields)
    - Safe for production deployment (outbox table is small)
    """

    dependencies = [
        ('api', '0019_add_btc_portfolio_tracking'),
    ]

    operations = [
        # Add aggregate_type field
        migrations.AddField(
            model_name='outbox',
            name='aggregate_type',
            field=models.CharField(
                max_length=50,
                db_index=True,
                null=True,  # Temporarily nullable for migration
                blank=True,
                help_text="Message type: 'stop_command' or 'stop_event'"
            ),
        ),

        # Add aggregate_id field
        migrations.AddField(
            model_name='outbox',
            name='aggregate_id',
            field=models.BigIntegerField(
                db_index=True,
                null=True,  # Temporarily nullable for migration
                blank=True,
                help_text='Operation ID (for correlation)'
            ),
        ),

        # Add event_type field
        migrations.AddField(
            model_name='outbox',
            name='event_type',
            field=models.CharField(
                max_length=50,
                db_index=True,
                null=True,  # Temporarily nullable for migration
                blank=True,
                help_text="Semantic type: 'COMMAND_ISSUED', 'EVENT_TRIGGERED', 'EXECUTED', etc."
            ),
        ),

        # Add correlation_id field (unique idempotency key)
        migrations.AddField(
            model_name='outbox',
            name='correlation_id',
            field=models.CharField(
                max_length=64,
                unique=True,
                db_index=True,
                null=True,  # Temporarily nullable for migration
                blank=True,
                help_text='Global idempotency key (format: {operation_id}:{stop_price}:{timestamp_ms})'
            ),
        ),

        # Make event FK nullable (only events reference StopEvent, commands do not)
        migrations.AlterField(
            model_name='outbox',
            name='event',
            field=models.ForeignKey(
                to='api.StopEvent',
                on_delete=django.db.models.deletion.CASCADE,
                null=True,
                blank=True,
                related_name='outbox_entries',
                help_text='Associated stop event (null for commands)'
            ),
        ),

        # Update exchange field to remove default (commands use stop_commands, events use stop_events)
        migrations.AlterField(
            model_name='outbox',
            name='exchange',
            field=models.CharField(
                max_length=100,
                help_text='RabbitMQ exchange name (stop_commands or stop_events)'
            ),
        ),

        # Add composite index for aggregate type and ID
        migrations.AddIndex(
            model_name='outbox',
            index=models.Index(
                fields=['aggregate_type', 'aggregate_id'],
                name='idx_outbox_aggregate',
            ),
        ),

        # Add index for correlation_id (in addition to unique constraint)
        migrations.AddIndex(
            model_name='outbox',
            index=models.Index(
                fields=['correlation_id'],
                name='idx_outbox_correlation',
            ),
        ),
    ]
