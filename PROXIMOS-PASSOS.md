# Próximos Passos - Production Operations

**Status Atual**: ✅ Features implemented, pending VPS deployment
**Last Updated**: 2025-12-29

---

## 🚨 IMMEDIATE ACTION REQUIRED (VPS Commands)

### Step 1: Create Groq Secret

```bash
kubectl create secret generic rbs-groq-secret \
  --from-literal=GROQ_API_KEY=<YOUR_GROQ_API_KEY> \
  -n robson \
  --dry-run=client -o yaml | kubectl apply -f -
```

### Step 2: Apply CronJobs

```bash
cd /path/to/robson
git pull origin main
kubectl apply -f infra/k8s/prod/rbs-stop-monitor-cronjob.yml
kubectl apply -f infra/k8s/prod/rbs-trailing-stop-cronjob.yml
kubectl get cronjobs -n robson
```

### Step 3: Verify CronJobs Are Running

```bash
kubectl logs -n robson -l app=rbs-stop-monitor --tail=20
kubectl logs -n robson -l app=rbs-trailing-stop --tail=20
```

---

## Session Summary: 2025-12-29

### ✅ Completed

1. **Closed BTC Position with Profit**
   - Entry: $87,193.34 → Exit: ~$90,058
   - Profit: +3.28% (with 3x leverage = ~+9.9%)

2. **Removed Dry-Run from Stop Monitor**
   - File: `infra/k8s/prod/rbs-stop-monitor-cronjob.yml`
   - CronJob now executes real stop orders

3. **Created Trailing Stop CronJob**
   - File: `infra/k8s/prod/rbs-trailing-stop-cronjob.yml`
   - Implements Hand-Span trailing stop algorithm

4. **Implemented AI Chat (Robson AI)**
   - Backend: Groq adapter, use case, views
   - Frontend: Floating chat component
   - Endpoints: `/api/chat/`, `/api/chat/status/`, `/api/chat/context/`

### ⏳ Pending Deployment

- [ ] Create Groq secret in Kubernetes
- [ ] Apply updated stop-monitor cronjob
- [ ] Apply new trailing-stop cronjob
- [ ] Update backend deployment with GROQ_API_KEY env var
- [ ] Rebuild and deploy Docker image with `groq` dependency

---

## Previous: Event Sourcing Stop Monitor (2024-12-25)

---

## Resumo do que foi implementado

✅ **Backend completo** para Stop-Loss Monitor com Event Sourcing:

- Migrations production-safe (zero downtime)
- Idempotência via execution_token (previne duplicações)
- Stop-loss por preço absoluto (não recalcula de porcentagem)
- Event sourcing completo (audit trail imutável)
- Testes abrangentes (12 casos de teste)

---

## Passo 1: Preparar Ambiente

### Opção A: Ambiente Local (venv)

```bash
# Criar ambiente virtual (se não existir)
cd apps/backend/monolith
python -m venv venv

# Ativar ambiente virtual
source venv/bin/activate  # Linux/Mac
# ou
venv\Scripts\activate  # Windows

# Instalar dependências
pip install -r requirements.txt

# Verificar instalação
python manage.py --version
```

### Opção B: Docker (Recomendado)

```bash
# Subir containers
docker compose up -d

# Verificar status
docker compose ps

# Acessar container backend
docker compose exec backend bash
```

---

## Passo 2: Aplicar Migrations (Zero Downtime)

### ⚠️ IMPORTANTE: Verificar PostgreSQL Version

```bash
# Verificar versão do PostgreSQL (deve ser >= 11)
docker compose exec db psql -U postgres -c "SELECT version();"
# ou
psql -c "SELECT version();"
```

**Requisito**: PostgreSQL 11+ (para DEFAULT metadata-only)

### Aplicar Migrations

