# Deep Storage Phase 0 — Session Checkpoint

**Date**: 2024-12-27
**Session**: Deep Storage Architecture (Phase 0)
**Status**: Planning complete, awaiting cluster deployment
**Next Session**: Start with Contabo S3 bucket creation

---

## Executive Summary

Completed architectural design and scaffolding for Robson Bot Deep Storage Phase 0. All manifests, scripts, and documentation committed to repository. **No changes made to live cluster yet** - this is purely preparation for tomorrow's deployment.

**What Was Done**:
- Designed architecture using Contabo Object Storage + Spark on Kubernetes
- Created ADR-0013 documenting all decisions
- Created operational runbook
- Generated Kubernetes manifests (namespaces, NetworkPolicies, RBAC, jobs)
- Wrote Python pipelines (bronze ingestion, silver transformation)
- Created smoke test script
- Fixed Makefile typo (bronce → bronze)

**What Was NOT Done** (Cluster-Side):
- No kubectl commands executed
- No namespaces created on cluster
- No secrets created
- No S3 bucket verified
- No Spark jobs executed
- No data written to S3

---

## Confirmed Constraints

From user requirements:

1. **Infrastructure**:
   - k3s cluster on 4 Contabo VPS (2×8GB + 2×4GB RAM, 24GB total)
   - NO MinIO (use Contabo Object Storage or alternative)
   - Parquet format for bronze/silver/gold layers
   - Namespace separation with NetworkPolicies

2. **Hadoop's Role**:
   - Hadoop = S3A connector + Spark ecosystem
   - HDFS explicitly deferred (object storage superior)
   - YARN explicitly deferred (k3s replaces resource manager)
   - Hive Metastore deferred to Phase 1 (canonical S3 paths in Phase 0)

3. **Future Hardware** (Context Update During Session):
   - New 16GB dedicated analytics node will be provisioned
   - Label: `robson.io/pool=analytics`
   - Taint: `robson.io/dedicated=analytics:NoSchedule`
   - All Spark jobs must schedule on this node

4. **Scope**:
   - Phase 0: Minimal deep storage (bronze + silver layers only)
   - Phase 1+: Add Hive Metastore, gold layer, data quality, governance

---

## Architecture Decisions Made

### Storage: Contabo Object Storage (S3-Compatible)

**Rationale**:
- Infinite scalability (separates compute from storage)
- Cost-effective (~€0.50/month for 100GB)
- No operational overhead (no DataNodes, no replication)
- 11 9's durability (better than HDFS on 4-node cluster)

**Data Layout**:
```
s3://robson-datalake/
├── bronze/events/date=2024-12-27/part-*.parquet    (Raw, append-only)
├── silver/stop_executions/client_id=1/date=.../    (Cleaned, typed)
└── gold/                                           (Deferred to Phase 1)
```

### Compute: Spark on Kubernetes

**Rationale**:
- k3s already manages resources (no YARN complexity)
- Ephemeral jobs (no long-running services)
- Kubernetes Jobs (not CronJobs in Phase 0)
- Python scripts embedded in Job manifests

**Scheduling**:
- Target dedicated 16GB analytics node (nodeSelector + tolerations)
- Jobs remain Pending until node provisioned
- ResourceQuotas enforce limits (6 CPU / 12GB for analytics-jobs)

### Catalog: Canonical S3 Paths (Phase 0)

**Rationale**:
- Operational simplicity (no Hive Metastore deployment)
- Sufficient for Phase 0 (batch ETL, not ad-hoc queries)
- Deferred SQL ergonomics to Phase 1 (when Hive Metastore added)

**Path Format**:
```
s3a://robson-datalake/bronze/events/date=2024-12-27/
s3a://robson-datalake/silver/stop_executions/client_id=1/date=2024-12-27/
```

### Security: Default-Deny NetworkPolicies

