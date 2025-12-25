# Migration 0018: Create indexes CONCURRENTLY (zero-downtime)
# ADR-0012: Non-blocking index creation

from django.db import migrations


class Migration(migrations.Migration):
    """
    Create indexes CONCURRENTLY (non-blocking).

    Lock Analysis:
    - CONCURRENTLY = no blocking locks
    - Reads/writes continue during index creation
    - Cannot run inside transaction (atomic=False required)
    - Duration: Depends on table size (minutes for large tables)
    - Safe for production deployment (zero downtime)
    """

    dependencies = [
        ('api', '0017_set_stop_check_default'),
    ]

    # ⭐ CRITICAL: atomic=False required for CONCURRENTLY
    atomic = False

    operations = [
        # Composite index for monitor queries (status + stop_price)
        migrations.RunSQL(
            sql="""
            CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_operation_status_stop
            ON operation (status, stop_price)
            WHERE status = 'ACTIVE' AND stop_price IS NOT NULL;
            """,
            reverse_sql="DROP INDEX CONCURRENTLY IF EXISTS idx_operation_status_stop;",
        ),

        # Index for execution token lookups
        migrations.RunSQL(
            sql="""
            CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_operation_exec_token
            ON operation (stop_execution_token)
            WHERE stop_execution_token IS NOT NULL;
            """,
            reverse_sql="DROP INDEX CONCURRENTLY IF EXISTS idx_operation_exec_token;",
        ),
    ]
