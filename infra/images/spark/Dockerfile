# Custom Spark Runtime Image for Robson Bot Deep Storage (Phase 0)
# Base: Apache Spark 3.5.0 with Python support
# Purpose: Pre-install all dependencies to avoid runtime downloads
# Created: 2024-12-27
# Related: ADR-0013, Phase 0 execution

FROM apache/spark:3.5.0

LABEL maintainer="Robson Bot Team <ldamasio@gmail.com>"
LABEL description="Custom Spark 3.5.0 with baked-in dependencies for S3 + PostgreSQL"
LABEL version="1.0.0"

# Set working directory
WORKDIR /opt/spark

# Install Java dependencies (Hadoop-AWS, AWS SDK, PostgreSQL JDBC)
# These are downloaded from Maven Central and placed in Spark's jars directory
USER root
RUN echo "=== Installing Java Dependencies ===" && \
    # Hadoop-AWS 3.3.4 (S3A connector for Spark)
    curl -sSL https://repo1.maven.org/maven2/org/apache/hadoop/hadoop-aws/3.3.4/hadoop-aws-3.3.4.jar -o /opt/spark/jars/hadoop-aws-3.3.4.jar && \
    # AWS SDK Bundle 1.12.262 (required by hadoop-aws)
    curl -sSL https://repo1.maven.org/maven2/com/amazonaws/aws-java-sdk-bundle/1.12.262/aws-java-sdk-bundle-1.12.262.jar -o /opt/spark/jars/aws-java-sdk-bundle-1.12.262.jar && \
    # PostgreSQL JDBC Driver 42.6.0
    curl -sSL https://repo1.maven.org/maven2/org/postgresql/postgresql/42.6.0/postgresql-42.6.0.jar -o /opt/spark/jars/postgresql-42.6.0.jar && \
    echo "✓ Java dependencies installed"

# Upgrade pip and install Python dependencies
RUN echo "=== Installing Python Dependencies ===" && \
    pip install --no-cache-dir --upgrade pip && \
    # PyArrow 14.0.0 (Parquet I/O)
    pip install --no-cache-dir pyarrow==14.0.0 && \
    # Boto3 1.34.0 (AWS SDK for Python - optional, for S3 direct access)
    pip install --no-cache-dir boto3==1.34.0 && \
    # Psycopg2-binary 2.9.9 (PostgreSQL adapter for Python - optional, for direct DB access)
    pip install --no-cache-dir psycopg2-binary==2.9.9 && \
    echo "✓ Python dependencies installed"

# Switch back to spark user for runtime
USER spark

# Verify installation
RUN echo "=== Verifying Installation ===" && \
    ls -lh /opt/spark/jars/hadoop-aws-*.jar /opt/spark/jars/aws-java-sdk-bundle-*.jar /opt/spark/jars/postgresql-*.jar && \
    python3 -c "import pyarrow; import boto3; import psycopg2; print('✓ All Python modules importable')"

# Disable Ivy cache writes (prevent runtime download attempts)
ENV SPARK_DRIVER_MEMORY=2g
ENV SPARK_EXECUTOR_MEMORY=4g
ENV SPARK_JARS_PACKAGES=""

# Set PYTHONPATH for PySpark (critical for direct Python usage)
ENV PYTHONPATH=/opt/spark/python:/opt/spark/python/lib/py4j-*-src.zip:$PYTHONPATH

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD spark-submit --version || exit 1

# Default entrypoint (can be overridden)
ENTRYPOINT ["/opt/spark/bin/spark-submit"]
CMD ["--help"]
