# Arquitetura de Staging - Isolamento Completo

**Data**: 2024-12-25
**Versão**: 1.0
**Domínio Base**: `rbx.ia.br`

---

## Princípios Fundamentais

### 1. Isolamento Total

**CRÍTICO**: O ambiente de staging é **100% isolado** de desenvolvimento e produção:

- ✅ Banco de dados PostgreSQL dedicado (cluster separado)
- ✅ Redis dedicado (instância separada)
- ✅ RabbitMQ dedicado (cluster separado)
- ✅ Namespace Kubernetes isolado (`staging`)
- ✅ Secrets/ConfigMaps separados (credenciais diferentes)
- ✅ Persistent Volumes separados (dados isolados)
- ✅ Network Policies (isolamento de rede)
- ✅ Resource Quotas (limite de recursos)
- ✅ Subdomínios DNS dedicados

### 2. Paridade com Produção

Staging **replica** a arquitetura de produção:

- Same infrastructure as code (IaC)
- Same Kubernetes manifests (diferentes variáveis)
- Same monitoring/observability stack
- Same backup/restore procedures
- **Diferença**: Menores recursos (menos réplicas, menos CPU/RAM)

---

## Arquitetura de Rede

### Namespaces Kubernetes

```
k3s cluster
├── namespace: development (dev local, port-forward only)
├── namespace: staging (staging isolado)
│   ├── NetworkPolicy: deny-all (default)
│   ├── NetworkPolicy: allow-staging-internal
│   └── NetworkPolicy: allow-ingress-from-istio
└── namespace: production (robson)
    ├── NetworkPolicy: deny-all (default)
    ├── NetworkPolicy: allow-production-internal
    └── NetworkPolicy: allow-ingress-from-istio
```

### Isolamento de Rede (Network Policies)

**Regra 1**: Staging NÃO pode comunicar com production (e vice-versa)

```yaml
# staging namespace
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: deny-cross-namespace
  namespace: staging
spec:
  podSelector: {}
  policyTypes:
  - Ingress
  - Egress
  ingress:
  - from:
    - namespaceSelector:
        matchLabels:
          name: staging  # Apenas staging
  egress:
  - to:
    - namespaceSelector:
        matchLabels:
          name: staging  # Apenas staging
    - namespaceSelector:
        matchLabels:
          name: istio-system  # Istio necessário
    - namespaceSelector:
        matchLabels:
          name: kube-system  # DNS necessário
```

**Regra 2**: Staging tem seus próprios recursos

```yaml
apiVersion: v1
kind: ResourceQuota
metadata:
  name: staging-quota
  namespace: staging
spec:
  hard:
    requests.cpu: "4"
    requests.memory: 8Gi
    persistentvolumeclaims: "10"
    services.loadbalancers: "1"
```

---

## Componentes do Ambiente Staging

### 1. PostgreSQL (Staging Database)

**Deployment**: Pod dedicado (NÃO compartilhado com prod)

```yaml
# PostgreSQL Staging
Name: postgres-staging
Namespace: staging
Image: paradedb/paradedb:latest
PVC: postgres-staging-data (10Gi)
Service: postgres-staging.staging.svc.cluster.local:5432
Database: robson_staging
User: robson_staging
Password: <staging-specific-secret>
```

**Backup**:
- Daily backup to S3 (bucket: `rbx-backup-staging`)
- Retention: 7 days (vs 30 days in production)

**Isolamento**:
- ✅ Cluster PostgreSQL separado (pod dedicado)
- ✅ PVC separado (dados isolados)
- ✅ Secret separado (credenciais diferentes de prod)
- ✅ Sem replicação de/para produção

### 2. Redis (Staging Cache)

**Deployment**: Pod dedicado

```yaml
# Redis Staging
Name: redis-staging
Namespace: staging
Image: redis:7-alpine
PVC: redis-staging-data (5Gi)
Service: redis-staging.staging.svc.cluster.local:6379
Password: <staging-specific-secret>
```

**Isolamento**:
- ✅ Instância Redis separada
- ✅ PVC separado
- ✅ Secret separado
- ✅ Sem conexão com Redis de produção

### 3. RabbitMQ (Staging Message Queue)

**Deployment**: Pod dedicado (ou cluster se necessário)

```yaml
# RabbitMQ Staging
Name: rabbitmq-staging
Namespace: staging
Image: rabbitmq:3-management-alpine
PVC: rabbitmq-staging-data (5Gi)
Service: rabbitmq-staging.staging.svc.cluster.local:5672
Management UI: https://rabbitmq.staging.rbx.ia.br
User: robson_staging
Password: <staging-specific-secret>
```

