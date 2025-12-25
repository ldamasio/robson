# Secrets para Staging

**IMPORTANTE**: Os secrets abaixo são **templates**. Valores reais devem ser criados via kubectl ou sealed-secrets.

## Como criar secrets

### Opção 1: kubectl (Manual)

```bash
# PostgreSQL
kubectl create secret generic postgres-staging \
  --from-literal=POSTGRES_USER=robson_staging \
  --from-literal=POSTGRES_PASSWORD='<GERAR_SENHA_FORTE>' \
  --from-literal=POSTGRES_DB=robson_staging \
  -n staging

# Django
kubectl create secret generic django-staging \
  --from-literal=SECRET_KEY='<GERAR_SECRET_KEY>' \
  --from-literal=DATABASE_URL='postgresql://robson_staging:<SENHA>@postgres-staging:5432/robson_staging' \
  --from-literal=REDIS_URL='redis://redis-staging:6379/0' \
  --from-literal=RABBITMQ_URL='amqp://robson_staging:<SENHA>@rabbitmq-staging:5672' \
  --from-literal=BINANCE_API_KEY='<TESTNET_KEY>' \
  --from-literal=BINANCE_API_SECRET='<TESTNET_SECRET>' \
  -n staging

# Redis
kubectl create secret generic redis-staging \
  --from-literal=REDIS_PASSWORD='<GERAR_SENHA_FORTE>' \
  -n staging

# RabbitMQ
kubectl create secret generic rabbitmq-staging \
  --from-literal=RABBITMQ_DEFAULT_USER=robson_staging \
  --from-literal=RABBITMQ_DEFAULT_PASS='<GERAR_SENHA_FORTE>' \
  -n staging
```

### Opção 2: Sealed Secrets (GitOps, Recomendado)

```bash
# Instalar kubeseal
# https://github.com/bitnami-labs/sealed-secrets

# 1. Criar secret local (NÃO commitar)
kubectl create secret generic postgres-staging \
  --from-literal=POSTGRES_USER=robson_staging \
  --from-literal=POSTGRES_PASSWORD='senha123' \
  --dry-run=client -o yaml > postgres-staging-unsealed.yaml

# 2. Selar secret (pode commitar)
kubeseal -f postgres-staging-unsealed.yaml -w postgres-staging-sealed.yaml

# 3. Aplicar sealed secret
kubectl apply -f postgres-staging-sealed.yaml -n staging

# 4. Sealed secrets controller descriptografa automaticamente
```

## Gerar senhas fortes

```bash
# PostgreSQL password
openssl rand -base64 32

# Django SECRET_KEY
python -c 'from django.core.management.utils import get_random_secret_key; print(get_random_secret_key())'

# Redis password
openssl rand -base64 24

# RabbitMQ password
openssl rand -base64 32
```

## Binance Testnet (Staging)

**NÃO USE API KEYS DE PRODUÇÃO EM STAGING!**

1. Criar conta Binance Testnet: https://testnet.binance.vision/
2. Gerar API Key/Secret no Testnet
3. Usar essas credenciais em `django-staging` secret

---

**CRÍTICO**: Senhas staging devem ser **diferentes** de produção!
