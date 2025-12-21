# Generated migration for adding binance_order_id to Order model

from django.db import migrations, models


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0008_restore_legacy_models'),
    ]

    operations = [
        migrations.AddField(
            model_name='order',
            name='binance_order_id',
            field=models.CharField(blank=True, db_index=True, max_length=100, null=True),
        ),
    ]

