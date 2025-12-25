# Mapa Completo de DNS - rbx.ia.br

**Data**: 2024-12-25
**Status**: ✅ DNS Criados e propagando
**Responsável**: Leandro Damásio

---

## DNS Criados no registro.br

### Produção (Cluster Kubernetes: 158.220.116.31)

| Subdomínio | IP | Serviço | Status |
|------------|-----|---------|--------|
| `rbx.ia.br` | 158.220.116.31 | Site principal / API | ✅ Criado |
| `robson.rbx.ia.br` | 158.220.116.31 | Frontend produção | ✅ Criado |
| `api.robson.rbx.ia.br` | 158.220.116.31 | Backend API produção | ✅ Criado |
| `app.robson.rbx.ia.br` | 158.220.116.31 | Aplicação web produção | ✅ Criado |
| `ws.rbx.ia.br` | 158.220.116.31 | WebSocket (futuro) | ✅ Criado |
| `criticws.rbx.ia.br` | 158.220.116.31 | **Rust WebSocket Service (crítico)** | ✅ Criado |
| `rabbitmq.rbx.ia.br` | 158.220.116.31 | RabbitMQ Management UI | ✅ Criado |
| `grafana.rbx.ia.br` | 158.220.116.31 | Grafana Monitoring | ✅ Criado |
| `prometheus.rbx.ia.br` | 158.220.116.31 | Prometheus Metrics | ✅ Criado |
| `argocd.rbx.ia.br` | 158.220.116.31 | ArgoCD GitOps Dashboard | ✅ Criado |
| `argocd.robson.rbx.ia.br` | 158.220.116.31 | ArgoCD (alias) | ✅ Criado |
| `tiger.rbx.ia.br` | 158.220.116.31 | (Reservado) | ✅ Criado |

### Staging (Cluster Kubernetes: 158.220.116.31)

| Subdomínio | IP | Serviço | Status |
|------------|-----|---------|--------|
| `staging.rbx.ia.br` | 158.220.116.31 | Frontend staging | ✅ Criado |
| `api.staging.rbx.ia.br` | 158.220.116.31 | Backend API staging | ✅ Criado |
| `ws.staging.rbx.ia.br` | 158.220.116.31 | WebSocket staging (futuro) | ✅ Criado |
| `criticws.staging.rbx.ia.br` | 158.220.116.31 | **Rust WebSocket Service staging** | ✅ Criado |
| `rabbitmq.staging.rbx.ia.br` | 158.220.116.31 | RabbitMQ Management UI staging | ✅ Criado |
| `grafana.staging.rbx.ia.br` | 158.220.116.31 | Grafana Monitoring staging | ✅ Criado |

### Outros Servidores (Fora do Cluster Kubernetes)

| Subdomínio | IP | Propósito | Status |
|------------|-----|-----------|--------|
| `bengal.rbx.ia.br` | 164.68.96.68 | Servidor dedicado | ✅ Criado |
| `eagle.rbx.ia.br` | 167.86.92.97 | Servidor dedicado | ✅ Criado |
| `pantera.rbx.ia.br` | 149.102.139.33 | Servidor dedicado | ✅ Criado |

---

## Arquitetura de Roteamento

### Como funciona (Istio Gateway)

Todos os subdomínios `*.rbx.ia.br` apontam para o **mesmo LoadBalancer IP** (158.220.116.31).

O roteamento é feito pelo **Istio Gateway** com base no `Host` header HTTP:

```yaml
# Exemplo: Istio VirtualService
apiVersion: networking.istio.io/v1beta1
kind: VirtualService
metadata:
  name: backend-staging
  namespace: staging
spec:
  hosts:
  - api.staging.rbx.ia.br  # Host header
  gateways:
  - istio-system/gateway-staging
  http:
  - match:
    - uri:
        prefix: /
    route:
    - destination:
        host: backend-staging.staging.svc.cluster.local
        port:
          number: 8000
```

**Fluxo**:
1. Cliente acessa `https://api.staging.rbx.ia.br`
2. DNS resolve para `158.220.116.31` (LoadBalancer)
3. Istio Gateway recebe request com `Host: api.staging.rbx.ia.br`
4. VirtualService roteia para `backend-staging.staging.svc.cluster.local:8000`

---

## Mapeamento Completo: DNS → Serviço

### Produção (namespace: robson)