```bash
# 1. Ver status atual
python manage.py showmigrations api

# 2. Aplicar migrations (4 passos automáticos)
python manage.py migrate api

# Esperado:
# [X] 0015_event_sourcing_stop_monitor (~1 segundo)
# [X] 0016_add_stop_price_columns (~1 segundo)
# [X] 0017_set_stop_check_default (~1 segundo)
# [X] 0018_create_stop_indexes_concurrent (alguns minutos, NON-BLOCKING)

# 3. Verificar migrations aplicadas
python manage.py showmigrations api | grep -E "(0015|0016|0017|0018)"
```

### Monitorar Progresso dos Índices (Migration 0018)

```bash
# Em outro terminal, monitorar criação de índices
watch -n 2 "psql -U postgres -d robson -c \"SELECT indexname, pg_size_pretty(pg_relation_size(indexrelid)) as size FROM pg_stat_progress_create_index JOIN pg_stat_all_indexes USING (relid, indexrelid);\""

# Ou verificar manualmente
psql -U postgres -d robson -c "SELECT indexname FROM pg_indexes WHERE tablename = 'operation' AND indexname LIKE 'idx_operation_%';"
```

---

## Passo 3: Backfill stop_price (Operações Existentes)

### 3.1 Dry-Run (Ver o que será atualizado)

```bash
python manage.py backfill_stop_price --dry-run
```

**Esperado**:

```
🔄 Starting stop_price backfill...
   DRY RUN MODE (no changes will be made)
   Batch size: 1000

📊 Found 50 operations to backfill

Processing batch 1 (1-50 of 50)...
  Op#1: stop_price=88200.00 (from 2.0%)
  Op#2: stop_price=45000.00 (from 1.5%)
  ...
  ✅ Batch complete: 50 updated

========================================
✅ Backfill complete!
   Total operations processed: 50
   Successfully updated: 50
   DRY RUN: No changes were made
```

### 3.2 Executar Backfill Real

```bash
# Executar backfill (batched, resumable)
python manage.py backfill_stop_price --batch-size 1000

# Em caso de erro, pode re-executar (é idempotente)
# Operações já backfilled serão puladas
```

### 3.3 Verificar Resultado

```bash
# Via Django shell
python manage.py shell

from api.models import Operation
from decimal import Decimal

# Verificar operações com stop_price
total = Operation.objects.count()
with_stop = Operation.objects.filter(stop_price__isnull=False).count()
without_stop = Operation.objects.filter(stop_price__isnull=True).count()

print(f"Total operations: {total}")
print(f"With stop_price: {with_stop}")
print(f"Without stop_price: {without_stop}")

# Verificar cálculo correto (exemplo)
op = Operation.objects.filter(stop_price__isnull=False, stop_loss_percent__isnull=False).first()
if op:
    expected = op.average_entry_price * (Decimal('1') - op.stop_loss_percent / Decimal('100'))
    print(f"\nOperation #{op.id}:")
    print(f"  Entry: {op.average_entry_price}")
    print(f"  Stop %: {op.stop_loss_percent}%")
    print(f"  Stop Price: {op.stop_price}")
    print(f"  Expected: {expected}")
    print(f"  Match: {op.stop_price == expected}")
```

---

## Passo 4: Rodar Testes

### 4.1 Testes de Event Sourcing

```bash
# Rodar todos os testes de event sourcing
cd apps/backend/monolith
pytest api/tests/test_event_sourcing_stop_monitor.py -v

# Esperado: 12 testes passando
# test_backfill_stop_price_calculates_correctly PASSED
# test_backfill_validates_stop_direction PASSED
# test_execution_token_prevents_duplicate_events PASSED
# test_stop_executor_idempotency_prevents_duplicate_execution PASSED
# test_stop_executor_emits_events_on_success PASSED
# test_stop_executor_updates_projection PASSED
# test_stop_executor_emits_failed_event_on_error PASSED
# test_simultaneous_ws_and_cron_triggers_deduplicated PASSED
# test_price_monitor_uses_absolute_stop_price PASSED
# test_price_monitor_skips_operation_without_stop_price PASSED
# (+ 2 mais)
```

