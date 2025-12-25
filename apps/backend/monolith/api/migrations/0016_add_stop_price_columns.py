# Migration 0016a: Add columns WITHOUT defaults (production-safe)
# ADR-0012: Zero-downtime migration (metadata-only, no table rewrite)

from django.db import migrations, models


class Migration(migrations.Migration):
    """
    Add stop_price columns to Operation table (NO defaults to avoid table rewrite).

    Lock Analysis:
    - ACCESS EXCLUSIVE lock duration: <1 second (metadata-only)
    - No table rewrite (NULL columns)
    - Safe for production deployment
    """

    dependencies = [
        ('api', '0015_event_sourcing_stop_monitor'),
    ]

    operations = [
        # Add stop_price (absolute technical stop level)
        migrations.AddField(
            model_name='operation',
            name='stop_price',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                null=True,  # ⭐ No DEFAULT (avoid table rewrite)
                blank=True,
                db_index=True,
                help_text='Absolute technical stop price (FIXED level, never recalculated)'
            ),
        ),

        # Add target_price (absolute take-profit level)
        migrations.AddField(
            model_name='operation',
            name='target_price',
            field=models.DecimalField(
                max_digits=20,
                decimal_places=8,
                null=True,  # ⭐ No DEFAULT
                blank=True,
                db_index=True,
                help_text='Absolute target/take-profit price (FIXED level)'
            ),
        ),

        # Add execution tracking fields
        migrations.AddField(
            model_name='operation',
            name='stop_execution_token',
            field=models.CharField(
                max_length=64,
                null=True,
                blank=True,
                db_index=True,
                help_text='Idempotency token of current/last stop execution'
            ),
        ),

        migrations.AddField(
            model_name='operation',
            name='last_stop_check_at',
            field=models.DateTimeField(
                null=True,
                blank=True,
                help_text='Last time stop monitor checked this operation'
            ),
        ),

        migrations.AddField(
            model_name='operation',
            name='stop_check_count',
            field=models.IntegerField(
                null=True,  # ⭐ No DEFAULT (set in next migration)
                blank=True,
                help_text='Number of times stop monitor has checked this operation'
            ),
        ),

        # Mark percentage fields as deprecated
        migrations.AlterField(
            model_name='operation',
            name='stop_loss_percent',
            field=models.DecimalField(
                max_digits=10,
                decimal_places=2,
                null=True,
                blank=True,
                help_text='[DEPRECATED] Use stop_price instead. Kept for reference only.'
            ),
        ),

        migrations.AlterField(
            model_name='operation',
            name='stop_gain_percent',
            field=models.DecimalField(
                max_digits=10,
                decimal_places=2,
                null=True,
                blank=True,
                help_text='[DEPRECATED] Use target_price instead. Kept for reference only.'
            ),
        ),
    ]