```
https://rbx.ia.br
  └─> Istio Gateway (istio-system)
      └─> frontend-prod.robson.svc.cluster.local:3000

https://robson.rbx.ia.br
  └─> Istio Gateway
      └─> frontend-prod.robson.svc.cluster.local:3000

https://api.robson.rbx.ia.br
  └─> Istio Gateway
      └─> backend-prod.robson.svc.cluster.local:8000

https://app.robson.rbx.ia.br
  └─> Istio Gateway
      └─> frontend-prod.robson.svc.cluster.local:3000

https://criticws.rbx.ia.br  (⭐ Rust WebSocket Service)
  └─> Istio Gateway
      └─> rust-ws-prod.robson.svc.cluster.local:8080

https://rabbitmq.rbx.ia.br
  └─> Istio Gateway
      └─> rabbitmq-prod.robson.svc.cluster.local:15672 (Management UI)

https://grafana.rbx.ia.br
  └─> Istio Gateway
      └─> grafana.monitoring.svc.cluster.local:3000

https://prometheus.rbx.ia.br
  └─> Istio Gateway
      └─> prometheus.monitoring.svc.cluster.local:9090

https://argocd.rbx.ia.br
  └─> Istio Gateway
      └─> argocd-server.argocd.svc.cluster.local:8080
```

### Staging (namespace: staging)

```
https://staging.rbx.ia.br
  └─> Istio Gateway (istio-system)
      └─> frontend-staging.staging.svc.cluster.local:3000

https://api.staging.rbx.ia.br
  └─> Istio Gateway
      └─> backend-staging.staging.svc.cluster.local:8000

https://criticws.staging.rbx.ia.br  (⭐ Rust WebSocket staging)
  └─> Istio Gateway
      └─> rust-ws-staging.staging.svc.cluster.local:8080

https://rabbitmq.staging.rbx.ia.br
  └─> Istio Gateway
      └─> rabbitmq-staging.staging.svc.cluster.local:15672

https://grafana.staging.rbx.ia.br
  └─> Istio Gateway
      └─> grafana-staging.staging.svc.cluster.local:3000
```

---

## Nomenclatura Adotada

### Padrão de Subdomínios

**Produção**:
- `rbx.ia.br` - Raiz (site principal / API)
- `robson.rbx.ia.br` - Frontend produção
- `api.robson.rbx.ia.br` - Backend API
- `<serviço>.rbx.ia.br` - Serviços específicos (rabbitmq, grafana, etc.)
- `criticws.rbx.ia.br` - ⭐ **Rust WebSocket Service** (crítico para stop-loss)

**Staging**:
- `staging.rbx.ia.br` - Frontend staging
- `<serviço>.staging.rbx.ia.br` - Serviços staging
- `criticws.staging.rbx.ia.br` - ⭐ **Rust WebSocket staging**

### Convenção de Nomenclatura

| Ambiente | Padrão | Exemplo |
|----------|--------|---------|
| Produção | `<serviço>.rbx.ia.br` | `grafana.rbx.ia.br` |
| Produção (alias) | `<serviço>.robson.rbx.ia.br` | `api.robson.rbx.ia.br` |
| Staging | `<serviço>.staging.rbx.ia.br` | `api.staging.rbx.ia.br` |
| Preview (futuro) | `<branch>.preview.rbx.ia.br` | `feature-auth.preview.rbx.ia.br` |

---

## Rust WebSocket Service (criticws)

### Por que "criticws"?

**criticws** = **Critic**al **W**eb**S**ocket

- **Crítico**: Serviço crítico para execução de stop-loss
- **Latência <100ms**: WebSocket contínuo com Binance
- **Rust**: Alta performance, baixo uso de memória
- **Separado do backend Django**: Escala independentemente

### Endpoints

**Produção**:
- `wss://criticws.rbx.ia.br/ws` - WebSocket para monitor de preços
- `https://criticws.rbx.ia.br/health` - Health check

**Staging**:
- `wss://criticws.staging.rbx.ia.br/ws` - WebSocket staging
- `https://criticws.staging.rbx.ia.br/health` - Health check staging

### Quando Implementar

**Fase 2** (após validar Event Sourcing em produção):
1. Implementar Rust WebSocket service
2. Deploy em `criticws.staging.rbx.ia.br`
3. Testes de latência e throughput
4. Deploy em `criticws.rbx.ia.br` (produção)
5. CronJob vira fallback (backstop)

---

## Certificados TLS (cert-manager)

### Certificados Necessários