### 4.2 Rodar Teste Específico

```bash
# Testar idempotência
pytest api/tests/test_event_sourcing_stop_monitor.py::test_execution_token_prevents_duplicate_events -v

# Testar backfill
pytest api/tests/test_event_sourcing_stop_monitor.py::test_backfill_stop_price_calculates_correctly -v

# Testar deduplicação WS + Cron
pytest api/tests/test_event_sourcing_stop_monitor.py::test_simultaneous_ws_and_cron_triggers_deduplicated -v
```

### 4.3 Testes com Coverage

```bash
# Ver coverage do código
pytest api/tests/test_event_sourcing_stop_monitor.py \
    --cov=api.application.stop_monitor \
    --cov=api.management.commands.backfill_stop_price \
    --cov-report=term-missing \
    --cov-report=html

# Abrir relatório HTML
# coverage_html/index.html
```

---

## Passo 5: Testar Monitor em Dry-Run

### 5.1 Testar Monitor (Sem Executar Ordens)

```bash
# Single check (dry-run)
python manage.py monitor_stops --dry-run

# Continuous monitoring (dry-run, every 5 seconds)
python manage.py monitor_stops --dry-run --continuous --interval 5

# JSON output (para logs/debugging)
python manage.py monitor_stops --dry-run --json
```

**Esperado** (se houver stops triggered):

```
🛑 STOP_LOSS: Op#123 BTCUSDC @ 88000.00
   Expected PnL: -20.00 USDC
```

**Esperado** (se não houver triggers):

```
✓ No triggers
```

### 5.2 Criar Operação de Teste

```bash
# Via Django shell
python manage.py shell

from api.models import Operation, Symbol, Strategy, Client, Order
from decimal import Decimal
from django.contrib.auth import get_user_model

User = get_user_model()
user = User.objects.first()
client = user.client
symbol = Symbol.objects.filter(name="BTCUSDC").first()
strategy = Strategy.objects.first()

# Criar operação com stop_price
operation = Operation.objects.create(
    client=client,
    symbol=symbol,
    strategy=strategy,
    side="BUY",
    status="ACTIVE",
    stop_price=Decimal("88200.00"),  # Stop absoluto
    target_price=Decimal("93600.00"),  # Target absoluto
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
```

### 5.3 Simular Trigger (Manualmente)

```bash
# Ajustar stop_price para trigger imediato
python manage.py shell

from api.models import Operation
from decimal import Decimal

op = Operation.objects.get(id=123)  # ID da operação de teste
op.stop_price = Decimal("95000.00")  # Acima do preço atual (BUY)
op.save()

# Sair do shell e rodar monitor
exit

# Monitor detectará o trigger
python manage.py monitor_stops --dry-run
```

---

## Passo 6: Verificar Event Sourcing

### 6.1 Verificar Tabelas Criadas

```bash
psql -U postgres -d robson

-- Verificar tabelas
\dt stop_*

-- Esperado:
-- stop_events
-- stop_executions

-- Ver estrutura
\d stop_events
\d stop_executions
```

### 6.2 Query Event Log

```sql
-- Ver todos os eventos (deve estar vazio inicialmente)
SELECT COUNT(*) FROM stop_events;

-- Ver executions (deve estar vazio inicialmente)
SELECT COUNT(*) FROM stop_executions;

-- Depois de executar um stop (em produção), ver eventos:
SELECT
    event_seq,
    event_type,
    occurred_at,
    source,
    operation_id,
    exchange_order_id,
    fill_price,
    slippage_pct
FROM stop_events
ORDER BY event_seq DESC
LIMIT 10;

-- Ver status atual das executions
SELECT
    o.id,
    o.symbol_id,
    e.status,
    e.triggered_at,
    e.executed_at,
    e.fill_price,
    e.slippage_pct,
    e.source
FROM stop_executions e
JOIN operation o ON e.operation_id = o.id
ORDER BY e.triggered_at DESC
LIMIT 10;
```

