# Generated manually for entry_gate models

import uuid
from decimal import Decimal
from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0021_add_market_context_models'),
        ('clients', '__latest__'),
    ]

    operations = [
        migrations.CreateModel(
            name='EntryGateConfig',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('created_at', models.DateTimeField(auto_now_add=True, help_text='Record creation timestamp')),
                ('updated_at', models.DateTimeField(auto_now=True, help_text='Last update timestamp')),
                ('enable_cooldown', models.BooleanField(default=True, help_text='Enable cooldown period after stop-out')),
                ('cooldown_after_stop_seconds', models.IntegerField(default=900, help_text='Cooldown period in seconds after a stop-out event')),
                ('enable_funding_rate_gate', models.BooleanField(default=True, help_text='Enable extreme funding rate check')),
                ('funding_rate_threshold', models.DecimalField(decimal_places=6, default=Decimal('0.0001'), help_text='Funding rate threshold (absolute value)', max_digits=10)),
                ('enable_stale_data_gate', models.BooleanField(default=True, help_text='Enable stale market data check')),
                ('max_data_age_seconds', models.IntegerField(default=300, help_text='Maximum acceptable age of market data in seconds')),
                ('client', models.ForeignKey(null=True, on_delete=django.db.models.deletion.SET_NULL, to='clients.client')),
            ],
            options={
                'verbose_name': 'Entry Gate Configuration',
                'verbose_name_plural': 'Entry Gate Configurations',
                'db_table': 'entry_gate_config',
                'unique_together': {('client',)},
            },
        ),
        migrations.CreateModel(
            name='EntryGateDecisionModel',
            fields=[
                ('created_at', models.DateTimeField(auto_now_add=True, help_text='Record creation timestamp')),
                ('updated_at', models.DateTimeField(auto_now=True, help_text='Last update timestamp')),
                ('decision_id', models.UUIDField(default=uuid.uuid4, editable=False, help_text='Unique decision identifier', primary_key=True, serialize=False)),
                ('symbol', models.CharField(help_text='Trading pair (e.g., BTCUSDT)', max_length=20)),
                ('allowed', models.BooleanField(help_text='True if entry was allowed, False if denied')),
                ('reasons', models.JSONField(help_text='List of human-readable reasons (failures + successes)')),
                ('gate_checks', models.JSONField(help_text='Detailed results for each gate check')),
                ('context', models.JSONField(default=dict, help_text='Additional context for debugging (side, price, etc.)')),
                ('client', models.ForeignKey(null=True, on_delete=django.db.models.deletion.SET_NULL, to='clients.client')),
            ],
            options={
                'verbose_name': 'Entry Gate Decision',
                'verbose_name_plural': 'Entry Gate Decisions',
                'db_table': 'entry_gate_decisions',
                'ordering': ['-created_at'],
            },
        ),
        migrations.AddIndex(
            model_name='entrygatedecisionmodel',
            index=models.Index(fields=['client', '-created_at'], name='idx_decisions_client_time'),
        ),
        migrations.AddIndex(
            model_name='entrygatedecisionmodel',
            index=models.Index(fields=['symbol', '-created_at'], name='idx_decisions_symbol_time'),
        ),
        migrations.AddIndex(
            model_name='entrygatedecisionmodel',
            index=models.Index(fields=['allowed', '-created_at'], name='idx_decisions_allowed_time'),
        ),
    ]
