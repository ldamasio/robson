# Deploy Staging - Passo a Passo

**Ambiente**: Staging (Isolado de produção)
**Namespace**: `staging`
**Data**: 2024-12-25

---

## Pré-requisitos

- [x] DNS criados e propagados (verificar com `dig staging.rbx.ia.br`)
- [x] kubectl configurado para acessar cluster k3s
- [x] Acesso SSH ao servidor: `ssh root@158.220.116.31`
- [ ] Secrets criados (ver seção "Criar Secrets" abaixo)

---

## Passo 1: Verificar DNS Propagou

```bash
# Verificar se DNS está resolvendo
dig staging.rbx.ia.br +short
dig api.staging.rbx.ia.br +short
dig criticws.staging.rbx.ia.br +short

# Esperado: 158.220.116.31 em todos

# Se não resolver, aguardar propagação (pode levar até 60 minutos)
```

---

## Passo 2: Conectar ao Cluster

```bash
# SSH no servidor
ssh root@158.220.116.31

# Verificar cluster
kubectl get nodes
kubectl get namespaces

# Verificar Istio instalado
kubectl get pods -n istio-system
```

---

## Passo 3: Criar Secrets

**IMPORTANTE**: Usar senhas DIFERENTES de produção!

```bash
# 1. PostgreSQL Secret
kubectl create secret generic postgres-staging \
  --from-literal=POSTGRES_USER=robson_staging \
  --from-literal=POSTGRES_PASSWORD="$(openssl rand -base64 32)" \
  --from-literal=POSTGRES_DB=robson_staging \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# 2. Redis Secret
kubectl create secret generic redis-staging \
  --from-literal=REDIS_PASSWORD="$(openssl rand -base64 24)" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# 3. RabbitMQ Secret
kubectl create secret generic rabbitmq-staging \
  --from-literal=RABBITMQ_DEFAULT_USER=robson_staging \
  --from-literal=RABBITMQ_DEFAULT_PASS="$(openssl rand -base64 32)" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# 4. Django Secret (IMPORTANTE: Binance TESTNET)
# PRIMEIRO: Obtenha as senhas criadas acima
POSTGRES_PASS=$(kubectl get secret postgres-staging -n staging -o jsonpath='{.data.POSTGRES_PASSWORD}' | base64 -d)
REDIS_PASS=$(kubectl get secret redis-staging -n staging -o jsonpath='{.data.REDIS_PASSWORD}' | base64 -d)
RABBITMQ_PASS=$(kubectl get secret rabbitmq-staging -n staging -o jsonpath='{.data.RABBITMQ_DEFAULT_PASS}' | base64 -d)

# DEPOIS: Crie Django secret
kubectl create secret generic django-staging \
  --from-literal=SECRET_KEY="$(python3 -c 'from django.core.management.utils import get_random_secret_key; print(get_random_secret_key())')" \
  --from-literal=DATABASE_URL="postgresql://robson_staging:${POSTGRES_PASS}@postgres-staging:5432/robson_staging" \
  --from-literal=REDIS_URL="redis://:${REDIS_PASS}@redis-staging:6379/0" \
  --from-literal=RABBITMQ_URL="amqp://robson_staging:${RABBITMQ_PASS}@rabbitmq-staging:5672" \
  --from-literal=BINANCE_API_KEY="<TESTNET_KEY_AQUI>" \
  --from-literal=BINANCE_API_SECRET="<TESTNET_SECRET_AQUI>" \
  -n staging --dry-run=client -o yaml | kubectl apply -f -

# Verificar secrets criados
kubectl get secrets -n staging
```

### Como obter Binance Testnet Keys

1. Acessar: https://testnet.binance.vision/
2. Conectar com Google/GitHub
3. Gerar API Key/Secret
4. Copiar e usar nos comandos acima

---

## Passo 4: Aplicar Manifestos

