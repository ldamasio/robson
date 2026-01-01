# Generated manually for Phase 5 MVP

from django.db import migrations, models
import django.db.models.deletion
import django.utils.timezone


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0026_add_agentic_workflow_fields'),
    ]

    operations = [
        # Add pattern metadata fields to TradingIntent
        migrations.AddField(
            model_name='tradingintent',
            name='pattern_code',
            field=models.CharField(blank=True, db_index=True, help_text='Pattern code that triggered this intent (e.g., HAMMER, MA_CROSSOVER)', max_length=50, null=True),
        ),
        migrations.AddField(
            model_name='tradingintent',
            name='pattern_event_id',
            field=models.CharField(blank=True, db_index=True, help_text='Unique event ID from pattern engine for idempotency', max_length=255, null=True),
        ),
        migrations.AddField(
            model_name='tradingintent',
            name='pattern_source',
            field=models.CharField(blank=True, default='manual', help_text="Source: 'pattern' or 'manual'", max_length=50, null=True),
        ),
        migrations.AddField(
            model_name='tradingintent',
            name='pattern_triggered_at',
            field=models.DateTimeField(blank=True, help_text='When the pattern triggered this intent', null=True),
        ),
        # Create PatternTrigger model
        migrations.CreateModel(
            name='PatternTrigger',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('created_at', models.DateTimeField(auto_now_add=True, default=django.utils.timezone.now)),
                ('updated_at', models.DateTimeField(auto_now=True)),
                ('client', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name='pattern_triggers', to='api.client')),
                ('pattern_event_id', models.CharField(db_index=True, help_text='Unique event ID from pattern engine', max_length=255, unique=True)),
                ('pattern_code', models.CharField(db_index=True, help_text='Pattern code (e.g., HAMMER, MA_CROSSOVER)', max_length=50)),
                ('intent', models.ForeignKey(blank=True, null=True, on_delete=django.db.models.deletion.CASCADE, related_name='pattern_triggers', to='api.tradingintent')),
                ('status', models.CharField(choices=[('processed', 'Processed'), ('failed', 'Failed')], default='processed', max_length=20)),
                ('error_message', models.TextField(blank=True, null=True)),
                ('processed_at', models.DateTimeField(auto_now_add=True)),
            ],
            options={
                'verbose_name': 'Pattern Trigger',
                'verbose_name_plural': 'Pattern Triggers',
                'indexes': [
                    models.Index(fields=['client', 'pattern_event_id'], name='api_patte_client__idx'),
                    models.Index(fields=['pattern_code', 'processed_at'], name='api_patte_patter__idx'),
                ],
            },
        ),
    ]
