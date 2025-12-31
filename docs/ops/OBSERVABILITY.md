# Observabilidade - Robson Bot

Implementação mínima mas efetiva de observabilidade para produção.

---

## Stack de Observabilidade

### 1. **Metrics** (Prometheus)
- Endpoint: `/metrics`
- Biblioteca: `django-prometheus==2.3.1`
- Auto-discovery via ServiceMonitor
- Métricas automáticas: requests, latency, DB queries, cache hits

### 2. **Logs** (JSON Structured)
- Output: `stdout` (coletado pelo k8s)
- Formato: JSON (produção) / Plain text (desenvolvimento)
- Biblioteca: `python-json-logger==2.0.7`
- Campos: timestamp, level, message, pathname, lineno, funcName

### 3. **Health Checks**
- Endpoint: `/health/`
- Biblioteca: `django-health-check==3.18.3`
- Checks: Database, Cache, Storage
- Formato: JSON com status detalhado

---

## Endpoints de Observabilidade

### `/metrics` - Prometheus Metrics

**URL**: `https://api.robson.rbx.ia.br/metrics`

**Métricas Disponíveis**:

```prometheus
# Request metrics
django_http_requests_total_by_method_total{method="GET",view="api.views.orders"}
django_http_requests_latency_seconds_by_view_method{view="api.views.orders",method="GET"}
django_http_responses_total_by_status_view_method{status="200",view="api.views.orders"}

# Database metrics
django_db_new_connections_total{alias="default"}
django_db_query_count{alias="default"}
django_db_query_duration_seconds{alias="default"}

# Cache metrics
django_cache_get_total{cache="default"}
django_cache_hit_total{cache="default"}
django_cache_miss_total{cache="default"}

# System metrics
django_migrations_applied_total
django_migrations_unapplied_total
```

**Exemplo de Uso**:
```bash
# Test endpoint
curl https://api.robson.rbx.ia.br/metrics

# With Prometheus query
rate(django_http_requests_total_by_method_total[5m])
```

---

### `/health/` - Health Checks

**URL**: `https://api.robson.rbx.ia.br/health/`

**Response Format**:
```json
{
  "status": "healthy",
  "checks": {
    "DatabaseBackend": "working",
    "CacheBackend": "working",
    "DefaultFileStorageHealthCheck": "working"
  }
}
```

**Status Codes**:
- `200 OK` - All checks passed
- `503 Service Unavailable` - One or more checks failed

**Health Check Components**:

| Check | Description | Failure Scenario |
|-------|-------------|------------------|
| `DatabaseBackend` | PostgreSQL connection | DB unavailable, connection pool exhausted |
| `CacheBackend` | Redis connection | Redis down, network partition |
| `DefaultFileStorageHealthCheck` | File system write | Disk full, permission denied |

**Exemplo de Uso**:
```bash
# Quick health check
curl -I https://api.robson.rbx.ia.br/health/

# Detailed health status
curl https://api.robson.rbx.ia.br/health/ | jq
```

---

## Logs Estruturados

### Formato JSON (Produção)

```json
{
  "asctime": "2025-12-31T23:30:00.123Z",
  "name": "api.views.orders",
  "levelname": "INFO",
  "message": "Order created successfully",
  "pathname": "/app/api/views/orders.py",
  "lineno": 42,
  "funcName": "create_order"
}
```

### Campos Disponíveis

| Campo | Descrição | Exemplo |
|-------|-----------|---------|
| `asctime` | Timestamp ISO 8601 | `2025-12-31T23:30:00.123Z` |
| `name` | Logger name | `api.views.orders` |
| `levelname` | Log level | `INFO`, `WARNING`, `ERROR` |
| `message` | Log message | `Order created successfully` |
| `pathname` | Source file path | `/app/api/views/orders.py` |
| `lineno` | Line number | `42` |
| `funcName` | Function name | `create_order` |

### Níveis de Log

| Level | Quando Usar | Exemplo |
|-------|-------------|---------|
| `DEBUG` | Debugging detalhado | SQL queries, variable dumps |
| `INFO` | Eventos normais | User login, order placed |
| `WARNING` | Alertas não-críticos | Deprecated API usage, slow query |
| `ERROR` | Erros que não param o sistema | Failed API call, validation error |
| `CRITICAL` | Erros críticos | Database down, out of memory |

### Visualizando Logs

```bash
# Logs do backend (JSON formatado)
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy | jq

# Filtrar por level
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy | jq 'select(.levelname=="ERROR")'

# Filtrar por módulo
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy | jq 'select(.name | contains("orders"))'

# Follow logs em tempo real
kubectl -n robson logs -f deployment/rbs-backend-monolith-prod-deploy | jq
```

---

## Prometheus ServiceMonitor

### Auto-Discovery

O ServiceMonitor permite que o Prometheus descubra automaticamente o endpoint `/metrics`:

