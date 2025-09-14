from django.db import migrations, models
import django.db.models.deletion
from decimal import Decimal


class Migration(migrations.Migration):

    dependencies = [
        ("api", "0004_patterns_models"),
    ]

    operations = [
        migrations.CreateModel(
            name="MovingAverage",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("timeframe", models.CharField(default="1h", max_length=8)),
                ("period", models.IntegerField()),
                ("value", models.DecimalField(decimal_places=8, default=Decimal("0"), max_digits=20)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("symbol", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="indicators", to="api.symbol")),
            ],
        ),
        migrations.CreateModel(
            name="RSIIndicator",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("timeframe", models.CharField(default="1h", max_length=8)),
                ("period", models.IntegerField()),
                ("value", models.DecimalField(decimal_places=8, default=Decimal("0"), max_digits=20)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("symbol", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="rsi_indicators", to="api.symbol")),
            ],
        ),
        migrations.CreateModel(
            name="MACDIndicator",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("timeframe", models.CharField(default="1h", max_length=8)),
                ("fast_period", models.IntegerField()),
                ("slow_period", models.IntegerField()),
                ("signal_period", models.IntegerField()),
                ("macd", models.DecimalField(decimal_places=8, default=Decimal("0"), max_digits=20)),
                ("signal", models.DecimalField(decimal_places=8, default=Decimal("0"), max_digits=20)),
                ("histogram", models.DecimalField(decimal_places=8, default=Decimal("0"), max_digits=20)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("symbol", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="macd_indicators", to="api.symbol")),
            ],
        ),
    ]