---

## Passo 7: Testar Idempotência (Crítico!)

### 7.1 Teste Manual de Race Condition

```bash
# Terminal 1: Executar monitor
python manage.py monitor_stops

# Terminal 2: Ao mesmo tempo, executar novamente
python manage.py monitor_stops

# Apenas UM deve executar o stop
# O outro deve logar: "Duplicate execution prevented (idempotency)"
```

### 7.2 Verificar Event Log

```sql
-- Ver eventos para uma operação
SELECT
    event_seq,
    event_type,
    execution_token,
    source,
    occurred_at
FROM stop_events
WHERE operation_id = 123
ORDER BY event_seq;

-- Esperado (se idempotência funcionou):
-- event_seq | event_type           | source
-- ----------|---------------------|-------
-- 1         | STOP_TRIGGERED      | cron
-- 2         | EXECUTION_SUBMITTED | cron
-- 3         | EXECUTED            | cron

-- NÃO deve haver eventos duplicados com mesmo execution_token
```

---

## Passo 8: Deploy para Produção

### 8.1 Checklist Pré-Deploy

- [ ] Migrations testadas em staging
- [ ] Backfill testado em staging
- [ ] Testes passando (12/12)
- [ ] PostgreSQL >= 11
- [ ] Backup do banco de dados
- [ ] Monitoramento configurado (logs)
- [ ] Rollback plan documentado

### 8.2 Deploy (Kubernetes)

```bash
# Verificar deployment atual
kubectl get pods -n robson

# Aplicar migrations via Job
kubectl create job --from=cronjob/django-migrate migrate-event-sourcing -n robson

# Ou via pod direto
kubectl exec -it deployment/rbs-backend-monolith-prod-deploy -n robson -- \
    python manage.py migrate api

# Verificar migrations aplicadas
kubectl exec -it deployment/rbs-backend-monolith-prod-deploy -n robson -- \
    python manage.py showmigrations api | grep -E "(0015|0016|0017|0018)"

# Executar backfill
kubectl exec -it deployment/rbs-backend-monolith-prod-deploy -n robson -- \
    python manage.py backfill_stop_price --batch-size 500
```

### 8.3 Monitorar CronJob

```bash
# Ver CronJob do stop monitor
kubectl get cronjob -n robson

# Ver últimos jobs executados
kubectl get jobs -n robson -l app=rbs-stop-monitor --sort-by=.status.startTime

# Ver logs do monitor
kubectl logs -n robson -l app=rbs-stop-monitor --tail=100 -f

# Ver eventos de stop (via Django shell no pod)
kubectl exec -it deployment/rbs-backend-monolith-prod-deploy -n robson -- \
    python manage.py shell

from api.models.event_sourcing import StopEvent
print(StopEvent.objects.count())
for event in StopEvent.objects.order_by('-event_seq')[:10]:
    print(f"{event.event_seq}: {event.event_type} - Op#{event.operation_id}")
```

---

## Passo 9: Monitoramento Contínuo

### 9.1 Queries de Monitoramento

```sql
-- Dashboard: Executions por status (últimas 24h)
SELECT
    status,
    COUNT(*) as count
FROM stop_executions
WHERE triggered_at > NOW() - INTERVAL '24 hours'
GROUP BY status;

-- Dashboard: Slippage médio
SELECT
    AVG(slippage_pct) as avg_slippage,
    MAX(slippage_pct) as max_slippage,
    COUNT(*) as execution_count
FROM stop_executions
WHERE status = 'EXECUTED'
  AND executed_at > NOW() - INTERVAL '24 hours';

-- Dashboard: Latência (trigger → execution)
SELECT
    AVG(EXTRACT(EPOCH FROM (executed_at - triggered_at))) as avg_latency_seconds,
    MAX(EXTRACT(EPOCH FROM (executed_at - triggered_at))) as max_latency_seconds
FROM stop_executions
WHERE status = 'EXECUTED'
  AND executed_at > NOW() - INTERVAL '24 hours';

-- Alertas: Failures recentes
SELECT
    operation_id,
    error_message,
    failed_at,
    retry_count
FROM stop_executions
WHERE status = 'FAILED'
  AND failed_at > NOW() - INTERVAL '1 hour'
ORDER BY failed_at DESC;
```

