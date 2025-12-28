# Custom Spark Runtime Image - Robson Bot Deep Storage

## Purpose

Custom Apache Spark 3.5.0 image with all dependencies pre-installed for Phase 0 deep storage execution.

**Key Design Principle**: NO RUNTIME DOWNLOADS. All dependencies are baked into the image to ensure reproducible execution under restricted network policies (deny-all egress).

## Baked Dependencies

### Java JARs (placed in `/opt/spark/jars/`)

| Dependency | Version | Purpose |
|------------|---------|---------|
| hadoop-aws | 3.3.4 | S3A connector for Contabo Object Storage |
| aws-java-sdk-bundle | 1.12.262 | AWS SDK required by hadoop-aws |
| postgresql | 42.6.0 | PostgreSQL JDBC driver |

### Python Packages

| Package | Version | Purpose |
|---------|---------|---------|
| pyarrow | 14.0.0 | Parquet I/O (columnar storage) |
| boto3 | 1.34.0 | AWS SDK for Python (optional S3 access) |
| psycopg2-binary | 2.9.9 | PostgreSQL adapter (optional direct DB access) |

## Base Image

- **Base**: `apache/spark:3.5.0`
- **Python**: 3.11.2 (included in base)
- **Java**: OpenJDK 17 (included in base)

## Environment Variables

- `SPARK_DRIVER_MEMORY=2g`
- `SPARK_EXECUTOR_MEMORY=4g`
- `SPARK_JARS_PACKAGES=""` (disabled to prevent runtime downloads)
- `PYTHONPATH=/opt/spark/python:/opt/spark/python/lib/py4j-*-src.zip:$PYTHONPATH`

## Usage

### Building the Image

```bash
# Using make (recommended)
make build-spark-image

# Or manually
docker build -t ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 infra/images/spark/
```

### Pushing to Registry

```bash
# Using make
make push-spark-image

# Or manually
docker push ghcr.io/ldamasio/rbs-spark:3.5.0-phase0
```

### Using in Kubernetes Jobs

See: `infra/k8s/datalake/jobs/bronze-ingest-job.yml`

```yaml
spec:
  template:
    spec:
      containers:
      - name: spark-driver
        image: ghcr.io/ldamasio/rbs-spark:3.5.0-phase0
        # NO spark.jars.packages needed (JARs are pre-installed)
        # NO pip install needed (Python packages are pre-installed)
```

## Validation

```bash
# Verify Java dependencies
docker run --rm ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 \
  ls -lh /opt/spark/jars/hadoop-aws-*.jar

# Verify Python dependencies
docker run --rm ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 \
  python3 -c "import pyarrow; print('✓ PyArrow installed')"

# Test Spark
docker run --rm ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 \
  spark-submit --version
```

## Versioning

Immutable tags are used for reproducibility:
- Format: `ghcr.io/ldamasio/rbs-spark:3.5.0-phase0`
- Pattern: `{spark-version}-{phase}`
- Future: `3.5.0-phase1`, `3.5.0-prod`, etc.

## Rollback

If this image causes issues, rollback to the official image:

```bash
# Update job manifests to use:
image: apache/spark:3.5.0

# But note: you'll need to restore runtime downloads:
# - spark.jars.packages="org.apache.hadoop:hadoop-aws:3.3.4,org.postgresql:postgresql:42.6.0"
# - pip install psycopg2-binary pyarrow
```

## Related Documentation

- ADR-0013: Deep Storage Architecture
- docs/runbooks/deep-storage.md
- infra/k8s/datalake/jobs/

## Build History

| Date | Version | Changes | Author |
|------|---------|---------|--------|
| 2024-12-27 | 3.5.0-phase0 | Initial image for Phase 0 execution | Leandro Damasio |
