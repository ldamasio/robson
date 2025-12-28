# ADR-0013: Deep Storage Architecture with Object Storage and Spark

**Status**: Accepted
**Date**: 2024-12-27
**Context**: Robson Bot v1.3
**Related**: ADR-0002 (Hexagonal Architecture), ADR-0012 (ParadeDB Primary Database)

---

## Context

Robson Bot requires a deep storage layer for:

1. **Event Sourcing Replay**: Re-process historical events for backfill and bug fixes
2. **Feature Engineering**: Generate training data for ML models (slippage prediction, execution success)
3. **Analytical Queries**: Ad-hoc analysis of trading performance, risk metrics, and strategy effectiveness
4. **Audit Trail**: Immutable log of all financial movements and decisions

**Existing Constraints**:
- k3s cluster across 4 Contabo VPS (2×8GB + 2×4GB RAM, 24GB total)
- Limited storage capacity (local-path provisioner, hostPath-based)
- Event sourcing already implemented (`StopEvent`, `AuditTransaction` models)
- Desire to avoid operational overhead (MinIO, self-hosted object stores)
- Parquet desired as core storage format

**Requirements**:
- Scalable beyond current cluster limits
- Cost-effective for multi-tenant data lake
- Integrates with existing event sourcing patterns
- Supports replay, reprocessing, and training data generation
- Minimal operational overhead

---

## Decision

**Adopt a phased deep storage architecture using Contabo Object Storage + Spark on Kubernetes**.

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│  Deep Storage = Contabo Object Storage (S3-compatible)         │
│  Compute = Spark on Kubernetes (spark-submit)                  │
│  Catalog = Phase 0: canonical paths / Phase 1+: Hive Metastore │
│  Format = Apache Parquet (columnar, compressed)                │
└─────────────────────────────────────────────────────────────────┘

Data Layers:
├── bronze/    Raw events (append-only, JSON → Parquet)
├── silver/    Cleaned features (typed, validated)
└── gold/      ML-ready datasets (versioned, feature-engineered)