### 9.2 Logs para Monitorar

```bash
# Logs do CronJob (Kubernetes)
kubectl logs -n robson -l app=rbs-stop-monitor --tail=50 -f | grep -E "(STOP_LOSS|TAKE_PROFIT|Duplicate|Failed)"

# Logs esperados (sucesso):
# ⚡ Executing STOP_LOSS for Operation 123
# ✅ STOP_LOSS executed: Order 67890, PnL: -20.00, Slippage: 0.15%

# Logs esperados (idempotency):
# ⚠️  Execution token collision for Operation 123: Another process already claimed this execution
```

---

## Passo 10: Próxima Fase (Rust WebSocket Service)

### Quando backend estiver estável em produção

1. **Implementar Rust WebSocket Service**
   - Ver ADR-0012 para arquitetura
   - WebSocket contínuo com Binance
   - Latência <100ms (vs 60s do CronJob)

2. **Integrar com Backend Existente**
   - Rust service emite eventos para `stop_events`
   - Usa mesmo `execution_token` (idempotência compartilhada)
   - CronJob continua como backstop (fallback)

3. **Implementar Guardrails**
   - Slippage limit (max 5%)
   - Circuit breaker (per-symbol)
   - Kill switch (per-tenant)
   - Stale price detection (pause se WS desconectar >30s)

---

## Troubleshooting

### Migration Falhou

```bash
# Ver migrations aplicadas
python manage.py showmigrations api

# Rollback para antes da 0015
python manage.py migrate api 0014

# Tentar novamente
python manage.py migrate api
```

### Backfill com Erros

```bash
# Ver operações problemáticas
python manage.py shell

from api.models import Operation

# Operações sem entry_price (não podem ser backfilled)
ops_no_entry = Operation.objects.filter(
    stop_loss_percent__isnull=False,
    stop_price__isnull=True,
    entry_orders__isnull=True
)
print(f"Operations without entry: {ops_no_entry.count()}")

# Corrigir manualmente se necessário
for op in ops_no_entry:
    # Analisar caso a caso
    pass
```

### Testes Falhando

```bash
# Limpar banco de dados de teste
python manage.py flush --database=test

# Re-rodar testes
pytest api/tests/test_event_sourcing_stop_monitor.py -v --tb=short

# Ver detalhes do erro
pytest api/tests/test_event_sourcing_stop_monitor.py::test_name -vv
```

### Monitor Não Detecta Triggers

```bash
# Verificar operações ACTIVE com stop_price
python manage.py shell

from api.models import Operation

active_ops = Operation.objects.filter(
    status="ACTIVE",
    stop_price__isnull=False
)
print(f"Active operations with stop: {active_ops.count()}")

for op in active_ops[:5]:
    print(f"Op#{op.id}: {op.symbol.name} stop={op.stop_price} entry={op.average_entry_price}")
```

---

## Documentação Completa

Ver documentação detalhada em:

- `docs/guides/IMPLEMENTATION-SUMMARY-ADR-0012.md` - Resumo completo
- `docs/guides/MIGRATION-DIFF-REVIEW.md` - Review de migrations
- `docs/adr/ADR-0012-rust-stop-monitor-event-sourcing.md` - Decisão arquitetural

---

**Última Atualização**: 2024-12-25
**Status**: ✅ Pronto para deployment em staging