**Isolamento**:
- ✅ Cluster RabbitMQ separado
- ✅ PVC separado
- ✅ Filas isoladas (não recebe eventos de prod)
- ✅ Management UI separado (subdomínio dedicado)

### 4. Backend (Django Monolith)

**Deployment**: 2 réplicas (vs 3 em produção)

```yaml
# Backend Staging
Name: backend-staging
Namespace: staging
Image: ghcr.io/ldamasio/rbs-backend-monolith:staging-<SHA>
Replicas: 2
CPU: 500m (vs 1000m em prod)
Memory: 1Gi (vs 2Gi em prod)
Env:
  - ENVIRONMENT: staging
  - DATABASE_URL: postgres://robson_staging@postgres-staging:5432/robson_staging
  - REDIS_URL: redis://redis-staging:6379/0
  - RABBITMQ_URL: amqp://robson_staging@rabbitmq-staging:5672
  - DEBUG: "False"
  - ALLOWED_HOSTS: api.staging.rbx.ia.br
```

**Service**:
```yaml
Service: backend-staging.staging.svc.cluster.local:8000
Ingress: https://api.staging.rbx.ia.br
```

### 5. Frontend (React)

**Deployment**: 1 réplica (vs 2 em produção)

```yaml
# Frontend Staging
Name: frontend-staging
Namespace: staging
Image: ghcr.io/ldamasio/rbs-frontend:staging-<SHA>
Replicas: 1
CPU: 200m (vs 500m em prod)
Memory: 512Mi (vs 1Gi em prod)
Env:
  - VITE_API_URL: https://api.staging.rbx.ia.br
  - VITE_WS_URL: wss://ws.staging.rbx.ia.br
```

**Service**:
```yaml
Service: frontend-staging.staging.svc.cluster.local:3000
Ingress: https://staging.rbx.ia.br
```

### 6. Stop Monitor (CronJob)

**CronJob**: Executa a cada 1 minuto (igual produção)

```yaml
# Stop Monitor Staging
Name: stop-monitor-staging
Namespace: staging
Schedule: "*/1 * * * *"  # Every 1 minute
Image: ghcr.io/ldamasio/rbs-backend-monolith:staging-<SHA>
Command: ["python", "manage.py", "monitor_stops"]
Env:
  - ENVIRONMENT: staging
  - DATABASE_URL: postgres://robson_staging@postgres-staging:5432/robson_staging
```

### 7. Rust WebSocket Service (Futuro)

**Deployment**: 1 réplica

```yaml
# Rust WS Staging (Fase 2)
Name: rust-ws-staging
Namespace: staging
Image: ghcr.io/ldamasio/rbs-rust-ws:staging-<SHA>
Replicas: 1
CPU: 300m
Memory: 512Mi
Service: ws.staging.rbx.ia.br:443
```

---

## Subdomínios DNS

### Subdomínios Necessários (Staging)

Todos apontam para o **LoadBalancer Kubernetes** do cluster:

| Subdomínio | Tipo | Destino | Propósito |
|------------|------|---------|-----------|
| `staging.rbx.ia.br` | A | `<K8S_LB_IP>` | Frontend staging |
| `api.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | Backend API staging |
| `ws.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | WebSocket staging (futuro) |
| `rabbitmq.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | RabbitMQ Management UI |
| `grafana.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | Grafana monitoring staging |
| `*.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | Wildcard (opcional, para serviços adicionais) |

**Nota**: Todos os subdomínios staging usam o **mesmo LoadBalancer IP** do Kubernetes. O roteamento é feito por **Istio Gateway** com base no `Host` header.

---

## Secrets e ConfigMaps

### Secrets Staging (ISOLADOS de produção)

```yaml
# staging/secrets/postgres-staging.yaml
apiVersion: v1
kind: Secret
metadata:
  name: postgres-staging
  namespace: staging
type: Opaque
data:
  POSTGRES_USER: <base64(robson_staging)>
  POSTGRES_PASSWORD: <base64(DIFFERENT_FROM_PROD)>
  POSTGRES_DB: <base64(robson_staging)>
```

```yaml
# staging/secrets/django-staging.yaml
apiVersion: v1
kind: Secret
metadata:
  name: django-staging
  namespace: staging
