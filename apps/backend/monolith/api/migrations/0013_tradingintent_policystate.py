"""No-op migration kept to preserve history."""

from django.db import migrations


class Migration(migrations.Migration):

    dependencies = [
        ('api', '0012_audit_trail_models'),
        ('clients', '0003_alter_customuser_client'),
    ]

    operations = []
