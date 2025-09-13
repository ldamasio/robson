from django.db import migrations, models
import django.db.models.deletion


class Migration(migrations.Migration):

    dependencies = [
        ("api", "0003_analysis_models"),
    ]

    operations = [
        migrations.CreateModel(
            name="Rectangle",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("name", models.CharField(max_length=100)),
                ("reliability", models.DecimalField(decimal_places=2, max_digits=5)),
                ("width", models.DecimalField(decimal_places=4, max_digits=10)),
                ("height", models.DecimalField(decimal_places=4, max_digits=10)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
            ],
        ),
        migrations.CreateModel(
            name="Triangle",
            fields=[
                ("id", models.BigAutoField(auto_created=True, primary_key=True, serialize=False, verbose_name="ID")),
                ("created_at", models.DateTimeField(auto_now_add=True, help_text="Record creation timestamp")),
                ("updated_at", models.DateTimeField(auto_now=True, help_text="Last update timestamp")),
                ("is_active", models.BooleanField(default=True, help_text="Indicates if this record is active")),
                ("type", models.CharField(choices=[("BULLISH", "Bullish"), ("BEARISH", "Bearish"), ("NEUTRAL", "Neutral")], help_text="Market direction (Bull, Bear, Neutral)", max_length=10)),
                ("description", models.TextField(help_text="Detailed description")),
                ("confidence", models.DecimalField(decimal_places=2, default=0.0, help_text="Confidence level (0-100%)", max_digits=5)),
                ("name", models.CharField(max_length=100)),
                ("reliability", models.DecimalField(decimal_places=2, max_digits=5)),
                ("triangle_type", models.CharField(choices=[("ASCENDING", "Ascending"), ("DESCENDING", "Descending"), ("SYMMETRICAL", "Symmetrical")], max_length=20)),
                ("client", models.ForeignKey(blank=True, help_text="Client that owns this record", null=True, on_delete=django.db.models.deletion.SET_NULL, to="clients.client")),
            ],
        ),
    ]

