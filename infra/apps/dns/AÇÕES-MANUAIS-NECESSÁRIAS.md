# Ações Manuais Necessárias - DNS Deployment

**Data**: 2026-02-14
**Status**: Manifests configurados, aguardando preenchimento de secrets e acesso ao cluster

---

## ⚠️ CRÍTICO: Não Commitamos Secrets

Os seguintes valores **NÃO estão commitados no Git** e precisam ser preenchidos **ANTES** do deploy:

---

## 1. PostgreSQL Password

### Gerar Password

```bash
POSTGRES_PASSWORD=$(openssl rand -base64 32)
echo "PostgreSQL Password: $POSTGRES_PASSWORD"

# Salvar em local seguro (password manager)
echo "$POSTGRES_PASSWORD" >> ~/dns-secrets-$(date +%Y%m%d).txt
chmod 600 ~/dns-secrets-*.txt
```

### Editar Arquivo

**Arquivo**: `infra/apps/dns/base/postgresql.yaml`

**Substituir** (2 ocorrências):
```yaml
# Linha ~6
stringData:
  postgres-password: "CHANGEME_POSTGRES_ROOT_PASSWORD"  ← Substituir aqui
  password: "CHANGEME_PDNS_PASSWORD"                    ← Substituir aqui (mesmo valor)
  username: "pdns_strategos"  # Manter
  database: "strategos_dns"   # Manter
```

**Por**:
```yaml
stringData:
  postgres-password: "SEU_PASSWORD_GERADO_AQUI"
  password: "SEU_PASSWORD_GERADO_AQUI"
  username: "pdns_strategos"
  database: "strategos_dns"
```

---

## 2. PowerDNS API Key

### Gerar API Key

```bash
PDNS_API_KEY=$(openssl rand -base64 32)
echo "PowerDNS API Key: $PDNS_API_KEY"

# Salvar junto com o password
echo "PDNS_API_KEY=$PDNS_API_KEY" >> ~/dns-secrets-$(date +%Y%m%d).txt
```

### Editar Arquivo

**Arquivo**: `infra/apps/dns/base/powerdns-config.yaml`

**Substituir**:
```yaml
# Linha ~7
stringData:
  api-key: "CHANGEME_PDNS_API_KEY"  ← Substituir aqui
```

**Por**:
```yaml
stringData:
  api-key: "SEU_API_KEY_GERADO_AQUI"
```

---

## 3. Labels de Auditoria (Opcional - Governança)

### Placeholders de Governança

Os deployments têm labels de auditoria com placeholders:

```yaml
rbx.change_id: "CHANGE_ID_PLACEHOLDER"
rbx.agent_id: "AGENT_ID_PLACEHOLDER"
rbx.env: "prod"
```

**Opção A - Substituir Manualmente (Deploy Inicial)**:
```bash
# Exemplo para primeiro deploy
CHANGE_ID="dns-init-$(date +%Y%m%d)"
AGENT_ID="admin-manual"

# Editar infra/apps/dns/overlays/nodeport/powerdns-deployment.yaml
# Substituir placeholders pelos valores acima
```

**Opção B - Via Kustomize (Futuro/CI)**:
```yaml
# No kustomization.yaml, adicionar:
commonLabels:
  rbx.change_id: "${CHANGE_ID}"  # Injetado pelo CI
  rbx.agent_id: "${AGENT_ID}"    # Injetado pelo CI
```

**Opção C - Deixar Placeholders (Aceito Temporariamente)**:
- Labels serão criados com valores literais "CHANGE_ID_PLACEHOLDER"
- Funciona, mas perde rastreabilidade de mudanças
- Pode atualizar posteriormente

## 4. IPs da Zona strategos.gr

### Definir IPs

```bash
# IP do servidor web de strategos.gr
STRATEGOS_IP="IP_DO_SERVIDOR_WEB"

# Opcionais (se aplicável)
THALAMUS_IP="IP_DO_THALAMUS"  # ou deixar vazio
ROBSON_IP="IP_DO_ROBSON"      # ou deixar vazio
```

### Editar Arquivo

**Arquivo**: `infra/apps/dns/base/zone-init-job.yaml`

**Substituir** (linha ~143):
```yaml
env:
  # ... (outros envs acima)
  - name: STRATEGOS_IP
    value: "CHANGEME_STRATEGOS_IP"  ← Substituir aqui
  - name: THALAMUS_IP
    value: ""  # Deixar vazio ou adicionar IP
  - name: ROBSON_IP
    value: ""  # Deixar vazio ou adicionar IP
```