**Produção**:
```yaml
# Wildcard production
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: rbx-ia-br-wildcard
  namespace: robson
spec:
  secretName: rbx-ia-br-tls
  issuerRef:
    name: letsencrypt-prod
    kind: ClusterIssuer
  dnsNames:
  - rbx.ia.br
  - "*.rbx.ia.br"
  - "*.robson.rbx.ia.br"
```

**Staging**:
```yaml
# Wildcard staging
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: staging-rbx-ia-br-wildcard
  namespace: staging
spec:
  secretName: staging-rbx-ia-br-tls
  issuerRef:
    name: letsencrypt-prod
    kind: ClusterIssuer
  dnsNames:
  - staging.rbx.ia.br
  - "*.staging.rbx.ia.br"
```

**Nota**: Como registro.br não suporta wildcard, cert-manager criará certificado SAN (Subject Alternative Names) com todos os subdomínios.

---

## Verificação DNS (Comandos)

### Verificar Propagação

```bash
# Staging
dig staging.rbx.ia.br +short
dig api.staging.rbx.ia.br +short
dig criticws.staging.rbx.ia.br +short

# Produção
dig rbx.ia.br +short
dig robson.rbx.ia.br +short
dig api.robson.rbx.ia.br +short
dig criticws.rbx.ia.br +short

# Esperado: 158.220.116.31 em todos
```

### Verificar TLS (Após Deploy)

```bash
# Staging
curl -I https://staging.rbx.ia.br 2>&1 | grep -E "(HTTP|SSL)"
curl -I https://api.staging.rbx.ia.br 2>&1 | grep -E "(HTTP|SSL)"

# Produção
curl -I https://rbx.ia.br 2>&1 | grep -E "(HTTP|SSL)"
curl -I https://api.robson.rbx.ia.br 2>&1 | grep -E "(HTTP|SSL)"
```

---

## Roadmap de DNS

### ✅ Fase 1: Staging (Concluída)
- [x] `staging.rbx.ia.br`
- [x] `api.staging.rbx.ia.br`
- [x] `criticws.staging.rbx.ia.br`
- [x] `rabbitmq.staging.rbx.ia.br`
- [x] `grafana.staging.rbx.ia.br`

### ✅ Fase 2: Produção (Concluída)
- [x] `rbx.ia.br`
- [x] `robson.rbx.ia.br`
- [x] `api.robson.rbx.ia.br`
- [x] `criticws.rbx.ia.br`
- [x] `rabbitmq.rbx.ia.br`
- [x] `grafana.rbx.ia.br`
- [x] `prometheus.rbx.ia.br`
- [x] `argocd.rbx.ia.br`

### ⏳ Fase 3: Preview Environments (Futuro)
- [ ] Sistema para criar DNS dinâmicos por branch
- [ ] Exemplo: `feature-auth.preview.rbx.ia.br`
- [ ] Requer automação (GitHub Actions + API registro.br ou Cloudflare)

---

## Observações Importantes

### 1. Wildcard Não Disponível
- ❌ registro.br não suporta wildcard (`*.staging.rbx.ia.br`)
- ✅ Solução: Criar registros explícitos para cada serviço
- ✅ Benefício: Mais controle e segurança

### 2. Mesmo IP para Todos
- ✅ Todos os subdomínios apontam para 158.220.116.31
- ✅ Roteamento feito por Istio (Layer 7 - HTTP Host header)
- ✅ Economia de IPs públicos

### 3. TLS Certificates
- ✅ cert-manager emite certificados automaticamente
- ✅ Let's Encrypt (validação HTTP-01)
- ✅ Renovação automática (90 dias → renova aos 60)

### 4. Isolamento Staging/Produção
- ✅ Mesmo cluster Kubernetes, namespaces separados
- ✅ Network Policies impedem comunicação cross-namespace
- ✅ Secrets/ConfigMaps completamente isolados

---

## Próximos Passos

1. ✅ **DNS Criados** (aguardando propagação)
2. ⏳ **Criar namespace staging** (Kubernetes)
3. ⏳ **Deploy PostgreSQL staging**
4. ⏳ **Deploy Backend staging**
5. ⏳ **Configurar Istio Gateway** (roteamento por Host header)
6. ⏳ **Emitir certificados TLS** (cert-manager)
7. ⏳ **Smoke tests** (health checks)

---

**Última Atualização**: 2024-12-25
**Status DNS**: ✅ Criados, aguardando propagação (5-60 minutos)
**Próximo**: Criar namespace staging e manifestos Kubernetes
