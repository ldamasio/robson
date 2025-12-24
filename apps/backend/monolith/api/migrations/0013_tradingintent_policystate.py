# Generated manually for TradingIntent and PolicyState models
# Created: 2025-12-24

import django.db.models.deletion
from decimal import Decimal
from django.db import migrations, models


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0012_audit_trail_models'),
        ('clients', '0003_alter_customuser_client'),
    ]

    operations = [
        migrations.CreateModel(
            name='TradingIntent',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('created_at', models.DateTimeField(auto_now_add=True)),
                ('updated_at', models.DateTimeField(auto_now=True)),
                ('intent_id', models.CharField(db_index=True, max_length=255, unique=True)),
                ('side', models.CharField(choices=[('BUY', 'Buy'), ('SELL', 'Sell')], max_length=10)),
                ('status', models.CharField(
                    choices=[
                        ('PENDING', 'Pending'),
                        ('VALIDATED', 'Validated'),
                        ('EXECUTING', 'Executing'),
                        ('EXECUTED', 'Executed'),
                        ('FAILED', 'Failed'),
                        ('CANCELLED', 'Cancelled')
                    ],
                    db_index=True,
                    default='PENDING',
                    max_length=20
                )),
                ('quantity', models.DecimalField(decimal_places=8, max_digits=20)),
                ('entry_price', models.DecimalField(decimal_places=8, max_digits=20)),
                ('stop_price', models.DecimalField(decimal_places=8, max_digits=20)),
                ('target_price', models.DecimalField(blank=True, decimal_places=8, max_digits=20, null=True)),
                ('regime', models.CharField(help_text='Market regime: bull, bear, sideways', max_length=50)),
                ('confidence', models.FloatField(help_text='Confidence level 0.0 to 1.0')),
                ('reason', models.TextField(help_text='Human-readable explanation of decision')),
                ('validated_at', models.DateTimeField(blank=True, null=True)),
                ('executed_at', models.DateTimeField(blank=True, null=True)),
                ('exchange_order_id', models.CharField(blank=True, max_length=100, null=True)),
                ('actual_fill_price', models.DecimalField(blank=True, decimal_places=8, max_digits=20, null=True)),
                ('actual_fill_quantity', models.DecimalField(blank=True, decimal_places=8, max_digits=20, null=True)),
                ('risk_amount', models.DecimalField(decimal_places=8, default=Decimal('0'), max_digits=20)),
                ('risk_percent', models.DecimalField(decimal_places=2, default=Decimal('0'), max_digits=10)),
                ('correlation_id', models.CharField(blank=True, db_index=True, max_length=255, null=True)),
                ('error_message', models.TextField(blank=True, null=True)),
                ('client', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, to='clients.client')),
                ('order', models.ForeignKey(
                    blank=True,
                    null=True,
                    on_delete=django.db.models.deletion.SET_NULL,
                    related_name='intents',
                    to='api.order'
                )),
                ('strategy', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, to='api.strategy')),
                ('symbol', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, to='api.symbol')),
            ],
            options={
                'verbose_name': 'Trading Intent',
                'verbose_name_plural': 'Trading Intents',
                'ordering': ['-created_at'],
            },
        ),
        migrations.CreateModel(
            name='PolicyState',
            fields=[
                ('id', models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name='ID')),
                ('created_at', models.DateTimeField(auto_now_add=True)),
                ('updated_at', models.DateTimeField(auto_now=True)),
                ('name', models.CharField(blank=True, default='', max_length=255)),
                ('description', models.TextField(blank=True, default='')),
                ('config', models.JSONField(blank=True, default=dict)),
                ('is_active', models.BooleanField(default=True)),
                ('month', models.CharField(
                    db_index=True,
                    help_text='Month in YYYY-MM format (e.g., 2025-12)',
                    max_length=7
                )),
                ('status', models.CharField(
                    choices=[
                        ('ACTIVE', 'Active'),
                        ('PAUSED', 'Paused'),
                        ('SUSPENDED', 'Suspended')
                    ],
                    db_index=True,
                    default='ACTIVE',
                    max_length=20
                )),
                ('starting_capital', models.DecimalField(
                    decimal_places=8,
                    help_text='Capital at the start of the month',
                    max_digits=20
                )),
                ('current_capital', models.DecimalField(
                    decimal_places=8,
                    help_text='Current capital (includes unrealized P&L)',
                    max_digits=20
                )),
                ('realized_pnl', models.DecimalField(
                    decimal_places=8,
                    default=Decimal('0'),
                    help_text='Realized profit/loss for the month',
                    max_digits=20
                )),
                ('unrealized_pnl', models.DecimalField(
                    decimal_places=8,
                    default=Decimal('0'),
                    help_text='Unrealized profit/loss from open positions',
                    max_digits=20
                )),
                ('total_trades', models.IntegerField(default=0, help_text='Total trades executed this month')),
                ('winning_trades', models.IntegerField(default=0, help_text='Number of winning trades')),
                ('losing_trades', models.IntegerField(default=0, help_text='Number of losing trades')),
                ('max_drawdown_percent', models.DecimalField(
                    decimal_places=2,
                    default=Decimal('4.0'),
                    help_text='Maximum monthly drawdown percentage',
                    max_digits=10
                )),
                ('max_trades_per_day', models.IntegerField(
                    default=50,
                    help_text='Maximum trades per day (medium-frequency limit)'
                )),
                ('paused_at', models.DateTimeField(blank=True, help_text='When the policy was paused', null=True)),
                ('pause_reason', models.TextField(blank=True, help_text='Reason for pausing trading', null=True)),
                ('client', models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, to='clients.client')),
            ],
            options={
                'verbose_name': 'Policy State',
                'verbose_name_plural': 'Policy States',
                'ordering': ['-month', 'client'],
            },
        ),
        migrations.AddIndex(
            model_name='tradingintent',
            index=models.Index(fields=['client', 'status', 'created_at'], name='api_trading_client_idx'),
        ),
        migrations.AddIndex(
            model_name='tradingintent',
            index=models.Index(fields=['symbol', 'created_at'], name='api_trading_symbol_idx'),
        ),
        migrations.AddIndex(
            model_name='tradingintent',
            index=models.Index(fields=['strategy', 'created_at'], name='api_trading_strat_idx'),
        ),
        migrations.AddIndex(
            model_name='policystate',
            index=models.Index(fields=['client', 'month'], name='api_policy_client_idx'),
        ),
        migrations.AddIndex(
            model_name='policystate',
            index=models.Index(fields=['status', 'month'], name='api_policy_status_idx'),
        ),
        migrations.AlterUniqueTogether(
            name='policystate',
            unique_together={('client', 'month')},
        ),
    ]