```yaml
# infra/k8s/prod/rbs-backend-monolith-prod-servicemonitor.yml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: rbs-backend-monolith-prod-monitor
  namespace: robson
spec:
  selector:
    matchLabels:
      app: rbs-backend-monolith-prod-deploy
  endpoints:
    - port: http
      path: /metrics
      interval: 30s
      scrapeTimeout: 10s
```

### Configuração do Prometheus

Para que o Prometheus descubra o ServiceMonitor, certifique-se que ele está configurado para monitorar o namespace `robson`:

```yaml
# values.yaml do Prometheus Operator
prometheus:
  prometheusSpec:
    serviceMonitorSelector:
      matchLabels:
        release: prometheus
    serviceMonitorNamespaceSelector:
      matchNames:
        - robson
```

### Verificar Scraping

```bash
# Check ServiceMonitor criado
kubectl -n robson get servicemonitor

# Check targets no Prometheus UI
# http://prometheus.robson.rbx.ia.br/targets
# Deve mostrar: robson/rbs-backend-monolith-prod-monitor/0 (UP)
```

---

## Grafana Dashboards (Opcional)

### Dashboard Básico

Importar dashboard pré-configurado para Django:
- Dashboard ID: `9528` (Django Prometheus Dashboard)
- URL: https://grafana.com/grafana/dashboards/9528

### Queries Úteis

```prometheus
# Request Rate (QPS)
rate(django_http_requests_total_by_method_total[5m])

# Latency P95
histogram_quantile(0.95, rate(django_http_requests_latency_seconds_by_view_method_bucket[5m]))

# Error Rate
rate(django_http_responses_total_by_status_view_method{status=~"5.."}[5m])

# Database Query Duration
rate(django_db_query_duration_seconds_sum[5m]) / rate(django_db_query_duration_seconds_count[5m])
```

---

## Alertas Recomendados

### PrometheusRule (Exemplo)

```yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: robson-backend-alerts
  namespace: robson
spec:
  groups:
    - name: robson-backend
      interval: 30s
      rules:
        - alert: HighErrorRate
          expr: rate(django_http_responses_total_by_status_view_method{status=~"5.."}[5m]) > 0.05
          for: 5m
          labels:
            severity: warning
          annotations:
            summary: "High error rate detected"
            description: "Error rate is {{ $value }} req/s"

        - alert: HighLatency
          expr: histogram_quantile(0.95, rate(django_http_requests_latency_seconds_by_view_method_bucket[5m])) > 2
          for: 5m
          labels:
            severity: warning
          annotations:
            summary: "High latency detected"
            description: "P95 latency is {{ $value }} seconds"

        - alert: HealthCheckFailing
          expr: up{job="robson/rbs-backend-monolith-prod-monitor"} == 0
          for: 2m
          labels:
            severity: critical
          annotations:
            summary: "Backend health check failing"
            description: "Backend is down or unreachable"
```

---

## Troubleshooting

### Métricas não aparecem no Prometheus

```bash
# 1. Verificar ServiceMonitor criado
kubectl -n robson get servicemonitor
kubectl -n robson describe servicemonitor rbs-backend-monolith-prod-monitor

# 2. Verificar labels do Service
kubectl -n robson get svc rbs-backend-monolith-prod-svc -o yaml | grep -A 3 labels

# 3. Testar endpoint /metrics diretamente
kubectl -n robson port-forward svc/rbs-backend-monolith-prod-svc 8000:8000
curl http://localhost:8000/metrics

# 4. Check Prometheus targets
# Acessar: http://prometheus.robson.rbx.ia.br/targets
# Procurar por: robson/rbs-backend-monolith-prod-monitor
```

### Health Check retorna 503

```bash
# 1. Ver detalhes do health check
curl https://api.robson.rbx.ia.br/health/ | jq

# 2. Verificar logs do backend
kubectl -n robson logs deployment/rbs-backend-monolith-prod-deploy | grep -i health

# 3. Testar componentes individualmente
# Database
kubectl -n robson exec deployment/rbs-backend-monolith-prod-deploy -- python manage.py dbshell

# Redis
kubectl -n robson exec deployment/rbs-redis-prod -- redis-cli ping
```

### Logs não estão em JSON

```bash
# Verificar se DEBUG=False em produção
kubectl -n robson get deployment rbs-backend-monolith-prod-deploy -o yaml | grep -A 5 env

# DEBUG=True → logs em plain text
# DEBUG=False → logs em JSON
```

---

## Próximos Passos

### Melhorias Futuras

- [ ] Adicionar distributed tracing (OpenTelemetry)
- [ ] Configurar log aggregation (Loki ou ELK)
- [ ] Criar dashboards customizados no Grafana
- [ ] Adicionar alertas via Slack/PagerDuty
- [ ] Implementar SLO/SLI tracking

### Referências

- Django Prometheus: https://github.com/korfuri/django-prometheus
- Django Health Check: https://github.com/revsys/django-health-check
- Prometheus Best Practices: https://prometheus.io/docs/practices/naming/

---

**Last Updated**: 2025-12-31
**Author**: Claude Code (Anthropic)
**Related**: Infrastructure, Monitoring, SRE