type: Opaque
data:
  SECRET_KEY: <base64(DIFFERENT_FROM_PROD)>
  BINANCE_API_KEY: <base64(testnet_key)>  # Binance Testnet
  BINANCE_API_SECRET: <base64(testnet_secret)>
  DATABASE_URL: <base64(postgres://robson_staging@postgres-staging:5432/robson_staging)>
```

**CRÍTICO**:
- ✅ Senhas diferentes de produção
- ✅ API keys de **Binance Testnet** (não produção)
- ✅ SECRET_KEY diferente de produção

---

## Persistent Volumes (Isolados)

### PVCs Staging

```yaml
# PostgreSQL PVC
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: postgres-staging-data
  namespace: staging
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 10Gi
  storageClassName: local-path  # k3s default

---
# Redis PVC
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: redis-staging-data
  namespace: staging
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 5Gi
  storageClassName: local-path

---
# RabbitMQ PVC
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: rabbitmq-staging-data
  namespace: staging
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 5Gi
  storageClassName: local-path
```

**Isolamento**:
- ✅ PVCs com nomes únicos por ambiente
- ✅ Dados armazenados em volumes separados
- ✅ Backups separados (bucket S3 diferente)

---

## GitOps (ArgoCD)

### Aplicações ArgoCD Staging

```yaml
# ArgoCD Application: Backend Staging
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: backend-staging
  namespace: argocd
spec:
  project: staging
  source:
    repoURL: https://github.com/ldamasio/robson
    targetRevision: main  # ou branch staging
    path: infra/k8s/apps/backend/overlays/staging
  destination:
    server: https://kubernetes.default.svc
    namespace: staging
  syncPolicy:
    automated:
      prune: true
      selfHeal: true
    syncOptions:
    - CreateNamespace=true
```

**ArgoCD Projects**:
- `project: staging` - Apenas recursos no namespace `staging`
- `project: production` - Apenas recursos no namespace `robson`

**Isolamento**:
- ✅ Projetos ArgoCD separados (staging vs production)
- ✅ Deploy automático apenas para namespace correto
- ✅ Sync policies independentes

---

## CI/CD Pipeline (GitHub Actions)

### Workflow Staging

```yaml
# .github/workflows/deploy-staging.yml
name: Deploy to Staging

on:
  push:
    branches:
      - main  # ou staging
    paths:
      - 'apps/backend/**'
      - 'apps/frontend/**'

jobs:
  build-and-deploy:
    runs-on: ubuntu-latest
    steps:
      - name: Build Backend Image
        run: |
          docker build -t ghcr.io/ldamasio/rbs-backend-monolith:staging-${{ github.sha }} .
          docker push ghcr.io/ldamasio/rbs-backend-monolith:staging-${{ github.sha }}

      - name: Update Staging Manifests
        run: |
          cd infra/k8s/apps/backend/overlays/staging
          kustomize edit set image ghcr.io/ldamasio/rbs-backend-monolith:staging-${{ github.sha }}
          git commit -am "chore(staging): update image to staging-${{ github.sha }}"
          git push

      - name: ArgoCD Sync (Staging)
        run: |
          argocd app sync backend-staging --prune
```

**Tags de Imagem**:
- Staging: `ghcr.io/ldamasio/rbs-backend-monolith:staging-<SHA>`
- Production: `ghcr.io/ldamasio/rbs-backend-monolith:sha-<SHA>`

**Isolamento**:
- ✅ Imagens com tags diferentes (staging- vs sha-)
- ✅ Workflows separados (deploy-staging.yml vs deploy-production.yml)
- ✅ ArgoCD sync em aplicações diferentes

---

## Monitoramento (Isolado)

### Prometheus/Grafana Staging

```yaml
# Prometheus Staging
Namespace: staging
ServiceMonitor: backend-staging, postgres-staging, redis-staging
Metrics Retention: 7 days (vs 30 days em prod)
Alerting: Slack channel #staging-alerts (não #production-alerts)
```

### Dashboards Grafana

- `Staging - Backend Overview` (dashboard separado)
- `Staging - Database Metrics` (dashboard separado)
- `Staging - Stop Monitor` (dashboard separado)

**Isolamento**:
- ✅ Namespace prometheus-staging separado (opcional) ou
- ✅ Labels diferentes (`environment=staging` vs `environment=production`)
- ✅ Alertas para canal Slack diferente

---

## TLS Certificates (cert-manager)

### Certificados Staging

```yaml
# Certificado para *.staging.rbx.ia.br
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: staging-rbx-ia-br-wildcard
  namespace: staging
spec:
  secretName: staging-rbx-ia-br-tls
  issuerRef:
    name: letsencrypt-prod  # Mesmo issuer, certificado diferente
    kind: ClusterIssuer
  dnsNames:
  - staging.rbx.ia.br
  - "*.staging.rbx.ia.br"
```

**Isolamento**:
- ✅ Secret TLS separado (`staging-rbx-ia-br-tls`)
- ✅ Certificado emitido para subdomínios staging
- ✅ Armazenado no namespace staging

---

## Backup e Restore (Isolado)

### Backup Strategy Staging

```yaml
# CronJob: Backup PostgreSQL Staging
Name: postgres-backup-staging
Schedule: "0 2 * * *"  # Daily at 2 AM
Destination: S3 bucket rbx-backup-staging/postgres/
Retention: 7 days
```

**Isolamento**:
- ✅ Bucket S3 separado (`rbx-backup-staging` vs `rbx-backup-production`)
- ✅ Retention policy mais curta (7 vs 30 dias)
- ✅ Restore NÃO afeta produção

---

## Testes em Staging

### Procedimento de Teste

1. **Deploy automático** (ArgoCD sync após push)
2. **Smoke tests** (health checks automáticos)
3. **Integration tests** (Playwright, pytest)
4. **Manual testing** (QA team)
5. **Performance testing** (k6, ab)
6. **Promoção para produção** (se todos testes passarem)

### Dados de Teste

**NÃO usar dados de produção em staging!**

- ✅ Dados sintéticos gerados por fixtures
- ✅ Binance Testnet (não API real)
- ✅ Contas de teste (não clientes reais)

---

## Rollback Strategy

### Rollback Staging (Rápido)

```bash
# ArgoCD rollback
argocd app rollback backend-staging

# Ou via Git
git revert <commit>
git push

# ArgoCD auto-sync
```

**Impacto**: Apenas staging (produção não afetada)

---

## Diferenças Staging vs Production

| Recurso | Staging | Production | Motivo |
|---------|---------|------------|--------|
| Réplicas Backend | 2 | 3 | Menor carga |
| CPU Backend | 500m | 1000m | Recursos menores |
| Memory Backend | 1Gi | 2Gi | Recursos menores |
| PostgreSQL PVC | 10Gi | 50Gi | Menos dados |
| Redis PVC | 5Gi | 20Gi | Menos cache |
| Backup Retention | 7 dias | 30 dias | Compliance menor |
| Binance API | Testnet | Real | Segurança |
| Monitoring Retention | 7 dias | 30 dias | Menos métricas |
| TLS Certificate | Staging subdomain | Production subdomain | Isolamento |

---

## Checklist de Isolamento

- [ ] Namespace Kubernetes separado (`staging`)
- [ ] PostgreSQL cluster dedicado (pod separado)
- [ ] Redis instância dedicada
- [ ] RabbitMQ cluster dedicado
- [ ] Secrets diferentes (senhas/API keys)
- [ ] PVCs separados (dados isolados)
- [ ] Network Policies (sem comunicação cross-namespace)
- [ ] Resource Quotas (limites de recursos)
- [ ] Subdomínios DNS dedicados (`*.staging.rbx.ia.br`)
- [ ] TLS certificates separados
- [ ] ArgoCD project separado
- [ ] GitHub Actions workflow separado
- [ ] Imagens Docker com tags diferentes (`staging-<SHA>`)
- [ ] Prometheus labels diferentes (`environment=staging`)
- [ ] Backup S3 bucket separado
- [ ] Binance API Testnet (não produção)
- [ ] Monitoring dashboards separados
- [ ] Alerting channels diferentes (Slack)

---

## Próximos Passos

1. **Criar namespace staging** (kubectl)
2. **Aplicar Network Policies**
3. **Criar Secrets staging**
4. **Deploy PostgreSQL staging**
5. **Deploy Redis staging**
6. **Deploy RabbitMQ staging**
7. **Deploy Backend staging**
8. **Aplicar migrations** (0015-0018)
9. **Executar backfill**
10. **Ativar CronJob monitor**
11. **Deploy Frontend staging**
12. **Configurar Istio Gateway**
13. **Emitir certificados TLS**
14. **Configurar ArgoCD sync**
15. **Smoke tests**

---

**Última Atualização**: 2024-12-25
**Aprovação Necessária**: Arquitetura/DevOps Lead
**Status**: 📋 Documentado, aguardando implementação
