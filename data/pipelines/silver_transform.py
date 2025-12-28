#!/usr/bin/env python3
"""
Silver Transformation Job: Bronze → Silver (Cleaned Features)

Reads raw events from bronze layer, cleans and validates,
writes to silver layer as typed Parquet.

Usage:
    python silver_transform.py --date 2024-12-27

Environment Variables:
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
from datetime import datetime
from pyspark.sql import SparkSession
from pyspark.sql.functions import (
    col, lit, when, avg, percentile_approx, count, sum as spark_sum,
    row_number, to_date, trunc
)
from pyspark.sql.window import Window
from pyspark.sql.types import DecimalType

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Environment variables
AWS_ACCESS_KEY_ID = os.getenv("AWS_ACCESS_KEY_ID")
AWS_SECRET_ACCESS_KEY = os.getenv("AWS_SECRET_ACCESS_KEY")
AWS_ENDPOINT = os.getenv("AWS_ENDPOINT", "s3.eu-central-2.contabo.com")
S3_BUCKET = os.getenv("S3_BUCKET", "robson-datalake")
S3_PATH = f"s3a://{S3_BUCKET}"


def create_spark_session(app_name: str = "silver-transform") -> SparkSession:
    """Create Spark session with S3 and Hive support."""
    logger.info("Creating Spark session...")

    builder = SparkSession.builder \
        .appName(app_name) \
        .config("spark.master", "k8s://https://kubernetes.default.svc") \
        .config("spark.kubernetes.container.image", "apache/spark:3.5.0") \
        .config("spark.kubernetes.namespace", "analytics-jobs") \
        .config("spark.kubernetes.authenticate.driver.serviceAccountName", "spark-jobs") \
        .config("spark.executor.instances", "2") \
        .config("spark.executor.memory", "4g") \
        .config("spark.executor.cores", "2") \
        .config("spark.driver.memory", "2g") \
        .config("spark.driver.cores", "1") \
        .config("spark.sql.shuffle.partitions", "4") \
        .config("spark.hadoop.fs.s3a.access.key", AWS_ACCESS_KEY_ID) \
        .config("spark.hadoop.fs.s3a.secret.key", AWS_SECRET_ACCESS_KEY) \
        .config("spark.hadoop.fs.s3a.endpoint", AWS_ENDPOINT) \
        .config("spark.hadoop.fs.s3a.path.style.access", "true") \
        .config("spark.hadoop.fs.s3a.impl", "org.apache.hadoop.fs.s3a.S3AFileSystem") \
        .config("spark.hadoop.fs.s3a.connection.maximum", "20") \
        .config("spark.hadoop.fs.s3a.attempts.maximum", "3") \
        .config("spark.jars.packages", "org.apache.hadoop:hadoop-aws:3.3.4") \
        # Note: Hive Metastore deferred to Phase 1, using canonical S3 paths

    spark = builder.getOrCreate()
    logger.info(f"Spark session created: {spark.version}")
    return spark


def read_bronze_events(spark: SparkSession, date_partition: str):
    """Read StopEvent from bronze layer."""
    logger.info(f"Reading bronze events: date={date_partition}")

    input_path = f"{S3_PATH}/bronze/events/date={date_partition}/"

    df = spark.read.parquet(input_path)

    row_count = df.count()
    logger.info(f"Read {row_count} rows from bronze layer")

    return df


def transform_to_stop_executions(df):
    """Transform bronze events to silver stop_executions (materialized view)."""
    logger.info("Transforming events to stop_executions...")

    # Filter only execution-related events
    execution_events = df.filter(
        col("event_type").isin([
            "STOP_TRIGGERED",
            "EXECUTION_SUBMITTED",
            "EXECUTED",
            "FAILED",
            "BLOCKED"
        ])
    )

    # Get latest state per operation_id (materialized view logic)
    window_spec = Window.partitionBy("operation_id").orderBy(col("event_seq").desc())

    latest_events = execution_events \
        .withColumn("row_num", row_number().over(window_spec)) \
        .filter(col("row_num") == 1) \
        .drop("row_num")

    # Derive status from latest event_type
    latest_events = latest_events \
        .withColumn("status",
            when(col("event_type") == "STOP_TRIGGERED", "PENDING")
            .when(col("event_type") == "EXECUTION_SUBMITTED", "SUBMITTED")
            .when(col("event_type") == "EXECUTED", "EXECUTED")
            .when(col("event_type") == "FAILED", "FAILED")
            .when(col("event_type") == "BLOCKED", "BLOCKED")
            .otherwise("UNKNOWN")
        )

    # Rename columns for silver layer
    silver_df = latest_events.select(
        col("event_id").alias("execution_id"),
        col("operation_id"),
        col("client_id"),
        col("symbol"),
        col("stop_price"),
        col("trigger_price"),
        col("quantity"),
        col("side"),
        col("status"),
        # Map occurred_at to triggered_at (simplification)
        when(col("event_type") == "STOP_TRIGGERED", col("occurred_at")).alias("triggered_at"),
        # TODO: Add submitted_at, executed_at via event replay
        lit(None).alias("submitted_at"),
        lit(None).alias("executed_at"),
        col("exchange_order_id"),
        col("fill_price"),
        col("slippage_pct"),
        col("source")
    )

    logger.info(f"Transformed to {silver_df.count()} stop_executions")

    return silver_df


def validate_silver_data(df):
    """Validate silver layer data quality."""
    logger.info("Validating silver layer data quality...")

    total_rows = df.count()
    null_ids = df.filter(col("execution_id").isNull()).count()
    null_statuses = df.filter(col("status").isNull()).count()

    logger.info(f"Total rows: {total_rows}")
    logger.info(f"Null execution_ids: {null_ids}")
    logger.info(f"Null statuses: {null_statuses}")

    if null_ids > 0:
        raise ValueError("Data quality check failed: found null execution_ids")

    if null_statuses > 0:
        raise ValueError("Data quality check failed: found null statuses")

    # Check status enum
    valid_statuses = ["PENDING", "SUBMITTED", "EXECUTED", "FAILED", "BLOCKED"]
    invalid_statuses = df.filter(~col("status").isin(valid_statuses)).count()

    if invalid_statuses > 0:
        raise ValueError(f"Data quality check failed: found {invalid_statuses} invalid statuses")

    logger.info("✅ Silver data quality validation passed")


def write_silver_stop_executions(df, date_partition: str):
    """Write stop_executions to silver layer (S3 Parquet)."""
    logger.info(f"Writing silver stop_executions to S3: partition date={date_partition}")

    # Get unique clients for partitioning
    clients = [row.client_id for row in df.select("client_id").distinct().collect()]

    for client_id in clients:
        client_df = df.filter(col("client_id") == client_id)
        output_path = f"{S3_PATH}/silver/stop_executions/client_id={client_id}/date={date_partition}/"

        # Coalesce to single partition per client
        client_df_coalesced = client_df.coalesce(1)

        # Write as Parquet
        client_df_coalesced.write \
            .mode("overwrite") \
            .parquet(output_path)

        logger.info(f"✅ Written client {client_id} to {output_path}")


# Note: Hive Metastore registration deferred to Phase 1
# Phase 0 uses canonical S3 paths directly: s3a://robson-datalake/silver/...


def compute_summary_stats(df):
    """Compute summary statistics for logging."""
    logger.info("=== Summary Statistics ===")

    # Status distribution
    status_dist = df.groupBy("status").count().collect()
    logger.info("Status distribution:")
    for row in status_dist:
        logger.info(f"  {row.status}: {row.count}")

    # Slippage stats
    slippage_stats = df.agg(
        avg("slippage_pct").alias("avg_slippage"),
        percentile_approx("slippage_pct", 0.5).alias("median_slippage"),
        percentile_approx("slippage_pct", 0.95).alias("p95_slippage")
    ).collect()[0]

    logger.info(f"Slippage stats:")
    logger.info(f"  Avg: {slippage_stats['avg_slippage']:.4f}%")
    logger.info(f"  Median: {slippage_stats['median_slippage']:.4f}%")
    logger.info(f"  P95: {slippage_stats['p95_slippage']:.4f}%")

    # Symbol distribution
    symbol_dist = df.groupBy("symbol").count().orderBy("count", ascending=False).collect()
    logger.info("Top symbols:")
    for row in symbol_dist[:5]:
        logger.info(f"  {row.symbol}: {row.count}")


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Silver Transformation Job")
    parser.add_argument("--date", type=str, required=True, help="Date partition (YYYY-MM-DD)")
    parser.add_argument("--dry-run", action="store_true", help="Dry run (don't write to S3)")
    args = parser.parse_args()

    # Validate date format
    try:
        datetime.strptime(args.date, "%Y-%m-%d")
    except ValueError:
        logger.error("Invalid date format. Use YYYY-MM-DD")
        sys.exit(1)

    logger.info(f"=== Silver Transformation Job ===")
    logger.info(f"Date partition: {args.date}")

    # Create Spark session
    spark = create_spark_session()

    try:
        # Read from bronze
        bronze_df = read_bronze_events(spark, args.date)

        # Transform to silver
        silver_df = transform_to_stop_executions(bronze_df)

        # Validate data quality
        validate_silver_data(silver_df)

        # Write to S3 (unless dry run)
        if args.dry_run:
            logger.info("DRY RUN: Skipping write to S3")
            silver_df.show(10, truncate=False)
        else:
            write_silver_stop_executions(silver_df, args.date)
            # Note: Hive Metastore registration deferred to Phase 1

        # Compute summary stats
        compute_summary_stats(silver_df)

        logger.info("✅ Silver transformation job completed successfully")

    except Exception as e:
        logger.error(f"❌ Silver transformation job failed: {e}", exc_info=True)
        sys.exit(1)

    finally:
        spark.stop()
        logger.info("Spark session stopped")


if __name__ == "__main__":
    main()
