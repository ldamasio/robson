# Windows/Cygwin + Podman: Complete Fix Guide

## 🔴 Problems Solved

This document explains the **3 fixes** needed to run Ansible via Podman on Windows/Cygwin.

---

## Issue 1: Cygwin Path Not Recognized

### ❌ Error
```
statfs /cygdrive/c/app/notes/robson/infra/ansible: no such file or directory
```

### 🔍 Root Cause
`$(pwd)` in Cygwin returns `/cygdrive/c/...` which Podman **doesn't understand**.

### ✅ Solution
Use explicit Windows-style path:

**Before (broken)**:
```bash
-v "$(pwd):/work"
```

**After (working)**:
```bash
-v "C:/app/notes/robson/infra/ansible:/work"
```

---

## Issue 2: Vault Password Not Provided

### ❌ Error
```
Attempting to decrypt but no vault secrets found
```

### 🔍 Root Cause
Ansible automatically loads `group_vars/all/vault.yml` (encrypted), but no password was provided.

### ✅ Solution
Add `--ask-vault-pass` to ALL Ansible commands:

```bash
ansible-playbook ... --ask-vault-pass
```

---

## Issue 3: SSH Host Key Checking

### ❌ Error
```
Using a SSH password instead of a key is not possible because Host Key checking is enabled
```

### 🔍 Root Cause
The Podman container has no `known_hosts` file with VPS fingerprints.

SSH refuses password auth when host is unknown (MITM protection).

### ✅ Solution
Disable host key checking via environment variable:

```bash
-e ANSIBLE_HOST_KEY_CHECKING=False
```

**Note**: Safe for bootstrap phase with fresh VPS installs.

---

## ✅ Complete Working Command

### Test Connectivity (ping)
```bash
cd /c/app/notes/robson/infra/ansible

podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --ask-vault-pass
```

### Install k3s (playbook)
```bash
podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --limit k3s_server
```

---

## 🧠 Key Takeaways

1. **Always use Windows paths** (`C:/...`) in Podman `-v` mounts on Windows/Cygwin
2. **Always add `-e ANSIBLE_HOST_KEY_CHECKING=False`** for fresh VPS
3. **Always add `--ask-vault-pass`** when `group_vars/all/vault.yml` exists
4. **Always add `--extra-vars "@passwords.yml"`** for VPS root password

---

## 📚 Related Documentation

- `docs/plan/infra/COMMANDS-QUICK-REFERENCE.md` → All corrected commands
- `SECURE-PASSWORDS.md` → Password management guide
- `docs/infra/K3S-CLUSTER-GUIDE.md` → Complete k3s cluster deployment guide

---

**Last Updated**: 2024-12-20 16:50  
**Status**: All 3 issues resolved ✅