**Por**:
```yaml
env:
  # ... (outros envs acima)
  - name: STRATEGOS_IP
    value: "203.0.113.10"  # EXEMPLO - use IP real
  - name: THALAMUS_IP
    value: "203.0.113.11"  # ou deixe "" se não usado
  - name: ROBSON_IP
    value: "203.0.113.12"  # ou deixe "" se não usado
```

---

## 4. Acesso SSH aos Nodes

Para configurar iptables redirect, você precisa de:

### Chave SSH para tiger e bengal

```bash
# Verificar se já tem acesso
ssh -o ConnectTimeout=5 root@158.220.116.31 "hostname"
ssh -o ConnectTimeout=5 root@164.68.96.68 "hostname"

# Se pedir senha ou negar:
# 1. Copiar sua chave pública para os nodes (uma vez)
# 2. Ou usar password quando rodar o script iptables
```

### Permissões Necessárias

- **root** nos nodes (ou sudo)
- Permissão para modificar iptables
- Permissão para instalar pacotes (iptables-persistent)

---

## 5. Verificar/Criar Glue Records

### Verificar Status Atual

```bash
# Testar se glue records existem
dig @8.8.8.8 ns1.rbxsystems.ch A +short
dig @8.8.8.8 ns2.rbxsystems.ch A +short

# Se retornar vazio ou IPs errados → AÇÃO NECESSÁRIA
```

### Ação Necessária

**Onde**: Depende de onde rbxsystems.ch é gerenciado

**Opção A - Se rbxsystems.ch tem DNS autoritativo gerenciado pela RBX**:
1. Adicionar A records na zona rbxsystems.ch:
   ```
   ns1.rbxsystems.ch.  IN  A  158.220.116.31
   ns2.rbxsystems.ch.  IN  A  164.68.96.68
   ```

**Opção B - Se rbxsystems.ch está em registrador externo**:
1. Login no painel do registrador
2. Navegar para DNS/Host records
3. Adicionar glue records:
   - Host: ns1.rbxsystems.ch → IP: 158.220.116.31
   - Host: ns2.rbxsystems.ch → IP: 164.68.96.68

**Opção C - Se rbxsystems.ch não existe/não tem DNS**:
1. **PROBLEMA**: Precisa criar DNS para rbxsystems.ch primeiro
2. **OU**: Usar nameservers com domínio diferente

**Validar após criação**:
```bash
# Aguardar propagação (pode levar minutos a horas)
dig @8.8.8.8 ns1.rbxsystems.ch A +short
# Deve retornar: 158.220.116.31

dig @1.1.1.1 ns2.rbxsystems.ch A +short
# Deve retornar: 164.68.96.68
```

---

## 6. Acesso ao Cluster Kubernetes

### Problema Atual

```
kubectl get nodes
→ Error: connection refused to 127.0.0.1:6443
```

### Soluções

**A) SSH Port-Forward (temporário, para deploy único)**:
```bash
# Em um terminal, manter rodando:
ssh -L 6443:localhost:6443 root@158.220.116.31

# Em outro terminal:
export KUBECONFIG=/home/psyctl/.kube/config-rbx
kubectl get nodes  # Deve funcionar agora
```

**B) Modificar kubeconfig (permanente)**:
```bash
# Editar ~/.kube/config-rbx
# Trocar:
#   server: https://127.0.0.1:6443
# Por:
#   server: https://158.220.116.31:6443

# NOTA: Requer porta 6443 aberta no firewall de tiger
```

**C) Deploy Remoto (mais seguro)**:
```bash
# Copiar manifestos para master
scp -r infra/apps/dns root@158.220.116.31:/tmp/

# Conectar e deployar de lá
ssh root@158.220.116.31
cd /tmp/dns
kubectl apply -k overlays/nodeport/
```

---

## Checklist de Pré-Deploy

Antes de executar `kubectl apply`:

- [ ] PostgreSQL password gerado e configurado em `postgresql.yaml`
- [ ] PowerDNS API key gerado e configurado em `powerdns-config.yaml`
- [ ] IPs de strategos.gr configurados em `zone-init-job.yaml`
- [ ] Acesso SSH a tiger (158.220.116.31) funcional
- [ ] Acesso SSH a bengal (164.68.96.68) funcional
- [ ] Acesso kubectl ao cluster funcional (`kubectl get nodes` ok)
- [ ] Glue records verificados (ns1/ns2.rbxsystems.ch resolvem globalmente)
- [ ] Passwords salvos em local seguro (password manager)
- [ ] **Secrets NÃO commitados no Git** (verificar com `git diff`)

