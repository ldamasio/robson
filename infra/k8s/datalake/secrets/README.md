# Secret Templates for Deep Storage Phase 0

**Create these secrets BEFORE deploying Spark jobs.**

---

## Secret 1: Contabo S3 Credentials

**Purpose**: Access to Contabo Object Storage (S3-compatible)

**Create in both namespaces**:

```bash
# Create in datalake-system (for Hive Metastore backups, Phase 1+)
kubectl create secret generic contabo-s3-credentials \
  -n datalake-system \
  --from-literal=AWS_ACCESS_KEY_ID=<your-access-key> \
  --from-literal=AWS_SECRET_ACCESS_KEY=<your-secret-key> \
  --from-literal=AWS_ENDPOINT=s3.eu-central-2.contabo.com \
  --from-literal=AWS_REGION=eu-central-2 \
  --from-literal=S3_BUCKET=robson-datalake

# Create in analytics-jobs (for Spark jobs)
kubectl create secret generic contabo-s3-credentials \
  -n analytics-jobs \
  --from-literal=AWS_ACCESS_KEY_ID=<your-access-key> \
  --from-literal=AWS_SECRET_ACCESS_KEY=<your-secret-key> \
  --from-literal=AWS_ENDPOINT=s3.eu-central-2.contabo.com \
  --from-literal=AWS_REGION=eu-central-2 \
  --from-literal=S3_BUCKET=robson-datalake
```

**Verify**:
```bash
kubectl get secret contabo-s3-credentials -n datalake-system -o yaml
kubectl get secret contabo-s3-credentials -n analytics-jobs -o yaml
```

**How to get credentials**:
1. Login to Contabo panel
2. Navigate to Object Storage
3. Create bucket `robson-datalake`
4. Generate S3 credentials (Access Key ID + Secret Access Key)

---

## Secret 2: Django DB Credentials

**Purpose**: Bronze ingestion reads Django Outbox from PostgreSQL

**Create in analytics-jobs namespace**:

```bash
# Get existing credentials from robson-prod namespace
kubectl get secret rbs-postgres-prod-secret -n robson -o yaml > /tmp/postgres-secret.yaml

# Extract password
POSTGRES_PASSWORD=$(kubectl get secret rbs-postgres-prod-secret -n robson -o jsonpath='{.data.password}' | base64 -d)

# Create secret in analytics-jobs
kubectl create secret generic django-db-credentials \
  -n analytics-jobs \
  --from-literal=DJANGO_DB_HOST=postgres.robson.svc.cluster.local \
  --from-literal=DJANGO_DB_NAME=robson \
  --from-literal=DJANGO_DB_USER=robson \
  --from-literal=DJANGO_DB_PASSWORD="$POSTGRES_PASSWORD"
```

**Verify**:
```bash
kubectl get secret django-db-credentials -n analytics-jobs -o yaml
kubectl get secret django-db-credentials -n analytics-jobs -o jsonpath='{.data.DJANGO_DB_PASSWORD}' | base64 -d
```

---

## Alternative: Sealed Secrets (GitOps-Friendly)

If using Sealed Secrets for GitOps:

1. Install Sealed Secrets controller (one-time):
```bash
kubectl apply -f https://github.com/bitnami-labs/sealed-secrets/releases/download/v0.24.0/controller.yaml
```

2. Create sealed secret template:

```bash
# Create sealed secret for Contabo S3
kubectl create secret generic contabo-s3-credentials \
  --dry-run=client \
  -n analytics-jobs \
  --from-literal=AWS_ACCESS_KEY_ID=<your-access-key> \
  --from-literal=AWS_SECRET_ACCESS_KEY=<your-secret-key> \
  --from-literal=AWS_ENDPOINT=s3.eu-central-2.contabo.com \
  --from-literal=AWS_REGION=eu-central-2 \
  --from-literal=S3_BUCKET=robson-datalake \
  -o yaml | kubeseal -o yaml > contabo-s3-credentials-sealed.yaml

# Commit to repo
git add contabo-s3-credentials-sealed.yaml
git commit -m "feat: add sealed secret for Contabo S3"
```

---

## Validation

After creating secrets, validate they work:

```bash
# Test S3 connectivity from cluster
kubectl run s3-test --image=amazon/aws-cli:latest --rm -it --restart=Never -n analytics-jobs -- \
  aws s3 ls s3://robson-datalake \
  --endpoint-url=https://s3.eu-central-2.contabo.com \
  --no-verify-ssl

# Should list: (empty or existing data)
```

---

**Last Updated**: 2024-12-27
**Related**: docs/runbooks/deep-storage.md, ADR-0013
