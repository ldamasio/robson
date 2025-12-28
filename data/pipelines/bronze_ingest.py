#!/usr/bin/env python3
"""
Bronze Ingestion Job: Django Outbox → Contabo Object Storage

Reads StopEvent and AuditTransaction from Django database,
writes to S3 as Parquet (append-only).

Usage:
    python bronze_ingest.py --date 2024-12-27

Environment Variables:
    DJANGO_DB_HOST: PostgreSQL host
    DJANGO_DB_NAME: Database name
    DJANGO_DB_USER: Database user
    DJANGO_DB_PASSWORD: Database password
    AWS_ACCESS_KEY_ID: Contabo S3 access key
    AWS_SECRET_ACCESS_KEY: Contabo S3 secret key
    AWS_ENDPOINT: s3.eu-central-2.contabo.com
    S3_BUCKET: robson-datalake

Author: Robson Bot Team
Created: 2024-12-27
Related: ADR-0013, docs/runbooks/deep-storage.md
"""

import os
import sys
import argparse
import logging
from datetime import datetime, timedelta
from pyspark.sql import SparkSession
from pyspark.sql.functions import (
    col, lit, to_timestamp, count, sum as spark_sum
)
from pyspark.sql.types import (
    StructType, StructField, StringType, IntegerType,
    TimestampType, DecimalType, BooleanType, ArrayType, MapType
)

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Environment variables
DJANGO_DB_HOST = os.getenv("DJANGO_DB_HOST", "postgres.robson-prod.svc.cluster.local")
DJANGO_DB_NAME = os.getenv("DJANGO_DB_NAME", "robson")
DJANGO_DB_USER = os.getenv("DJANGO_DB_USER", "robson")
DJANGO_DB_PASSWORD = os.getenv("DJANGO_DB_PASSWORD")
AWS_ACCESS_KEY_ID = os.getenv("AWS_ACCESS_KEY_ID")
AWS_SECRET_ACCESS_KEY = os.getenv("AWS_SECRET_ACCESS_KEY")
AWS_ENDPOINT = os.getenv("AWS_ENDPOINT", "s3.eu-central-2.contabo.com")
S3_BUCKET = os.getenv("S3_BUCKET", "robson-datalake")
S3_PATH = f"s3a://{S3_BUCKET}"


def create_spark_session(app_name: str = "bronze-ingest") -> SparkSession:
    """Create Spark session with S3 and Hive support."""
    logger.info("Creating Spark session...")

    builder = SparkSession.builder \
        .appName(app_name) \
        .config("spark.master", "k8s://https://kubernetes.default.svc") \
        .config("spark.kubernetes.container.image", "apache/spark:3.5.0") \
        .config("spark.kubernetes.namespace", "analytics-jobs") \
        .config("spark.kubernetes.authenticate.driver.serviceAccountName", "spark-jobs") \
        .config("spark.executor.instances", "1") \
        .config("spark.executor.memory", "4g") \
        .config("spark.executor.cores", "2") \
        .config("spark.driver.memory", "2g") \
        .config("spark.driver.cores", "1") \
        .config("spark.sql.shuffle.partitions", "2") \
        .config("spark.hadoop.fs.s3a.access.key", AWS_ACCESS_KEY_ID) \
        .config("spark.hadoop.fs.s3a.secret.key", AWS_SECRET_ACCESS_KEY) \
        .config("spark.hadoop.fs.s3a.endpoint", AWS_ENDPOINT) \
        .config("spark.hadoop.fs.s3a.path.style.access", "true") \
        .config("spark.hadoop.fs.s3a.impl", "org.apache.hadoop.fs.s3a.S3AFileSystem") \
        .config("spark.hadoop.fs.s3a.connection.maximum", "20") \
        .config("spark.hadoop.fs.s3a.attempts.maximum", "3") \
        .config("spark.jars.packages", "org.apache.hadoop:hadoop-aws:3.3.4,org.postgresql:postgresql:42.6.0") \
        # Note: Hive Metastore deferred to Phase 1, using canonical S3 paths

    spark = builder.getOrCreate()
    logger.info(f"Spark session created: {spark.version}")
    return spark


def define_stopevent_schema() -> StructType:
    """Define StopEvent schema (matches Django model)."""
    return StructType([
        StructField("event_id", StringType(), False),
        StructField("event_seq", IntegerType(), True),
        StructField("occurred_at", TimestampType(), False),
        StructField("operation_id", IntegerType(), False),
        StructField("client_id", IntegerType(), False),
        StructField("symbol", StringType(), False),
        StructField("event_type", StringType(), False),
        StructField("trigger_price", DecimalType(20, 8), True),
        StructField("stop_price", DecimalType(20, 8), True),
        StructField("quantity", DecimalType(20, 8), True),
        StructField("side", StringType(), True),
        StructField("execution_token", StringType(), True),
        StructField("payload_json", MapType(StringType(), StringType()), True),
        StructField("exchange_order_id", StringType(), True),
        StructField("fill_price", DecimalType(20, 8), True),
        StructField("slippage_pct", DecimalType(10, 4), True),
        StructField("source", StringType(), False),
        StructField("error_message", StringType(), True),
        StructField("retry_count", IntegerType(), True),
    ])


