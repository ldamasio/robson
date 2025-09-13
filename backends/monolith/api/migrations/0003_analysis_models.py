from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):

    dependencies = [
        ("api", "0002_alter_order_price_nullable"),
    ]

    operations = [
        migrations.CreateModel(
            name="TechnicalAnalysisInterpretation",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("name", models.CharField(max_length=255)),
                ("experience", models.IntegerField(default=1, help_text="Required experience level (1-5)")),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
            ],
        ),
        migrations.CreateModel(
            name="TechnicalEvent",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("timeframe", models.CharField(default="1h", help_text="Timeframe of the event (e.g., 1h, 4h, 1d)", max_length=8)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("interpretation", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="events", to="api.technicalanalysisinterpretation")),
                ("strategy", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="technical_events", to="api.strategy")),
            ],
        ),
        migrations.CreateModel(
            name="Argument",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("name", models.CharField(blank=True, default="", max_length=255)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("technical_event", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="arguments", to="api.technicalevent")),
            ],
        ),
        migrations.CreateModel(
            name="Reason",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("name", models.CharField(blank=True, default="", max_length=255)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="reasons", to="api.argument")),
            ],
        ),
    ]

