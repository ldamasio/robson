# Registros DNS Necessários - registro.br

**Domínio**: `rbx.ia.br`
**Data**: 2024-12-25
**Responsável**: Leonardo Damásio

---

## Informações do Cluster Kubernetes

### LoadBalancer IP

**IMPORTANTE**: Antes de criar os registros DNS, você precisa obter o IP do LoadBalancer do Kubernetes:

```bash
# Obter IP do LoadBalancer (k3s + Istio)
kubectl get svc -n istio-system istio-ingressgateway -o jsonpath='{.status.loadBalancer.ingress[0].ip}'

# Exemplo de resposta:
# 158.220.116.31
```

**Substitua `<K8S_LB_IP>` abaixo pelo IP retornado**

---

## Registros DNS para Produção

### Registros Tipo A (Produção)

| Nome | Tipo | Valor | TTL | Descrição |
|------|------|-------|-----|-----------|
| `rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Site principal (futuro) |
| `api.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Backend API (produção) |
| `ws.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | WebSocket Service (futuro Rust WS) |
| `rabbitmq.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | RabbitMQ Management UI (produção) |
| `grafana.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Grafana Monitoring (produção) |
| `prometheus.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Prometheus (produção) |
| `argocd.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | ArgoCD GitOps Dashboard |

---

## Registros DNS para Staging

### Registros Tipo A (Staging)

| Nome | Tipo | Valor | TTL | Descrição |
|------|------|-------|-----|-----------|
| `staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Frontend staging |
| `api.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Backend API staging |
| `ws.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | WebSocket staging (futuro) |
| `rabbitmq.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | RabbitMQ Management UI staging |
| `grafana.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Grafana Monitoring staging |

---

## Registro Wildcard (Opcional)

### Para Serviços Dinâmicos

| Nome | Tipo | Valor | TTL | Descrição |
|------|------|-------|-----|-----------|
| `*.staging.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Wildcard para staging (preview envs, etc.) |
| `*.preview.rbx.ia.br` | A | `<K8S_LB_IP>` | 3600 | Preview environments (feature branches) |

**Nota sobre Wildcards**:
- Facilita criação de preview environments dinâmicos
- Exemplo: `feature-123.preview.rbx.ia.br` para branch `feature/123`
- Opcional: pode criar registros específicos conforme necessário

---

## Registros CNAME (Alternativa)

### Se preferir usar CNAME ao invés de A records

**Vantagem**: Se IP do LoadBalancer mudar, só precisa atualizar um registro

```
# Registro principal A
rbx.ia.br.                    A     <K8S_LB_IP>

# Todos os outros via CNAME
api.rbx.ia.br.                CNAME rbx.ia.br.
ws.rbx.ia.br.                 CNAME rbx.ia.br.
staging.rbx.ia.br.            CNAME rbx.ia.br.
api.staging.rbx.ia.br.        CNAME rbx.ia.br.
ws.staging.rbx.ia.br.         CNAME rbx.ia.br.
rabbitmq.rbx.ia.br.           CNAME rbx.ia.br.
rabbitmq.staging.rbx.ia.br.   CNAME rbx.ia.br.
grafana.rbx.ia.br.            CNAME rbx.ia.br.
grafana.staging.rbx.ia.br.    CNAME rbx.ia.br.
prometheus.rbx.ia.br.         CNAME rbx.ia.br.
argocd.rbx.ia.br.             CNAME rbx.ia.br.
```

**Recomendação**: Use **registros A diretos** (mais simples, menos indireção)

---

## Registros TXT (SPF, DKIM, DMARC)

### Se for enviar emails do domínio rbx.ia.br

```
# SPF (Sender Policy Framework)
rbx.ia.br.    TXT    "v=spf1 include:_spf.google.com ~all"

# DMARC (Domain-based Message Authentication)
_dmarc.rbx.ia.br.    TXT    "v=DMARC1; p=quarantine; rua=mailto:dmarc@rbx.ia.br"

