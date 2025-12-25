# Migration 0017: Set defaults (metadata-only, no table rewrite)
# ADR-0012: Zero-downtime migration

from django.db import migrations


class Migration(migrations.Migration):
    """
    Set default value for stop_check_count (metadata-only operation).

    Lock Analysis:
    - In PostgreSQL 11+, setting default is metadata-only (no table rewrite)
    - ACCESS EXCLUSIVE lock duration: <1 second
    - Safe for production deployment
    """

    dependencies = [
        ('api', '0016_add_stop_price_columns'),
    ]

    operations = [
        # Set default for stop_check_count (metadata-only in PG 11+)
        migrations.RunSQL(
            sql="ALTER TABLE operation ALTER COLUMN stop_check_count SET DEFAULT 0;",
            reverse_sql="ALTER TABLE operation ALTER COLUMN stop_check_count DROP DEFAULT;",
        ),
    ]
