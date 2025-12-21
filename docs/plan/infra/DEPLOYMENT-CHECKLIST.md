# Deployment Checklist - Quick Deploy 2024-12

Track your progress through the deployment.

## ✅ Phase 1: Ansible Setup

- [ ] Cleaned SSH known_hosts
- [ ] Got SSH public key (`cat ~/.ssh/id_ed25519.pub`)
- [ ] Created new Ansible vault (`ansible-vault create`)
- [ ] Saved vault password securely
- [ ] Edited `inventory/contabo/hosts.ini` with root passwords
- [ ] Tested connectivity (`ansible all -m ping`)

## ✅ Phase 2: k3s Installation

- [ ] Installed k3s server on tiger
- [ ] Captured k3s token from server
- [ ] Added token to vault (`ansible-vault edit`)
- [ ] Installed k3s agents (bengal, pantera, eagle)
- [ ] Copied kubeconfig locally
- [ ] Verified all nodes Ready (`kubectl get nodes`)

## ✅ Phase 3: ArgoCD

- [ ] Created argocd namespace
- [ ] Installed ArgoCD
- [ ] Got admin password
- [ ] Accessed ArgoCD UI (optional)

## ✅ Phase 4: Applications

- [ ] Installed cert-manager
- [ ] Installed Gateway API CRDs
- [ ] Created robson namespace
- [ ] Created rbs-django-secret
- [ ] Updated image tags in manifests (sha-*)
- [ ] Applied ArgoCD Application
- [ ] Verified pods running

## ✅ Phase 5: DNS & TLS

- [ ] Configured DNS records at Registro.br
- [ ] Waited for DNS propagation
- [ ] Verified certificates Ready
- [ ] Tested HTTPS endpoints

## 🎉 Success

- [ ] All 4 nodes Ready
- [ ] ArgoCD synced and healthy
- [ ] All pods running
- [ ] DNS resolves correctly
- [ ] HTTPS works
- [ ] Application accessible

---

**Started**: ___________  
**Completed**: ___________  
**Total Time**: ___________
