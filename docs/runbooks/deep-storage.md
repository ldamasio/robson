# Deep Storage Operations Runbook

**Operational procedures for Robson Bot Deep Storage (Phase 0)**

**Last Updated**: 2025-12-28 (Phase 0 Complete)
**Related**: ADR-0013, K3S-CLUSTER-GUIDE.md

---

## Quick Reference

### Key Namespaces

| Namespace | Purpose | Quota |
|-----------|---------|-------|
| analytics-jobs | Spark jobs (bronze/silver) | 6 CPU / 12GB |
| robson | Backend (source of events) | existing |

### Key Endpoints

| Service | Endpoint | Purpose |
|---------|----------|---------|
| Contabo S3 | s3a://rbs | Deep storage (eu2.contabostorage.com) |
| Django DB | paradedb.robson.svc.cluster.local:5432 | Outbox read access |

---

## Phase 0 — Verified State

**Completion Date**: 2025-12-28
**Status**: OPERATIONAL
**Environment**: k3s cluster on Contabo Cloud VPS jaguar (24GB dedicated analytics node)

### What Was Proven

**Bronze Layer (Django Outbox → S3)**:
- Read 2 rows from stop_events table (Django Outbox pattern)
- Written to s3a://rbs/bronze/events/date=2025-12-28/*.parquet
- Storage: Contabo Object Storage (S3-compatible, eu2.contabostorage.com)
- Job: bronze-ingest-manual executed on dedicated 24GB analytics node

**Silver Layer (Bronze → Silver)**:
- Read 2 rows from bronze layer
- Transformed to 1 stop_executions (materialized view: latest state per operation_id)
- Written to s3a://rbs/silver/stop_executions/client_id=1/date=2025-12-28/*.parquet

**Infrastructure**:
- Custom Spark runtime image: ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 (550MB)
- NetworkPolicy isolation model functional (deny-all + explicit allow rules)
- Analytics node scheduling: robson.io/pool=analytics label + taints
- External S3 egress from restricted namespace

### Out of Scope (Future Work)

- Hive Metastore (not deployed in Phase 0)
- CronJob scheduling (manual Job execution only)
- Gold layer (feature engineering)
- Kubeflow Pipelines orchestration
- Prometheus monitoring

---

## Known Preconditions (REQUIRED)

1. **Custom Spark Runtime Image (REQUIRED)**: Build ghcr.io/ldamasio/rbs-spark:3.5.0-phase0 with pre-installed hadoop-aws, postgresql JDBC drivers

2. **NetworkPolicy External Egress (CRITICAL)**: Omit 'to:' section in allow-s3-egress policy to permit external Internet traffic

3. **Database Schema**: Use actual table name 'stop_events', not Django model name

4. **Namespace Labels**: Ensure namespaces have required labels (name=robson, name=analytics-jobs)

5. **Secret Escaping**: Use stringData (not data) to avoid special character escaping

6. **Parquet VOID Type**: Cast NULL literals: lit(None).cast("timestamp")

7. **Environment Variables**: Kubernetes doesn't expand shell commands - use Python fallback

---

## Known Failure Modes

| # | Failure Mode | Root Cause | Resolution |
|---|--------------|------------|------------|
| 1 | Runtime dependency downloads | Official Spark image lacks hadoop-aws, postgresql JDBC | Build custom image with pre-installed dependencies |
| 2 | External S3 blocked | namespaceSelector: {} matches only cluster namespaces, not Internet | Remove to: section from allow-s3-egress policy |
| 3 | PostgreSQL password auth fails | Secret had escaped exclamation mark | Recreate secret using stringData instead of data |
| 4 | Table not found | Using Django model name instead of actual table name | Use stop_events (actual table name) |
| 5 | Invalid timestamp | Kubernetes doesn't expand shell commands in env vars | Remove JOB_DATE env var, use Python fallback |
| 6 | Parquet VOID type | lit(None) creates VOID type in Spark SQL | Use lit(None).cast("timestamp") |

---

## References

- **ADR-0013**: Deep Storage Architecture Decision (with Phase 0 evidence)
- **Session Checkpoint**: docs/sessions/phase0-deep-storage-execution.md

---

**Last Updated**: 2025-12-28 (Phase 0 Complete)
**Maintained by**: Leandro Damasio (ldamasio@gmail.com)
