# 🚀 START HERE - Quick Production Deployment

**Welcome!** This is your starting point for deploying Robson to production.

---

## 📌 Current Status (2024-12-20)

✅ **VPS Ready**: 4 fresh Ubuntu 24.04 installs  
✅ **SSH Access**: Root access confirmed on all nodes  
✅ **Network**: Connectivity validated  
⏳ **Next**: Create vault and start deployment  

---

## 📚 Documentation Map

### 🔥 **Quick Start** (Read these in order):

1. **[COMMANDS-QUICK-REFERENCE.md](COMMANDS-QUICK-REFERENCE.md)**  
   → **Copy-paste commands** for entire deployment

2. **[QUICK-DEPLOY-2024-12.md](QUICK-DEPLOY-2024-12.md)**  
   → **Complete guide** with explanations and troubleshooting

3. **[DEPLOYMENT-CHECKLIST.md](DEPLOYMENT-CHECKLIST.md)**  
   → **Track progress** through deployment phases

### 🔧 **Setup Templates**:

- `../../infra/ansible/VAULT-TEMPLATE.md` - How to create vault
- `../../infra/ansible/INVENTORY-TEMPLATE.md` - How to configure inventory
- `../../infra/ansible/VAULT-RESET-2024-12.md` - Context on vault reset

### 📖 **Background** (Optional):

- `INFRASTRUCTURE_DEPLOYMENT_PLAN.md` - Original detailed plan (F1-F6)
- `ANSIBLE_BOOTSTRAP_PLAN.md` - Ansible hardening details
- `TLS_CERT_MANAGER_HTTP01.md` - Certificate configuration

---

## ⚡ Quick Start (5 Steps)

### 1️⃣ Read the Command Reference

Open: [COMMANDS-QUICK-REFERENCE.md](COMMANDS-QUICK-REFERENCE.md)

This has all commands ready to copy-paste.

### 2️⃣ Create Vault

```bash
cd /c/app/notes/robson/infra/ansible

# Get your public key
cat ~/.ssh/id_ed25519.pub  # SAVE THIS

# Create vault (follow VAULT-TEMPLATE.md)
podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-vault create group_vars/all/vault.yml
```

### 3️⃣ Configure Inventory

Edit: `infra/ansible/inventory/contabo/hosts.ini`

Add your root passwords (see INVENTORY-TEMPLATE.md)

### 4️⃣ Test Connection

```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -v "$(pwd):/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping
```

### 5️⃣ Follow Commands

Open [COMMANDS-QUICK-REFERENCE.md](COMMANDS-QUICK-REFERENCE.md) and execute each step.

---

## ⏱️ Expected Timeline

| Phase | Time | What |
|-------|------|------|
| Phase 1 | 30 min | Ansible setup (vault, inventory) |
| Phase 2 | 1 hour | k3s installation |
| Phase 3 | 30 min | ArgoCD installation |
| Phase 4 | 1 hour | Application deployment |
| Phase 5 | 30 min | DNS configuration |
| **Total** | **~4 hours** | Production ready |

---

## 🎯 Success Criteria

You're done when:

- ✅ All 4 nodes show `Ready` in `kubectl get nodes`
- ✅ ArgoCD shows `Synced` and `Healthy`
- ✅ 3 pods running in `robson` namespace
- ✅ DNS resolves `api.robson.rbx.ia.br` → `158.220.116.31`
- ✅ HTTPS works with valid certificates
- ✅ Application accessible via browser

---

## 🆘 Need Help?

### Common Issues

**Ansible ping fails:**
- Check root passwords in `inventory/contabo/hosts.ini`
- Verify SSH connectivity: `ssh root@158.220.116.31`

**k3s agent won't join:**
- Verify token in vault: `ansible-vault view group_vars/all/vault.yml`
- Check server reachable: `ssh root@158.220.116.31 "systemctl status k3s"`

**Pods not starting:**
- Check image tags updated: `grep image: infra/k8s/prod/*.yml`
- Check secrets exist: `kubectl get secret -n robson`

**Certificates not Ready:**
- Verify DNS resolves: `dig +short api.robson.rbx.ia.br`
- Check cert-manager logs: `kubectl logs -n cert-manager deployment/cert-manager`

### Getting More Help

1. Check [QUICK-DEPLOY-2024-12.md](QUICK-DEPLOY-2024-12.md) troubleshooting section
2. Check ArgoCD UI: `kubectl port-forward svc/argocd-server -n argocd 8080:443`
3. Check pod logs: `kubectl logs -n robson <pod-name>`

---

## 📝 Important Notes

### Temporary Simplifications

This quick deployment **skips some security** for speed:

❌ **Not included** (add later):
- SSH port change (staying on 22)
- Root login disabled
- UFW firewall rules
- Istio Ambient mesh
- Monitoring (Prometheus/Grafana)
- Backup automation (Velero)

✅ **Included** (production minimum):
- k3s cluster (1 server + 3 agents)
- ArgoCD (GitOps)
- cert-manager (Let's Encrypt TLS)
- Gateway API (ingress)
- Application deployment

### Security Hardening Plan

**Week 1** (after production works):
- Change SSH to custom port
- Disable root login
- Create admin user with sudo
- Enable UFW firewall

**Week 2**:
- NetworkPolicies
- PodSecurity standards
- Sealed Secrets

**Week 3**:
- Monitoring stack
- Backup automation
- Disaster recovery plan

---

## 🔄 Continuing in a New Session

If you need to continue in a new session:

1. **Read**: `QUICK-DEPLOY-2024-12.md` to see full context
2. **Check**: `DEPLOYMENT-CHECKLIST.md` to see what's done
3. **Continue**: From the last unchecked step

All commands are in `COMMANDS-QUICK-REFERENCE.md`.

---

## 📦 What's in This Repository

```
docs/plan/infra/
├── START-HERE.md                    ← YOU ARE HERE
├── COMMANDS-QUICK-REFERENCE.md      ← Copy-paste commands
├── QUICK-DEPLOY-2024-12.md          ← Complete guide
├── DEPLOYMENT-CHECKLIST.md          ← Progress tracker
└── INFRASTRUCTURE_DEPLOYMENT_PLAN.md  ← Original detailed plan

infra/ansible/
├── VAULT-TEMPLATE.md                ← How to create vault
├── INVENTORY-TEMPLATE.md            ← How to configure inventory
├── VAULT-RESET-2024-12.md           ← Reset context
└── playbooks/
    └── k3s-simple-install.yml       ← k3s playbook
```

---

## 🎉 Ready to Start?

1. Open [COMMANDS-QUICK-REFERENCE.md](COMMANDS-QUICK-REFERENCE.md)
2. Start from **STEP 1: Prepare SSH Keys**
3. Follow each step in order
4. Check off progress in [DEPLOYMENT-CHECKLIST.md](DEPLOYMENT-CHECKLIST.md)

**Good luck!** 🚀

---

**Last Updated**: 2024-12-20  
**Maintainer**: Leandro Damásio  
**Estimated Time**: 4 hours to production