**Model**: Whitelist only required traffic
- DNS (CoreDNS)
- Kubernetes API (Spark driver creates executor pods)
- S3 endpoint (Contabo Object Storage)
- Django database (PostgreSQL in robson-prod namespace)
- Intra-namespace communication (Spark driver ↔ executors)

**Isolation**: analytics-jobs blocked from staging namespace (production isolation)

---

## What Exists in Git (Committed)

### Documentation
1. `docs/adr/ADR-0013-deep-storage-architecture.md` - Architecture decision record
2. `docs/runbooks/deep-storage.md` - Operational procedures
3. `data/schemas/README.md` - JSON Schema definitions
4. `data/contracts/README.md` - Data contracts
5. `data/datasets/README.md` - Dataset directory structure

### Infrastructure Manifests
1. `infra/k8s/datalake/namespaces/datalake-system.yml` - Namespace + ResourceQuota (2 CPU / 4GB)
2. `infra/k8s/datalake/namespaces/analytics-jobs.yml` - Namespace + ResourceQuota (6 CPU / 12GB)
3. `infra/k8s/datalake/network-policies/datalake-system.yml` - 6 policies (default-deny, DNS, S3, PostgreSQL)
4. `infra/k8s/datalake/network-policies/analytics-jobs.yml` - 5 policies (includes k8s API access)
5. `infra/k8s/datalake/rbac/spark-rbac.yml` - ServiceAccount + Role + RoleBinding
6. `infra/k8s/datalake/secrets/README.md` - Secret creation templates
7. `infra/k8s/datalake/jobs/bronze-ingest-job.yml` - REAL Job (Django → S3 Parquet)
8. `infra/k8s/datalake/jobs/silver-transform-job.yml` - REAL Job (Bronze → Silver Parquet)

### Python Pipelines
1. `data/pipelines/bronze_ingest.py` - Reads Django Outbox, writes Parquet to S3
2. `data/pipelines/silver_transform.py` - Reads bronze Parquet, writes silver Parquet

### Scripts
1. `data/scripts/smoke-test.sh` - Validates namespaces, NetworkPolicies, S3 connectivity

### Build Configuration
1. `Makefile` - Fixed typo: datalake-run-bronce → datalake-run-bronze

**Total**: 13 files created/modified in this session.

---

## What Exists Only as a Plan (Not Yet Realized)

### Cluster Resources (Not Created)
- Namespaces `datalake-system`, `analytics-jobs` do not exist on k3s cluster
- NetworkPolicies not applied
- ServiceAccount `spark-jobs` not created
- ResourceQuotas not enforced
- No secrets (contabo-s3-credentials, django-db-credentials)

### External Resources (Not Verified)
- Contabo S3 bucket `robson-datalake` may not exist (verify tomorrow)
- Access keys not generated
- AWS CLI not configured with Contabo endpoint

### Jobs (Never Executed)
- Bronze ingestion job never ran
- Silver transformation job never ran
- No Parquet data in S3
- No validation of end-to-end flow

### Analytics Node (Not Provisioned)
- 16GB VPS not ordered yet
- No label `robson.io/pool=analytics` applied to any node
- No taint `robson.io/dedicated=analytics:NoSchedule` on cluster
- Jobs will remain Pending until node added

---

## DO NOT Repeat (Architectural Decisions Finalized)

These decisions are FINAL. Do NOT revisit in next session:

1. **NO HDFS** - Object storage is superior for this scale. HDFS requires DataNodes (memory overhead), replication factor (wastes disk), NameNode HA (complexity). **Contabo Object Storage replaces HDFS.**

2. **NO MinIO** - User explicitly rejected MinIO. Contabo provides same S3-compatible capability without self-hosting overhead. **Use Contabo Object Storage.**

3. **NO YARN** - k3s already manages resources via resource limits/quotas. YARN adds unnecessary layer. **Use Kubernetes scheduler.**

4. **NO Hive Metastore in Phase 0** - Deferring to Phase 1 for operational simplicity. Phase 0 uses canonical S3 paths (`s3a://robson-datalake/...`). Phase 1 adds Hive Metastore for SQL ergonomics.

