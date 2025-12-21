# Generated migration for adding is_active field to Client model

from django.db import migrations, models


class Migration(migrations.Migration):

    dependencies = [
        ('clients', '0001_initial'),
    ]

    operations = [
        migrations.AddField(
            model_name='client',
            name='is_active',
            field=models.BooleanField(default=True),
        ),
        migrations.AlterField(
            model_name='client',
            name='access_key',
            field=models.CharField(blank=True, max_length=500),
        ),
        migrations.AlterField(
            model_name='client',
            name='secret_key',
            field=models.CharField(blank=True, max_length=500),
        ),
        migrations.AlterModelOptions(
            name='client',
            options={'ordering': ['-created_at']},
        ),
    ]