```bash
# Aplicar todos os manifestos (em ordem)
kubectl apply -k infra/k8s/staging/

# OU aplicar manualmente em ordem:

# 1. Namespace
kubectl apply -f infra/k8s/namespaces/staging.yml

# 2. Network Policies
kubectl apply -f infra/k8s/staging/network-policies/isolation.yml

# 3. PostgreSQL
kubectl apply -f infra/k8s/staging/postgres/postgres-staging.yml

# 4. Redis
kubectl apply -f infra/k8s/staging/redis/redis-staging.yml

# 5. RabbitMQ
kubectl apply -f infra/k8s/staging/rabbitmq/rabbitmq-staging.yml

# 6. Backend
kubectl apply -f infra/k8s/staging/backend/backend-staging.yml

# 7. CronJob Stop Monitor
kubectl apply -f infra/k8s/staging/backend/cronjob-stop-monitor.yml

# 8. Istio Gateway
kubectl apply -f infra/k8s/staging/istio/gateway-staging.yml

# 9. TLS Certificate
kubectl apply -f infra/k8s/staging/istio/certificate-staging.yml
```

---

## Passo 5: Verificar Deployment

```bash
# Verificar pods
kubectl get pods -n staging

# Esperado:
# postgres-staging-xxxxx           1/1     Running
# redis-staging-xxxxx              1/1     Running
# rabbitmq-staging-xxxxx           1/1     Running
# backend-staging-xxxxx            1/1     Running
# backend-staging-yyyyy            1/1     Running

# Verificar logs
kubectl logs -f deployment/backend-staging -n staging

# Verificar services
kubectl get svc -n staging

# Verificar PVCs
kubectl get pvc -n staging
```

---

## Passo 6: Aguardar PostgreSQL Iniciar

```bash
# Aguardar PostgreSQL estar pronto
kubectl wait --for=condition=ready pod -l app=postgres-staging -n staging --timeout=300s

# Verificar PostgreSQL
kubectl exec -it deployment/postgres-staging -n staging -- psql -U robson_staging -d robson_staging -c "SELECT version();"
```

---

## Passo 7: Aplicar Migrations

```bash
# Exec no pod backend
POD=$(kubectl get pods -n staging -l app=backend-staging -o jsonpath='{.items[0].metadata.name}')

# Aplicar migrations (0015-0018 + anteriores)
kubectl exec -it $POD -n staging -- python manage.py migrate

# Verificar migrations aplicadas
kubectl exec -it $POD -n staging -- python manage.py showmigrations api | tail -10

# Esperado:
# [X] 0015_event_sourcing_stop_monitor
# [X] 0016_add_stop_price_columns
# [X] 0017_set_stop_check_default
# [X] 0018_create_stop_indexes_concurrent
```

---

## Passo 8: Executar Backfill

```bash
# Dry-run primeiro
kubectl exec -it $POD -n staging -- python manage.py backfill_stop_price --dry-run

# Se OK, executar de verdade
kubectl exec -it $POD -n staging -- python manage.py backfill_stop_price

# Verificar resultado
kubectl exec -it $POD -n staging -- python manage.py dbshell
# SQL:
SELECT COUNT(*) FROM api_operation WHERE stop_price IS NOT NULL;
```

---

## Passo 9: Emitir Certificado TLS

```bash
# Verificar cert-manager
kubectl get pods -n cert-manager

# Verificar Certificate
kubectl get certificate -n staging

# Verificar status
kubectl describe certificate staging-rbx-ia-br-tls -n staging

# Aguardar emissão (pode levar 2-5 minutos)
kubectl wait --for=condition=ready certificate/staging-rbx-ia-br-tls -n staging --timeout=300s

# Verificar secret TLS criado
kubectl get secret staging-rbx-ia-br-tls -n staging
```

---

## Passo 10: Smoke Tests

```bash
# 1. Health check (interno)
kubectl exec -it $POD -n staging -- curl -I http://localhost:8000/health/

# 2. Health check (via Istio Gateway)
curl -I https://api.staging.rbx.ia.br/health/

# Esperado: HTTP/2 200

# 3. Admin check
curl https://api.staging.rbx.ia.br/admin/ -L

# Esperado: Redirect to login page

# 4. Verificar backend logs
kubectl logs -f deployment/backend-staging -n staging --tail=50
```

---

## Passo 11: Ativar CronJob Monitor