5. **NO Spark Operator** - Using Kubernetes Jobs (simpler for Phase 0). Can migrate to Spark Operator in Phase 2 if needed (job orchestration complexity).

6. **NO HDFS DataNodes** - Object storage is the deep storage layer. Spark reads/writes S3 directly via S3A connector.

7. **NO Streaming in Phase 0** - Batch polling from Django Outbox (hourly). Phase 1 adds CDC/streaming.

---

## Tomorrow Start Point (Copy-Paste Commands)

### Preconditions to Check

1. **Verify Contabo S3 bucket exists**:
   ```bash
   # Install AWS CLI v2
   apt install awscli  # or brew install awscli

   # Configure with Contabo credentials
   aws configure --profile contabo
   # Enter Access Key ID, Secret Access Key
   # Region: eu-central-2
   # Output format: json

   # Test connectivity
   aws s3 ls --profile contabo --endpoint-url https://s3.eu-central-2.contabo.com

   # Create bucket if not exists
   aws s3 mb s3://robson-datalake \
       --profile contabo \
       --region eu-central-2 \
       --endpoint-url https://s3.eu-central-2.contabo.com
   ```

2. **Verify k3s cluster access**:
   ```bash
   kubectl cluster-info
   kubectl get nodes
   ```

3. **Verify analytics node exists** (IF provisioned):
   ```bash
   kubectl get nodes -l robson.io/pool=analytics
   kubectl describe node <analytics-node> | grep -A 5 "Labels:"
   kubectl describe node <analytics-node> | grep -A 5 "Taints:"
   ```

### Exact Commands to Deploy (In Order)

```bash
# 1. Create Kubernetes secrets
kubectl create secret generic contabo-s3-credentials \
    -n datalake-system \
    --from-literal=AWS_ACCESS_KEY_ID=<your-access-key> \
    --from-literal=AWS_SECRET_ACCESS_KEY=<your-secret-key> \
    --from-literal=AWS_ENDPOINT=s3.eu-central-2.contabo.com \
    --from-literal=AWS_REGION=eu-central-2 \
    --from-literal=S3_BUCKET=robson-datalake

kubectl create secret generic django-db-credentials \
    -n analytics-jobs \
    --from-literal=DJANGO_DB_HOST=postgres.robson-prod.svc.cluster.local \
    --from-literal=DJANGO_DB_NAME=robson \
    --from-literal=DJANGO_DB_USER=robson \
    --from-literal=DJANGO_DB_PASSWORD=<from-prod-secret>

# 2. Apply namespaces
kubectl apply -f infra/k8s/datalake/namespaces/

# 3. Apply RBAC
kubectl apply -f infra/k8s/datalake/rbac/

# 4. Apply NetworkPolicies
kubectl apply -f infra/k8s/datalake/network-policies/

# 5. Verify deployment
kubectl get ns datalake-system analytics-jobs
kubectl get networkpolicy -n datalake-system
kubectl get networkpolicy -n analytics-jobs
kubectl get sa spark-jobs -n analytics-jobs
kubectl describe resourcequota datalake-system-quota -n datalake-system
kubectl describe resourcequota analytics-jobs-quota -n analytics-jobs

# 6. Run smoke test
./data/scripts/smoke-test.sh

# Expected output: All tests PASS
```

### First Real Run (After Smoke Test Passes)

```bash
# 1. Run bronze ingestion
kubectl apply -f infra/k8s/datalake/jobs/bronze-ingest-job.yml

# 2. Watch logs
kubectl logs -f job/bronze-ingest -n analytics-jobs

# 3. Verify S3 output
aws s3 ls s3://robson-datalake/bronze/events/ --recursive --endpoint-url=https://s3.eu-central-2.contabo.com

# 4. Run silver transformation
kubectl apply -f infra/k8s/datalake/jobs/silver-transform-job.yml

# 5. Watch logs
kubectl logs -f job/silver-transform -n analytics-jobs

# 6. Verify S3 output
aws s3 ls s3://robson-datalake/silver/stop_executions/ --recursive --endpoint-url=https://s3.eu-central-2.contabo.com
```