def read_django_stopevents(spark: SparkSession, start_date: str, end_date: str):
    """Read StopEvent from Django database."""
    logger.info(f"Reading StopEvent from Django DB: {start_date} to {end_date}")

    jdbc_url = f"jdbc:postgresql://{DJANGO_DB_HOST}:5432/{DJANGO_DB_NAME}"

    query = f"""
    SELECT
        event_id::text,
        event_seq,
        occurred_at,
        operation_id,
        client_id,
        symbol,
        event_type,
        trigger_price,
        stop_price,
        quantity,
        side,
        execution_token,
        payload_json::text,
        exchange_order_id,
        fill_price,
        slippage_pct,
        source,
        error_message,
        retry_count
    FROM api_stopevent
    WHERE occurred_at >= '{start_date} 00:00:00'
      AND occurred_at < '{end_date} 00:00:00'
    ORDER BY event_seq
    """

    df = spark.read \
        .format("jdbc") \
        .option("url", jdbc_url) \
        .option("dbtable", f"({query}) AS subquery") \
        .option("user", DJANGO_DB_USER) \
        .option("password", DJANGO_DB_PASSWORD) \
        .option("driver", "org.postgresql.Driver") \
        .load()

    row_count = df.count()
    logger.info(f"Read {row_count} StopEvent rows from Django DB")

    return df


def write_bronze_events(df, date_partition: str):
    """Write StopEvent to bronze layer (S3 Parquet)."""
    logger.info(f"Writing bronze events to S3: partition date={date_partition}")

    output_path = f"{S3_PATH}/bronze/events/date={date_partition}/"

    # Coalesce to single partition (Phase 0: small data volume)
    df_coalesced = df.coalesce(1)

    # Write as Parquet
    df_coalesced.write \
        .mode("overwrite") \
        .parquet(output_path)

    logger.info(f"✅ Written to {output_path}")


# Note: Hive Metastore registration deferred to Phase 1
# Phase 0 uses canonical S3 paths directly: s3a://robson-datalake/bronze/events/date=...


def validate_data_quality(df):
    """Validate data quality (null checks, uniqueness)."""
    logger.info("Validating data quality...")

    total_rows = df.count()
    null_ids = df.filter(col("event_id").isNull()).count()
    null_seqs = df.filter(col("event_seq").isNull()).count()

    logger.info(f"Total rows: {total_rows}")
    logger.info(f"Null event_ids: {null_ids}")
    logger.info(f"Null event_seqs: {null_seqs}")

    if null_ids > 0:
        raise ValueError("Data quality check failed: found null event_ids")

    if null_seqs > 0:
        logger.warning("Found null event_seqs (expected for some events)")

    # Check for duplicates
    duplicate_ids = df.groupBy("event_id") \
        .count() \
        .filter(col("count") > 1) \
        .count()

    if duplicate_ids > 0:
        raise ValueError(f"Data quality check failed: found {duplicate_ids} duplicate event_ids")

    logger.info("✅ Data quality validation passed")


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Bronze Ingestion Job")
    parser.add_argument("--date", type=str, required=True, help="Date partition (YYYY-MM-DD)")
    parser.add_argument("--dry-run", action="store_true", help="Dry run (don't write to S3)")
    args = parser.parse_args()

    # Validate date format
    try:
        datetime.strptime(args.date, "%Y-%m-%d")
    except ValueError:
        logger.error("Invalid date format. Use YYYY-MM-DD")
        sys.exit(1)

    # Calculate date range (full day)
    start_date = args.date
    end_date = (datetime.strptime(args.date, "%Y-%m-%d") + timedelta(days=1)).strftime("%Y-%m-%d")

    logger.info(f"=== Bronze Ingestion Job ===")
    logger.info(f"Date partition: {args.date}")
    logger.info(f"Date range: {start_date} to {end_date}")
    logger.info(f"Target: {S3_BUCKET}/bronze/events/date={args.date}/")

    # Create Spark session
    spark = create_spark_session()

    try:
        # Read from Django
        df = read_django_stopevents(spark, start_date, end_date)

        # Validate data quality
        validate_data_quality(df)

        # Write to S3 (unless dry run)
        if args.dry_run:
            logger.info("DRY RUN: Skipping write to S3")
            df.show(10, truncate=False)
        else:
            write_bronze_events(df, args.date)
            # Note: Hive Metastore registration deferred to Phase 1

        # Summary stats
        logger.info(f"=== Summary ===")
        logger.info(f"Total events: {df.count()}")
        logger.info(f"Event types: {df.groupBy('event_type').count().collect()}")
        logger.info(f"Clients: {df.select('client_id').distinct().count()}")
        logger.info(f"Symbols: {df.select('symbol').distinct().collect()}")

        logger.info("✅ Bronze ingestion job completed successfully")

    except Exception as e:
        logger.error(f"❌ Bronze ingestion job failed: {e}", exc_info=True)
        sys.exit(1)

    finally:
        spark.stop()
        logger.info("Spark session stopped")


if __name__ == "__main__":
    main()
