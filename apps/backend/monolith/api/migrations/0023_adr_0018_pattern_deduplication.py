# Generated for ADR-0018: Pattern Detection Deduplication
# Adds UniqueConstraint and missing index for PatternInstance

from django.db import migrations, models


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0022_add_entry_gate_models'),
    ]

    operations = [
        # Add missing index for client-based queries
        migrations.AddIndex(
            model_name='patterninstance',
            index=models.Index(
                fields=['client', 'symbol', 'timeframe', 'status'],
                name='api_pattern_client_idx',
            ),
        ),
        # Add UniqueConstraint for idempotent deduplication
        # This ensures concurrent scans cannot create duplicate pattern instances
        migrations.AddConstraint(
            model_name='patterninstance',
            constraint=models.UniqueConstraint(
                fields=['client', 'pattern', 'symbol', 'timeframe', 'start_ts'],
                name='unique_pattern_instance',
                violation_error_message='A pattern instance with these attributes already exists.',
            ),
        ),
    ]