---

## Stop Conditions (When to Pause)

**IF any of these occur, PAUSE and debug before proceeding**:

1. **Smoke Test FAILS**:
   - ❌ Namespaces not created → Check kubectl context
   - ❌ NetworkPolicies missing → Check file paths
   - ❌ Secrets not found → Create secrets first
   - ❌ S3 connectivity fails → Check credentials, endpoint, network

2. **Job Stuck in PENDING**:
   - Check events: `kubectl describe job/bronze-ingest -n analytics-jobs`
   - Check node labels: `kubectl get nodes -L robson.io/pool`
   - Check taints: `kubectl describe nodes | grep -A 2 "Taints"`
   - Verify analytics node exists (if not, jobs will Pending until node added)

3. **Job FAILS**:
   - Check logs: `kubectl logs -n analytics-jobs <driver-pod>`
   - Check S3 credentials (401/403 errors)
   - Check Django DB connectivity (connection refused)
   - Check NetworkPolicies (timeout errors)

4. **No Data in S3**:
   - Verify job completed successfully (`kubectl get jobs -n analytics-jobs`)
   - Check S3 bucket permissions
   - Check Parquet file write errors in logs

---

## Expected Outcomes (If Everything Works)

**Successful Deployment**:
- ✅ Namespaces exist with ResourceQuotas enforced
- ✅ NetworkPolicies block all traffic except explicit allows
- ✅ Secrets created (contabo-s3-credentials, django-db-credentials)
- ✅ RBAC allows Spark driver to create executor pods
- ✅ Smoke test passes all checks

**Successful First Run**:
- ✅ Bronze job completes (writes Parquet to S3)
- ✅ Silver job completes (reads bronze, writes silver to S3)
- ✅ S3 contains valid Parquet files
- ✅ Jobs scheduled on analytics node (if provisioned)
- ✅ Resource usage within quotas

**Validation**:
- ✅ Parquet files readable via Spark shell
- ✅ Data quality checks pass (null counts, duplicates)
- ✅ Summary statistics logged (status distribution, slippage metrics)

---

## Notes for Tomorrow

1. **Start at the beginning**: Verify S3 bucket first, then secrets, then cluster deployment.

2. **Do NOT skip smoke test**: It validates prerequisites before running real jobs.

3. **Jobs require analytics node**: If 16GB node not provisioned, jobs will Pending indefinitely. Either provision node OR reduce job resource requests to fit on existing 8GB nodes (not recommended).

4. **S3 credentials are critical**: Double-check Contabo access keys before creating secrets. Typos cause 403 errors.

5. **NetworkPolicies are strict**: If jobs hang, check egress rules (DNS, k8s API, S3, PostgreSQL).

6. **Read the logs**: Spark logs are verbose. Look for ERROR/WARNING lines. Key sections:
   - "Creating Spark session..."
   - "Read X rows from bronze layer"
   - "Written to s3a://..."
   - "✅ ... job completed successfully"

---

## Session Handoff

**Where Tomorrow's Work Should Resume**:
1. Open `docs/runbooks/deep-storage.md` → "TOMORROW START POINT" section
2. Follow "Exact Commands to Deploy" sequentially
3. Run smoke test after each step
4. Debug if smoke test fails
5. Proceed to first real run (bronze job) only after smoke test passes

**Key Files to Reference**:
- Architecture: `docs/adr/ADR-0013-deep-storage-architecture.md`
- Operations: `docs/runbooks/deep-storage.md`
- Session checkpoint: `docs/sessions/2024-12-27-deep-storage-phase0.md` (this file)

**Expected Session Duration**: 1-2 hours for deployment + first run, assuming S3 bucket already exists.

---

**End of Session Checkpoint**
**Last Updated**: 2024-12-27
**Next Session**: 2024-12-28 (or whenever you resume)
