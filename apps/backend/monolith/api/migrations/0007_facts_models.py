from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):

    dependencies = [
        ("api", "0006_merge_20250913_merge"),
    ]

    operations = [
        migrations.CreateModel(
            name="Resistance",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="resistances", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Support",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="supports", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Line",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="lines", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="TrendLine",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="trendlines", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Channel",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="channels", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Accumulation",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="accumulations", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Sideways",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="sideways", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Breakout",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="breakouts", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Uptrend",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="uptrends", to="api.argument")),
            ],
        ),
        migrations.CreateModel(
            name="Downtrend",
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
                ("argument", models.ForeignKey(on_delete=django.db.models.deletion.CASCADE, related_name="downtrends", to="api.argument")),
            ],
        ),
    ]