```bash
# Verificar CronJob criado
kubectl get cronjob -n staging

# Triggerar job manualmente (teste)
kubectl create job --from=cronjob/stop-monitor-staging test-monitor -n staging

# Ver logs do job
kubectl logs -f job/test-monitor -n staging

# Esperado:
# ✓ No triggers (se não houver operações ativas)
# ou
# 🛑 STOP_LOSS: Op#123 BTCUSDC @ 88000.00 (se houver trigger)
```

---

## Passo 12: Criar Operação de Teste (Opcional)

```bash
# Exec Django shell
kubectl exec -it $POD -n staging -- python manage.py shell

# Python:
from api.models import Operation, Symbol, Strategy, Client, Order
from decimal import Decimal

client = Client.objects.first()
symbol = Symbol.objects.filter(name="BTCUSDC").first()
strategy = Strategy.objects.first()

# Criar operação com stop_price
operation = Operation.objects.create(
    client=client,
    symbol=symbol,
    strategy=strategy,
    side="BUY",
    status="ACTIVE",
    stop_price=Decimal("88200.00"),
    target_price=Decimal("93600.00"),
)

# Criar ordem de entrada
entry_order = Order.objects.create(
    client=client,
    symbol=symbol,
    strategy=strategy,
    side="BUY",
    order_type="MARKET",
    quantity=Decimal("0.001"),
    filled_quantity=Decimal("0.001"),
    avg_fill_price=Decimal("90000.00"),
    status="FILLED",
)

operation.entry_orders.add(entry_order)

print(f"✅ Operation #{operation.id} created with stop at {operation.stop_price}")
exit()

# Rodar monitor manualmente
kubectl exec -it $POD -n staging -- python manage.py monitor_stops --dry-run
```

---

## Passo 13: Verificar Event Sourcing

```bash
# Django shell
kubectl exec -it $POD -n staging -- python manage.py shell

# Python:
from api.models.event_sourcing import StopEvent, StopExecution

print(f"Total events: {StopEvent.objects.count()}")
print(f"Total executions: {StopExecution.objects.count()}")

# Ver últimos eventos
for event in StopEvent.objects.order_by('-event_seq')[:10]:
    print(f"{event.event_seq}: {event.event_type} - Op#{event.operation_id}")
```

---

## Troubleshooting

### Pods não iniciam

```bash
# Ver eventos
kubectl get events -n staging --sort-by='.lastTimestamp'

# Describe pod
kubectl describe pod <pod-name> -n staging

# Ver logs
kubectl logs <pod-name> -n staging
```

### PostgreSQL não conecta

```bash
# Verificar secret
kubectl get secret postgres-staging -n staging -o yaml

# Testar conexão
kubectl exec -it deployment/postgres-staging -n staging -- psql -U robson_staging -d robson_staging -c "SELECT 1;"
```

### Certificado TLS não emite

```bash
# Ver challenges
kubectl get challenges -n staging

# Ver order
kubectl get order -n staging

# Describe certificate
kubectl describe certificate staging-rbx-ia-br-tls -n staging

# Verificar cert-manager logs
kubectl logs -f deployment/cert-manager -n cert-manager
```

### Backend não inicia

```bash
# Ver logs
kubectl logs deployment/backend-staging -n staging --tail=100

# Verificar variáveis de ambiente
kubectl exec -it deployment/backend-staging -n staging -- env | grep -E "(DATABASE|REDIS|RABBITMQ)"

# Testar conexões
kubectl exec -it deployment/backend-staging -n staging -- python manage.py dbshell
```

---

## Rollback

```bash
# Deletar tudo (se necessário recomeçar)
kubectl delete namespace staging

# Deletar recursos individuais
kubectl delete -k infra/k8s/staging/
```

---

## Próximos Passos

1. ✅ Deploy staging concluído
2. ✅ Migrations aplicadas
3. ✅ Backfill executado
4. ✅ TLS emitido
5. ⏳ Testes manuais (criar operações, testar stop monitor)
6. ⏳ Testes de integração (Playwright, pytest)
7. ⏳ Testes de carga (k6, ab)
8. ⏳ Deploy frontend staging
9. ⏳ Configurar CI/CD (GitHub Actions)
10. ⏳ Deploy production (após validação)

---

**Última Atualização**: 2024-12-25
**Status**: 📋 Pronto para deployment