# DKIM (será fornecido pelo provedor de email)
default._domainkey.rbx.ia.br.    TXT    "<DKIM_PUBLIC_KEY>"
```

**Nota**: Apenas necessário se for enviar emails transacionais (ex: notificações, alertas)

---

## Registros MX (Email)

### Se quiser receber emails em @rbx.ia.br

```
# Google Workspace
rbx.ia.br.    MX    1    aspmx.l.google.com.
rbx.ia.br.    MX    5    alt1.aspmx.l.google.com.
rbx.ia.br.    MX    5    alt2.aspmx.l.google.com.
rbx.ia.br.    MX    10   alt3.aspmx.l.google.com.
rbx.ia.br.    MX    10   alt4.aspmx.l.google.com.
```

**Nota**: Apenas se usar Google Workspace ou outro provedor de email

---

## Resumo: Registros DNS Mínimos Necessários

### Para Deploy Inicial (Staging)

**Obrigatórios**:
1. `staging.rbx.ia.br` → `<K8S_LB_IP>`
2. `api.staging.rbx.ia.br` → `<K8S_LB_IP>`

**Recomendados**:
3. `ws.staging.rbx.ia.br` → `<K8S_LB_IP>` (futuro Rust WS)
4. `rabbitmq.staging.rbx.ia.br` → `<K8S_LB_IP>` (debugging)
5. `grafana.staging.rbx.ia.br` → `<K8S_LB_IP>` (monitoring)

### Para Deploy Produção (Após validar staging)

**Obrigatórios**:
1. `rbx.ia.br` → `<K8S_LB_IP>`
2. `api.rbx.ia.br` → `<K8S_LB_IP>`

**Recomendados**:
3. `ws.rbx.ia.br` → `<K8S_LB_IP>` (futuro Rust WS)
4. `rabbitmq.rbx.ia.br` → `<K8S_LB_IP>` (admin)
5. `grafana.rbx.ia.br` → `<K8S_LB_IP>` (monitoring)
6. `prometheus.rbx.ia.br` → `<K8S_LB_IP>` (metrics)
7. `argocd.rbx.ia.br` → `<K8S_LB_IP>` (GitOps dashboard)

---

## Instruções para registro.br

### Passo 1: Obter IP do LoadBalancer

```bash
ssh root@158.220.116.31
kubectl get svc -n istio-system istio-ingressgateway -o wide

# Exemplo de saída:
# NAME                   TYPE           CLUSTER-IP      EXTERNAL-IP      PORT(S)
# istio-ingressgateway   LoadBalancer   10.43.100.200   158.220.116.31   80:30080/TCP,443:30443/TCP

# IP do LoadBalancer: 158.220.116.31
```

### Passo 2: Criar Registros DNS no registro.br

**Interface Web**: https://registro.br/

1. Login com CPF/CNPJ
2. Acessar "Meus domínios" → `rbx.ia.br`
3. Clicar em "DNS" → "Editar zona"
4. Adicionar registros conforme tabela abaixo

### Passo 3: Registros DNS para Criar (Lista Completa)

**COPIAR E COLAR NO REGISTRO.BR**:

```dns
; === PRODUÇÃO ===
rbx.ia.br.                    3600  IN  A      <COLE_IP_AQUI>
api.rbx.ia.br.                3600  IN  A      <COLE_IP_AQUI>
ws.rbx.ia.br.                 3600  IN  A      <COLE_IP_AQUI>
rabbitmq.rbx.ia.br.           3600  IN  A      <COLE_IP_AQUI>
grafana.rbx.ia.br.            3600  IN  A      <COLE_IP_AQUI>
prometheus.rbx.ia.br.         3600  IN  A      <COLE_IP_AQUI>
argocd.rbx.ia.br.             3600  IN  A      <COLE_IP_AQUI>

; === STAGING ===
staging.rbx.ia.br.            3600  IN  A      <COLE_IP_AQUI>
api.staging.rbx.ia.br.        3600  IN  A      <COLE_IP_AQUI>
ws.staging.rbx.ia.br.         3600  IN  A      <COLE_IP_AQUI>
rabbitmq.staging.rbx.ia.br.   3600  IN  A      <COLE_IP_AQUI>
grafana.staging.rbx.ia.br.    3600  IN  A      <COLE_IP_AQUI>

; === WILDCARDS (Opcional) ===
*.staging.rbx.ia.br.          3600  IN  A      <COLE_IP_AQUI>
*.preview.rbx.ia.br.          3600  IN  A      <COLE_IP_AQUI>
```

**Substitua `<COLE_IP_AQUI>` pelo IP do LoadBalancer**

### Passo 4: Verificar Propagação DNS

```bash
# Verificar se DNS foi propagado (pode levar até 24h, mas geralmente é rápido)
dig staging.rbx.ia.br +short
dig api.staging.rbx.ia.br +short