---

## Checklist Pós-Deploy

Após `kubectl apply`:

- [ ] Pods rodando: `kubectl get pods -n dns`
- [ ] Script iptables executado em tiger
- [ ] Script iptables executado em bengal
- [ ] Redirect funcionando: `dig @158.220.116.31 strategos.gr SOA` (porta 53)
- [ ] Zona inicializada: `kubectl logs -n dns job/zone-init`
- [ ] Authoritative flag: `dig @158.220.116.31 strategos.gr SOA | grep aa`
- [ ] Recursion desabilitada: `dig @158.220.116.31 google.com A` → REFUSED
- [ ] AXFR bloqueado: `dig @158.220.116.31 strategos.gr AXFR` → REFUSED
- [ ] TCP funcional: `dig +tcp @158.220.116.31 strategos.gr SOA`
- [ ] Glue records propagados globalmente
- [ ] Nameservers atualizados no Intername (strategos.gr)

---

## Comandos Rápidos de Referência

### Gerar Todos os Secrets de Uma Vez

```bash
cat > ~/generate-dns-secrets.sh <<'SCRIPT'
#!/bin/bash
echo "Generating DNS secrets..."
POSTGRES_PASSWORD=$(openssl rand -base64 32)
PDNS_API_KEY=$(openssl rand -base64 32)

echo ""
echo "================================"
echo "PostgreSQL Password:"
echo "$POSTGRES_PASSWORD"
echo ""
echo "PowerDNS API Key:"
echo "$PDNS_API_KEY"
echo "================================"
echo ""

# Save to file
cat > ~/dns-secrets-$(date +%Y%m%d-%H%M%S).txt <<EOF
# DNS Secrets - Generated $(date)
# KEEP SECURE - DO NOT COMMIT TO GIT

POSTGRES_PASSWORD=$POSTGRES_PASSWORD
PDNS_API_KEY=$PDNS_API_KEY

# Next steps:
# 1. Update infra/apps/dns/base/postgresql.yaml
# 2. Update infra/apps/dns/base/powerdns-config.yaml
# 3. Save this file to password manager
# 4. Delete this file after secrets are updated: rm ~/dns-secrets-*.txt
EOF

chmod 600 ~/dns-secrets-*.txt
echo "Secrets saved to: ~/dns-secrets-$(date +%Y%m%d-%H%M%S).txt"
echo "Remember to delete after updating manifests!"
SCRIPT

chmod +x ~/generate-dns-secrets.sh
~/generate-dns-secrets.sh
```

### Testar DNS Completo (Após Deploy)

```bash
cat > ~/test-dns.sh <<'SCRIPT'
#!/bin/bash
NS1="158.220.116.31"
NS2="164.68.96.68"

echo "Testing NS1 ($NS1)..."
dig @$NS1 strategos.gr SOA +short
dig @$NS1 strategos.gr NS +short

echo ""
echo "Testing NS2 ($NS2)..."
dig @$NS2 strategos.gr SOA +short
dig @$NS2 strategos.gr NS +short

echo ""
echo "Security tests..."
echo -n "Recursion (should fail): "
dig @$NS1 google.com A +short

echo -n "AXFR (should fail): "
dig @$NS1 strategos.gr AXFR | grep -q "Transfer failed" && echo "BLOCKED ✓" || echo "ALLOWED ✗"

echo ""
echo "Authoritative flag check..."
dig @$NS1 strategos.gr SOA | grep "flags:"
SCRIPT

chmod +x ~/test-dns.sh
```

---

## Avisos Finais

### ⚠️ NUNCA commitar secrets

Antes de fazer `git commit`:
```bash
git diff infra/apps/dns/base/ | grep -i "changeme"
# Se encontrar "CHANGEME", secrets ainda não foram substituídos
# Se não encontrar "CHANGEME" mas tem passwords em texto: PERIGO!
```

### ⚠️ Glue records são CRÍTICOS

Sem glue records corretos, o DNS de strategos.gr **NÃO FUNCIONARÁ** globalmente, mesmo que o PowerDNS esteja perfeito.

### ⚠️ iptables redirect é OBRIGATÓRIO

Sem redirect, DNS só funciona nas portas NodePort (30053, 31053), não na porta 53 padrão.

---

**Próximo passo**: Preencher secrets → Estabelecer acesso kubectl → Seguir README-NODEPORT.md
