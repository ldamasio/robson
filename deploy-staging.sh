#!/bin/bash
# Deploy Staging - Automated Script
# Run on k3s server: bash deploy-staging.sh

set -e  # Exit on error

echo "🚀 Starting Staging Deployment..."

# Step 1: Create namespace
echo "📦 Creating namespace staging..."
kubectl apply -f infra/k8s/namespaces/staging.yaml

# Step 2: Create secrets
echo "🔐 Creating secrets..."

# PostgreSQL
kubectl create secret generic postgres-staging \
  --from-literal=POSTGRES_USER=robson_staging \
  --from-literal=POSTGRES_PASSWORD="$(openssl rand -base64 32)" \
  --from-literal=POSTGRES_DB=robson_staging \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# Redis
kubectl create secret generic redis-staging \
  --from-literal=REDIS_PASSWORD="$(openssl rand -base64 24)" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# RabbitMQ
kubectl create secret generic rabbitmq-staging \
  --from-literal=RABBITMQ_DEFAULT_USER=robson_staging \
  --from-literal=RABBITMQ_DEFAULT_PASS="$(openssl rand -base64 32)" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# Get passwords
POSTGRES_PASS=$(kubectl get secret postgres-staging -n staging -o jsonpath='{.data.POSTGRES_PASSWORD}' | base64 -d)
REDIS_PASS=$(kubectl get secret redis-staging -n staging -o jsonpath='{.data.REDIS_PASSWORD}' | base64 -d)
RABBITMQ_PASS=$(kubectl get secret rabbitmq-staging -n staging -o jsonpath='{.data.RABBITMQ_DEFAULT_PASS}' | base64 -d)

# Django
kubectl create secret generic django-staging \
  --from-literal=SECRET_KEY="django-insecure-staging-$(openssl rand -base64 32)" \
  --from-literal=DATABASE_URL="postgresql://robson_staging:${POSTGRES_PASS}@postgres-staging:5432/robson_staging" \
  --from-literal=REDIS_URL="redis://:${REDIS_PASS}@redis-staging:6379/0" \
  --from-literal=RABBITMQ_URL="amqp://robson_staging:${RABBITMQ_PASS}@rabbitmq-staging:5672" \
  --from-literal=BINANCE_API_KEY="testnet-placeholder" \
  --from-literal=BINANCE_API_SECRET="testnet-placeholder" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

echo "✅ Secrets created"

# Step 3: Apply all manifests
echo "📋 Applying Kubernetes manifests..."
kubectl apply -k infra/k8s/staging/

echo "⏳ Waiting for pods to start..."
sleep 10

# Step 4: Check pods
echo "🔍 Checking pods status..."
kubectl get pods -n staging

echo ""
echo "✅ Deployment initiated!"
echo ""
echo "Next steps:"
echo "1. Wait for pods: watch kubectl get pods -n staging"
echo "2. Apply migrations: kubectl exec -it deployment/backend-staging -n staging -- python manage.py migrate"
echo "3. Run backfill: kubectl exec -it deployment/backend-staging -n staging -- python manage.py backfill_stop_price"