# Esperado: <K8S_LB_IP>
```

---

## Template para Interface Web registro.br

### Formato de Input (registro.br aceita formulário web)

| Campo | Valor |
|-------|-------|
| **Nome** | `staging` |
| **Tipo** | `A` |
| **Conteúdo** | `<K8S_LB_IP>` |
| **TTL** | `3600` |

Repetir para cada subdomínio da tabela acima.

---

## Verificação Pós-Criação

### Comandos de Verificação

```bash
# Verificar todos os registros staging
for subdomain in staging api.staging ws.staging rabbitmq.staging grafana.staging; do
  echo "=== $subdomain.rbx.ia.br ==="
  dig $subdomain.rbx.ia.br +short
  echo ""
done

# Verificar certificados TLS (após deploy)
for subdomain in staging api.staging; do
  echo "=== $subdomain.rbx.ia.br ==="
  curl -I https://$subdomain.rbx.ia.br 2>&1 | grep -E "(HTTP|SSL|TLS)"
  echo ""
done
```

**Esperado**: Todos retornam o mesmo IP do LoadBalancer

---

## Troubleshooting

### DNS não resolve

```bash
# 1. Verificar se DNS foi propagado
nslookup staging.rbx.ia.br 8.8.8.8  # Google DNS
nslookup staging.rbx.ia.br 1.1.1.1  # Cloudflare DNS

# 2. Verificar TTL expirou
dig staging.rbx.ia.br +noall +answer

# 3. Limpar cache local
sudo systemd-resolve --flush-caches  # Linux
ipconfig /flushdns                    # Windows
```

### Certificado TLS não emite

```bash
# Verificar cert-manager
kubectl get certificate -n staging
kubectl describe certificate staging-rbx-ia-br-tls -n staging

# Verificar desafio HTTP-01 (Let's Encrypt)
kubectl get challenges -n staging
```

**Causa comum**: DNS ainda não propagou (cert-manager precisa que DNS resolva)

---

## Plano de Rollout DNS

### Fase 1: Staging (Agora)

1. ✅ Criar registros staging (`*.staging.rbx.ia.br`)
2. ✅ Aguardar propagação (5-60 min)
3. ✅ Deploy staging no Kubernetes
4. ✅ Emitir certificados TLS (cert-manager)
5. ✅ Validar acesso: https://staging.rbx.ia.br

### Fase 2: Produção (Após validar staging)

1. ⏳ Criar registros produção (`rbx.ia.br`, `api.rbx.ia.br`, etc.)
2. ⏳ Deploy produção no Kubernetes
3. ⏳ Emitir certificados TLS
4. ⏳ Validar acesso: https://api.rbx.ia.br
5. ⏳ Migrar tráfego de robsonbot.com → rbx.ia.br (gradual)

### Fase 3: Preview Environments (Futuro)

1. ⏳ Criar wildcard `*.preview.rbx.ia.br`
2. ⏳ Configurar GitHub Actions para preview per-branch
3. ⏳ Exemplo: `feature-auth.preview.rbx.ia.br`

---

## Resumo Executivo

### O que você precisa fazer AGORA no registro.br:

1. **Login**: https://registro.br/ (CPF/CNPJ + senha)
2. **Acessar**: "Meus domínios" → `rbx.ia.br` → "DNS"
3. **Obter IP**: `ssh root@158.220.116.31` → `kubectl get svc -n istio-system istio-ingressgateway`
4. **Criar registros** (copiar tabela abaixo):

| Nome | Tipo | Valor | TTL |
|------|------|-------|-----|
| `staging` | A | `<IP_DO_LOADBALANCER>` | 3600 |
| `api.staging` | A | `<IP_DO_LOADBALANCER>` | 3600 |
| `ws.staging` | A | `<IP_DO_LOADBALANCER>` | 3600 |
| `rabbitmq.staging` | A | `<IP_DO_LOADBALANCER>` | 3600 |
| `grafana.staging` | A | `<IP_DO_LOADBALANCER>` | 3600 |

5. **Aguardar**: 5-60 minutos (propagação DNS)
6. **Verificar**: `dig staging.rbx.ia.br +short` (deve retornar IP)
7. **Notificar**: "DNS criado, pode prosseguir com deploy staging"

---

**Última Atualização**: 2024-12-25
**Responsável**: Leonardo Damásio
**Ação Requerida**: ✅ Criar registros DNS no registro.br conforme tabela acima