PHASE 0 SCOPE:
- No Hive Metastore (uses canonical S3 paths: s3a://robson-datalake/...)
- Spark jobs via Kubernetes Jobs (spark-submit embedded)
- Single analytics node (24GB, labeled robson.io/pool=analytics, tainted robson.io/dedicated=analytics:NoSchedule)
- Bronze + Silver jobs only (Gold deferred to Phase 1)

PHASE 1+ SCOPE:
- Hive Metastore (PostgreSQL backend) for SQL catalog
- CronJobs for scheduled execution
- Gold feature generation
- Data quality validation
```

Ingestion:
├── Phase 0: Batch polling from Django Outbox (hourly)
├── Phase 1: CDC from Django WAL or streaming (real-time)
└── Phase 2: Change Data Capture with schema registry
```

### Key Decisions

| Decision | Rationale |
|----------|-----------|
| **Contabo Object Storage over HDFS** | Infinite scalability, separates compute/storage, cost-effective |
| **No HDFS** | Object storage superior for this scale, avoids DataNode overhead on 4GB nodes |
| **No MinIO** | Contabo provides same capability without self-hosting overhead |
| **Spark-on-Kubernetes over YARN** | k3s already manages resources, avoids YARN complexity |
| **spark-submit over Spark Operator** | Simpler for Phase 0, can migrate to operator later |
| **Hive Metastore (lightweight)** | Required for Spark SQL + Parquet abstraction |
| **Batch ingestion first** | Simpler than streaming, sufficient for hourly latency |
| **Parquet for all layers** | Columnar compression, Spark-native, supports schema evolution |

### Hadoop's Role (Clarified)

**What "Hadoop" means in this architecture**:
- ✅ **Hadoop Common** (S3A connector for Spark to talk S3) - Phase 0
- ⏳ **Hive Metastore** (catalog for Parquet tables) - Phase 1+ (deferred for minimal Phase 0)
- ❌ **HDFS** (replaced by object storage) - Not planned
- ❌ **YARN** (replaced by Kubernetes) - Not planned

**Phase 0**: Canonical S3 paths (`s3a://robson-datalake/bronze/events/date=.../`)
**Phase 1+**: Hive Metastore for SQL queries (`SELECT * FROM bronze.events WHERE ...`)

**Why no HDFS?**
- HDFS requires dedicated DataNodes (memory overhead)
- Replication factor consumes limited disk space
- NameNode single point of failure (HA requires additional infrastructure)
- Operational complexity (balancer, decommissioning)
- Object storage provides better durability (11 9's) and scalability

**Why defer Hive Metastore?**
- Phase 0: Operational simplicity, canonical paths sufficient
- Phase 1+: SQL ergonomics require catalog for ad-hoc queries
- Trade-off: Setup time vs query convenience

---

## Alternatives Considered

### Option A: HDFS on k3s (Rejected)

**Pros**:
- Lower latency (<10ms vs 100ms for S3)
- No external dependency

**Cons**:
- DataNode memory overhead (~1GB per node) exceeds 4GB node capacity
- Replication factor (3×) wastes limited disk space
- NameNode single point of failure (HA requires additional infrastructure)
- Operational complexity (balancer, decommissioning)

**Verdict**: Overkill for 4-node cluster, object storage is superior fit.

### Option B: MinIO Self-Hosted (Rejected per User Requirement)

**Pros**:
- S3-compatible API
- Self-hosted (no external dependency)
- Can use local-path storage

**Cons**:
- Operational overhead (monitoring, upgrades, healing)
- No better than Contabo Object Storage for single-tenant use case
- Additional resource consumption (MinIO servers)

**Verdict**: User explicitly rejected MinIO. Contabo Object Storage provides same capability.

### Option C: garage (Rust-based Object Store) (Rejected)

**Pros**:
- Rust implementation (memory-efficient, secure)
- Distributed S3-compatible store
- Interesting technology

**Cons**:
- Operational complexity (cluster management, consensus)
- Immature ecosystem compared to MinIO
- No benefit over Contabo Object Storage for single-tenant

**Verdict**: Interesting for distributed scenarios, but overkill here.

### Option D: Pure SQL on ParadeDB (Rejected)

**Pros**:
- No new infrastructure
- Already have ParadeDB deployed

**Cons**:
- Not optimized for analytical workloads (columnar storage)
- Doesn't separate compute from storage
- Can't scale independently
- No event replay at scale

**Verdict**: ParadeDB remains OLTP database, deep storage requires separate architecture.

---

## Consequences

### Positive

1. **Scalability**: Object storage scales independently of compute
2. **Cost**: Contabo Object Storage is cost-effective vs additional VPS for HDFS
3. **Operational Simplicity**: No DataNodes, no NameNode HA, no YARN
4. **Replayability**: Bronze layer is append-only, supports event replay
5. **ML-Ready**: Parquet + Spark integrates with MLflow, TensorFlow, PyTorch
6. **Separation of Concerns**: Analytics cluster can be separated from app cluster later

### Negative

1. **S3 Latency**: 100ms vs <10ms for HDFS (acceptable for batch, not streaming)
2. **External Dependency**: Contabo Object Storage is external to cluster
3. **Network Cost**: Egress charges if data leaves Contabo network (negligible for same-region)
4. **Learning Curve**: Team must learn Spark, Parquet, Hive Metastore

### Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Contabo S3 API rate limits | Medium (job delays) | Exponential backoff, retry logic, job queuing |
| Hive Metastore single point of failure | High (query failures) | Phase 2: Patroni + PostgreSQL HA |
| OOMKilled on analytics node | High (job failure) | Phase 0: Conservative executor sizing on 24GB node (see sizing recommendations) |
| Schema drift in bronze layer | Medium (query failures) | Schema registry, versioned `_metadata/` folder |
| NetworkPolicy blocks S3 access | Critical (no ingress) | Explicit egress allow to S3 endpoint |

---

## Implementation Plan

### Phase 0 (Week 1-2): Minimal Deep Storage

**Infrastructure Requirements**:
- Contabo Cloud VPS 30 (24GB RAM) as dedicated analytics worker node
  - Label: `robson.io/pool=analytics`
  - Taint: `robson.io/dedicated=analytics:NoSchedule`
  - Worker-only (no control plane components)

**Deliverables**:
1. Namespaces: `datalake-system`, `analytics-jobs`
2. NetworkPolicies for isolation
3. Spark on k8s integration (spark-submit)
4. Bronze ingestion job (Django Outbox → S3)
5. Silver transformation job (bronze → silver)
6. Sample gold dataset (manual, training set)

**Success Criteria**:
- ✅ Bronze job writes Parquet to Contabo Object Storage
- ✅ Silver job reads bronze, writes silver Parquet to S3
- ✅ End-to-end: Django event → S3 bronze → S3 silver in <1 hour
- ✅ Jobs scheduled on dedicated 24GB analytics node
- ✅ Total infra cost: €0/month (uses existing hardware + Contabo Object Storage add-on)

**Rollback Plan**:
```bash
kubectl delete namespace analytics-jobs datalake-system
aws s3 rm --recursive s3://robson-datalake/
argocd app delete robson-datalake --cascade
```

### Phase 1 (Month 3-6): Governance and Streaming

**Trigger**: Bronze layer stable, need for real-time features

**Deliverables**:
1. Hive Metastore (PostgreSQL backend) for SQL catalog
2. Schema registry (JSON Schema or Protobuf)
3. CDC from Django WAL or streaming from RabbitMQ
4. Data quality validation (Great Expectations)
5. Feature store for gold layer (Hudi DeltaStreamer)
6. Lineage tracking (OpenLineage)

**Upgrade Trigger**: Add 1×24GB VPS when daily CDC processing >4 hours

### Phase 2 (Month 6-12): High Availability

**Trigger**: Production SLA >99% or multi-user data science team

**Deliverables**:
1. HA Hive Metastore (Patroni + PostgreSQL cluster)
2. S3 backup/redundancy
3. RBAC for Spark jobs
4. Secrets encryption (External Secrets Operator)
5. Monitoring (Prometheus + Grafana)
6. Alerting (job failures, SLO breaches)

**Upgrade Trigger**: Add 2×24GB VPS for HA and monitoring

### Phase 3 (Month 12+): Advanced Analytics

**Trigger**: Need for interactive queries or ML pipeline orchestration

**Deliverables**:
1. Presto/Trino for interactive SQL
2. Kubeflow Pipelines for ML workflow orchestration (see "Kubeflow Later" section)
3. MLflow for experiment tracking
4. JupyterHub for data science notebooks
5. Separate analytics k3s cluster (4×24GB nodes)

---

## Data Layout

```
s3://robson-datalake/
├── bronze/                        (Raw, append-only, immutable)
│   ├── events/
│   │   ├── date=2024-12-27/
│   │   │   ├── part-0001.parquet
│   │   │   └── part-0002.parquet
│   ├── orders/                    (AuditTransaction)
│   │   └── client_id=1/date=2024-12-27/
│   └── trades/                    (Binance sync)
│       └── date=2024-12-27/
│
├── silver/                        (Cleaned, typed, validated)
│   ├── stop_executions/
│   │   └── client_id=1/date=2024-12-27/
│   ├── operations/
│   │   └── strategy_id=1/date=2024-12-27/
│   └── portfolio_snapshots/
│       └── client_id=1/date=2024-12-27/
│
└── gold/                          (ML-ready, feature-engineered)
    ├── training_sets/
    │   ├── stop_outcome/v1.0.0/
    │   └── slippage_prediction/v1.0.0/
    └── features/                  (Online store for inference)
        └── stop_execution_features/

RETENTION:
- Bronze: 90 days
- Silver: 365 days
- Gold: Indefinite (versioned)
```

**Partitioning Strategy**:
- Primary: `date` (for time-series queries)
- Secondary: `client_id` (multi-tenant isolation)
- Tertiary: `strategy_id` or `symbol` (use case-specific)

---

## Security Model

### Namespace Isolation

```
┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
│   robson-prod   │   │    staging     │   │ datalake-system │
│  (app backend)  │   │  (dev/testing) │   │  (metastore)    │
└─────────────────┘   └─────────────────┘   └─────────────────┘
           │                     │                      ▲
           │                     │                      │
           ▼                     ▼                      │
  ┌─────────────────┐   ┌─────────────────┐             │
  │analytics-jobs   │◄──┤ analytics-jobs  │─────────────┘
  │  (CronJobs)     │   │  (staging)      │
  └─────────────────┘   └─────────────────┘
           │                     │
           ▼                     ▼
  ┌─────────────────────────────────────────────────────────────────┐
  │                CONTABO OBJECT STORAGE (External)                 │
  └─────────────────────────────────────────────────────────────────┘
```

### Network Policies

- **Default Deny**: All namespaces block all ingress/egress by default
- **Explicit Allow**: Whitelist DNS, Hive Metastore, S3 endpoint
- **Production Isolation**: `analytics-jobs` blocked from `staging` namespace

### Secrets Management

- **Phase 0**: Kubernetes Secrets (manual, acceptably simple)
- **Phase 1**: Sealed Secrets (GitOps-friendly, encrypted in repo)
- **Phase 2**: External Secrets Operator (AWS Secrets Manager or Vault)

---

## Operational Costs

### Phase 0 (Current Hardware)

| Component | CPU | Memory | Storage | Monthly Cost |
|-----------|-----|--------|---------|--------------|
| Spark jobs (ephemeral) | 6 cores | 20GB | - | €0 (existing nodes) |
| Analytics node (24GB) | 8 cores | 24GB | - | €15.49/month (VPS 30) |
| Contabo Object Storage | - | - | 100GB | ~€0.50/month |
| **Total** | | | | **~€16/month** |

**Note**: 24GB analytics node is dedicated to Spark workloads only (worker-only, no control plane).

### Phase 1 (Add 1×24GB VPS)

| Component | Additional Cost |
|-----------|-----------------|
| 1×24GB Contabo VPS | €15.49/month |
| **Total Phase 1** | **~€31.50/month** |

### Phase 2 (Add HA)

| Component | Additional Cost |
|-----------|-----------------|
| 2×24GB Contabo VPS | €30.98/month |
| **Total Phase 2** | **~€62.50/month** |

---

## Spark Executor Sizing (24GB Analytics Node)

### Phase 0: Single-Node Configuration

**Node**: Contabo Cloud VPS 30 (24GB RAM, 8 CPU cores)

**Conservative Baseline** (recommended for Phase 0):
```
Driver: 1 core, 2GB memory
Executor: 2 cores, 6GB memory × 2 executors = 4 cores, 12GB total
Overhead: 2GB for OS + Kubernetes
Total: 5 cores, 14GB used (10GB headroom available)
```

**Job-Specific Configurations**:

| Job Type | Driver | Executors | Cores | Memory | Use Case |
|----------|--------|-----------|-------|--------|----------|
| Bronze Ingestion | 1C / 2GB | 1 × 2C / 4GB | 3 | 6GB | Simple JDBC read + S3 write |
| Silver Transform | 1C / 2GB | 2 × 2C / 6GB | 5 | 14GB | Complex joins + aggregations |
| Gold Features (Phase 1+) | 1C / 2GB | 2 × 3C / 8GB | 7 | 18GB | ML feature engineering |

**Memory Fraction Tuning** (for 6GB executor):
```properties
spark.executor.memory=4g
spark.executor.memoryOverhead=2g
spark.memory.fraction=0.6        # 60% for storage (cache)
spark.memory.storageFraction=0.5  # 50% of storage for cache
```

**Phase 0 ResourceQuota**:
```yaml
analytics-jobs:
  requests.cpu: "3"
  requests.memory: 6Gi
  limits.cpu: "6"
  limits.memory: 12Gi
```

### Phase 1+: Multi-Node Configuration

**When to scale**: Single 24GB node saturates (jobs >4 hours, OOMKilled >1/day)

**Add 1×24GB VPS** → 2-node cluster:
```
Driver: Node 1, 1 core, 2GB
Executors: 2 nodes, 2 × 3 cores, 2 × 8GB = 6 cores, 16GB total
Parallelism: 2 jobs concurrently (1 per node)
```

---

## Kubeflow-Ready Extension Points (Deferred to Phase 3)

### Current Phase 0/1: Manual Spark Jobs

**What we have today**:
- Kubernetes Jobs for bronze/silver/gold
- Manual spark-submit commands
- CronJob-based scheduling
- No pipeline orchestration

### Kubeflow Readiness: Design Principles

**Extension points baked into Phase 0 design**:

1. **Stable Dataset Layout**:
   - Canonical S3 paths (`s3a://robson-datalake/bronze/events/date=.../`)
   - Versioned gold datasets (`gold/training_sets/v1.0.0/`)
   - Partitioned by `date`, `client_id`, `strategy_id`
   - **Kubeflow compatibility**: Input/output paths are fixed, reproducible

2. **Idempotent Job Specifications**:
   - Jobs use `mode("overwrite")` for Parquet writes
   - Partition-based (no duplicate data on re-run)
   - No external state dependencies
   - **Kubeflow compatibility**: Pipelines can retry steps without side effects

3. **Namespace and RBAC Separation**:
   - `analytics-jobs` namespace isolated from app workloads
   - ServiceAccount `spark-jobs` with minimal permissions
   - NetworkPolicies enforce least privilege
   - **Kubeflow compatibility**: Can deploy Kubeflow in dedicated namespace without conflicts

4. **Containerized Spark Jobs**:
   - Spark driver/executors run in containers
   - No host dependencies
   - Environment-based configuration (secrets, env vars)
   - **Kubeflow compatibility**: Job specs can be wrapped in Kubeflow Pipelines DSL

### When to Adopt Kubeflow (Phase 3)

**Triggers**:
- Need for complex DAG dependencies (bronze → silver → gold → ML train → ML eval)
- Multiple data scientists collaborating on pipelines
- Need for pipeline visualization and monitoring
- Requirement for pipeline versioning and rollback
- Hyperparameter tuning at scale (Katib integration)

**What Kubeflow Would Replace**:

| Current (Phase 0/1) | With Kubeflow (Phase 3) |
|---------------------|-------------------------|
| Manual `kubectl apply -f job.yaml` | `kfp run --pipeline-id xxx` |
| CronJobs for scheduling | Kubeflow Pipelines cron scheduler |
| Manual job orchestration | DAG-based dependencies (bronze → silver → gold) |
| Local ML training scripts | Kubeflow Pipelines + Katib (hyperparameter tuning) |
| Manual experiment tracking | MLflow integration (Kubeflow metadata) |

**What Stays Unchanged**:

| Component | Why Unchanged |
|-----------|---------------|
| **S3 Data Layout** | Canonical paths are Kubeflow-agnostic |
| **Spark Jobs** | Spark executors run as containers in both cases |
| **Parquet Format** | Kubeflow doesn't change storage format |
| **NetworkPolicies** | Isolation requirements remain |
| **RBAC** | ServiceAccount permissions similar |
| **Django → Bronze Ingestion** | Data source unchanged |

### Migration Path (Phase 3)

**Step 1**: Deploy Kubeflow Pipelines in `kubeflow` namespace
**Step 2**: Convert existing Spark Jobs to Kubeflow Pipeline DSL (Python)
**Step 3**: Replace CronJobs with Kubeflow cron scheduler
**Step 4**: Add Katib for hyperparameter tuning (gold training sets)
**Step 5**: Gradual rollout (run both CronJobs and Kubeflow in parallel)

**Key Point**: Phase 0 design decisions ensure zero breaking changes when migrating to Kubeflow. The data layout, job specs, and infrastructure are already compatible.

---

## References

- [Spark on Kubernetes](https://spark.apache.org/docs/latest/running-on-kubernetes.html)
- [Hadoop S3A Connector](https://hadoop.apache.org/docs/stable/hadoop-aws/tools/hadoop-aws/index.html)
- [Hive Metastore](https://hive.apache.org/concepts/metastore.html)
- [Parquet Format](https://parquet.apache.org/)
- [Contabo Object Storage](https://contabo.com/en/object-storage/)

---

---

## SESSION CHECKPOINT — END OF DAY (2024-12-27)

### Phase 0 Scope and Decisions

**Implemented in Repository** (Scaffolding + Manifests + Jobs):
- ✅ ADR-0013 documenting architecture decisions
- ✅ Runbook with operational procedures
- ✅ Data schemas, contracts, datasets documentation
- ✅ Kubernetes manifests:
  - Namespaces: `datalake-system`, `analytics-jobs` (with ResourceQuotas)
  - NetworkPolicies: Default-deny + explicit allow (DNS, k8s API, S3)
  - RBAC: ServiceAccount + Role + RoleBinding for Spark
  - Jobs: `bronze-ingest-job.yaml`, `silver-transform-job.yaml` (REAL manifests, not just references)
- ✅ Python pipelines: `bronze_ingest.py`, `silver_transform.py` (Hive Metastore removed for Phase 0)
- ✅ Smoke test script: `smoke-test.sh` (S3 validation, no Hive checks)
- ✅ Makefile: Fixed typo (bronce → bronze)

**NOT Yet Executed on Live Cluster** (Requires manual validation):
- ❌ Namespaces not created on k3s cluster
- ❌ NetworkPolicies not applied
- ❌ Secrets not created (contabo-s3-credentials, django-db-credentials)
- ❌ Jobs never executed (no bronze/silver data in S3)
- ❌ S3 bucket may not exist (verify first)
- ❌ 16GB analytics node not provisioned (planned, not deployed)

### Hadoop Role Clarification

**What "Hadoop" Means in This Architecture**:
- ✅ **Hadoop Common (S3A connector)**: Phase 0 - Used by Spark to read/write S3
- ❌ **HDFS**: Explicitly deferred - Replaced by Contabo Object Storage
- ❌ **YARN**: Explicitly deferred - Replaced by Kubernetes scheduler
- ⏳ **Hive Metastore**: Deferred to Phase 1 - Phase 0 uses canonical S3 paths

**DO NOT Repeat**:
- No HDFS DataNodes (object storage superior for this scale)
- No YARN ResourceManager/NodeManager (k3s manages resources)
- No MinIO (Contabo Object Storage provides S3-compatible endpoint)
- No Hive Metastore in Phase 0 (operational simplicity > SQL ergonomics)

### Scheduling Decision

**Dedicated 24GB Analytics Node** (Contabo Cloud VPS 30, Planned, Not Yet Deployed):
- Labels: `robson.io/pool=analytics`
- Taints: `robson.io/dedicated=analytics:NoSchedule`
- Worker-only (no control plane components)
- All Spark jobs include `nodeSelector` and `tolerations` to target this pool
- Job manifests ready (will schedule on analytics node once provisioned)

**Current Hardware** (Existing 4 VPS):
- 2×8GB RAM (app workloads)
- 2×4GB RAM (too small for Spark executors)
- Analytics jobs will remain Pending until 16GB node added

### Open Items (Cluster-Side Validation)

1. **Contabo S3 Setup**:
   - [ ] Create bucket `robson-datalake` in Contabo panel
   - [ ] Generate access keys (Access Key ID + Secret Access Key)
   - [ ] Test connectivity via AWS CLI

2. **Kubernetes Secrets**:
   - [ ] Create `contabo-s3-credentials` secret in `datalake-system` namespace
   - [ ] Create `django-db-credentials` secret in `analytics-jobs` namespace
   - [ ] (Alternative) Use Sealed Secrets for GitOps-friendly secrets

3. **Cluster Deployment**:
   - [ ] Apply namespace manifests (`kubectl apply -f infra/k8s/datalake/namespaces/`)
   - [ ] Apply RBAC (`kubectl apply -f infra/k8s/datalake/rbac/`)
   - [ ] Apply NetworkPolicies (`kubectl apply -f infra/k8s/datalake/network-policies/`)
   - [ ] Verify ResourceQuotas enforced

4. **Analytics Node Provisioning**:
   - [ ] Order Contabo Cloud VPS 30 (24GB RAM)
   - [ ] Add to k3s cluster as worker-only node
   - [ ] Apply label: `kubectl label node <node> robson.io/pool=analytics`
   - [ ] Apply taint: `kubectl taint nodes <node> robson.io/dedicated=analytics:NoSchedule`

5. **First Real Run**:
   - [ ] Run smoke test (`./data/scripts/smoke-test.sh`)
   - [ ] Execute bronze job (`kubectl apply -f infra/k8s/datalake/jobs/bronze-ingest-job.yml`)
   - [ ] Verify S3 output: `aws s3 ls s3://robson-datalake/bronze/events/ --recursive`
   - [ ] Execute silver job (`kubectl apply -f infra/k8s/datalake/jobs/silver-transform-job.yml`)
   - [ ] Verify silver output: `aws s3 ls s3://robson-datalake/silver/stop_executions/ --recursive`

6. **Validation**:
   - [ ] Confirm Parquet files readable via Spark
   - [ ] Verify data quality checks pass
   - [ ] Check resource usage within quotas
   - [ ] Verify jobs scheduled on analytics node only

**Next Session**: Start with item 1 (Contabo S3 setup), then proceed sequentially.

---

**Authors**: Leandro Damasio (ldamasio@gmail.com)
**Reviewers**: (Pending)
**Approval**: (Pending)

**Last Updated**: 2024-12-27 (Session Checkpoint added)

---

## Phase 0 Completion Evidence (2024-12-28)

**Status**: ✅ **PHASE 0 COMPLETE**

### What Was Proven

**Bronze Layer (Django → S3)**:
- Read 2 rows from `stop_events` table (Django Outbox pattern)
- Written to `s3a://rbs/bronze/events/date=2024-12-28/*.parquet`
- Storage: Contabo Object Storage (S3-compatible, eu2.contabostorage.com)
- Job: `bronze-ingest-manual` executed on dedicated 24GB analytics node (jaguar)

**Silver Layer (Bronze → Silver)**:
- Read 2 rows from bronze layer
- Transformed to 1 `stop_executions` (materialized view: latest state per `operation_id`)
- Written to `s3a://rbs/silver/stop_executions/client_id=1/date=2024-12-28/*.parquet`
- Job: `silver-transform-manual` executed on dedicated 24GB analytics node (jaguar)

**Technology Stack Verified**:
- ✅ Spark 3.5.0 on k3s (spark-submit embedded in Kubernetes Jobs)
- ✅ Hadoop-AWS 3.3.4 (S3A connector for S3 access)
- ✅ PostgreSQL JDBC 42.6.0 (Django database read)
- ✅ PyArrow 14.0.0 (Parquet columnar format)
- ✅ Contabo Object Storage (S3-compatible external object store)

### Custom Runtime Image

**Image**: `ghcr.io/ldamasio/rbs-spark:3.5.0-phase0` (550MB)
- **Base**: `apache/spark:3.5.0`
- **Baked Dependencies**:
  - JARs: `hadoop-aws-3.3.4.jar`, `aws-java-sdk-bundle-1.12.262.jar`, `postgresql-42.6.0.jar`
  - Python: `pyarrow==14.0.0`, `boto3==1.34.0`, `psycopg2-binary==2.9.9`
- **Rationale**: Eliminates runtime dependency downloads under restricted networking (deny-all-default policy)

### Known Preconditions (Lessons Learned)

1. **Custom Spark Image Required**: Official `apache/spark:3.5.0` lacks S3A/JDBC drivers
2. **NetworkPolicy External Egress**: `namespaceSelector: {}` matches only cluster namespaces, NOT external Internet
   - **Fix**: Omit `to:` section in `allow-s3-egress` policy to permit external S3 access
3. **Database Table Name**: Use `stop_events` (actual table name), not `api_stopevent` (Django model name)
4. **Namespace Labels Required**: `robson` namespace must have label `name=robson` for PostgreSQL NetworkPolicy match
5. **Secret Escaping**: Use `stringData` (not `data`) to avoid base64 escaping issues with special characters like `!`
6. **Timestamp Nulls**: Use `lit(None).cast("timestamp")` not `lit(None)` for Parquet VOID type compatibility
7. **Environment Variables**: Kubernetes does NOT expand shell commands in env var values (e.g., `$(date +%Y-%m-%d)`)

### Out of Scope for Phase 0

- ❌ Hive Metastore (uses canonical S3 paths: `s3a://rbs/bronze/...`)
- ❌ CronJobs (manual execution via `kubectl apply -f job.yaml`)
- ❌ Gold feature generation (deferred to Phase 1)
- ❌ Kubeflow Pipelines (Phase 3)
- ❌ Real-time streaming/CDC (Phase 1)

### Data Flow Validated

```
Django PostgreSQL (stop_events table)
    ↓
Bronze Ingest Job (Spark on k3s, custom image)
    ↓
Contabo S3 (s3a://rbs/bronze/events/date=2024-12-28/*.parquet)
    ↓
Silver Transform Job (Spark on k3s, custom image)
    ↓
Contabo S3 (s3a://rbs/silver/stop_executions/client_id=1/date=2024-12-28/*.parquet)
```

**Test Data**:
- 2 stop_events (STOP_TRIGGERED, EXECUTED) for operation_id=1, symbol=BTCUSDC
- Client: 1
- Transformation: 2 events → 1 materialized stop_execution (latest state)

### Storage Paths (Canonical S3)

**Bronze**:
```
s3a://rbs/bronze/events/date=YYYY-MM-DD/*.parquet
```

**Silver**:
```
s3a://rbs/silver/stop_executions/client_id={id}/date=YYYY-MM-DD/*.parquet
```

### Rollback Procedures

```bash
# Delete Spark jobs
kubectl delete jobs -n analytics-jobs -l spark-app-selector=true

# Delete S3 data (requires aws-cli configured for Contabo endpoint)
aws s3 rm --endpoint-url=https://eu2.contabostorage.com \
  s3://rbs/bronze/ --recursive
aws s3 rm --endpoint-url=https://eu2.contabostorage.com \
  s3://rbs/silver/ --recursive

# Delete namespaces
kubectl delete namespace analytics-jobs datalake-system
```

### Session Details

- **Execution Date**: 2024-12-28
- **Environment**: Production k3s cluster
- **Analytics Node**: jaguar (161.97.147.76, 24GB RAM)
- **Object Storage**: Contabo Object Storage European Union 4333
- **Session Notes**: `docs/sessions/phase0-deep-storage-execution.md`
