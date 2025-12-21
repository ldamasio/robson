# Session State - 2024-12-20

## ✅ COMPLETED

1. ✅ CI/CD workflow updated (SHA tags, buildx cache)
2. ✅ ArgoCD Application created (robson-prod.yaml)
3. ✅ Documentation created (8 files)
4. ✅ 4 VPS reinstalled (Ubuntu 24.04 fresh)
5. ✅ SSH root access confirmed (port 22)
6. ✅ `passwords.yml` created (not in Git)
7. ✅ `vault.yml` created and encrypted
8. ✅ **STEP 4 COMPLETE: All 4 VPS responding to Ansible ping**

## 🔴 ISSUES RESOLVED

### Issue 1: Vault password not provided
**Error**: `Attempting to decrypt but no vault secrets found`  
**Solution**: Add `--ask-vault-pass` to ALL Ansible commands ✅

### Issue 2: Cygwin path not recognized by Podman
**Error**: `statfs /cygdrive/c/...: no such file or directory`  
**Solution**: Use Windows-style path `C:/app/notes/robson/...` instead of `$(pwd)` ✅

### Issue 3: SSH Host Key Checking blocking password auth
**Error**: `Using a SSH password instead of a key is not possible because Host Key checking is enabled`  
**Solution**: Add `-e ANSIBLE_HOST_KEY_CHECKING=False` to all Podman commands ✅

## ⚡ WORKING COMMANDS (Windows/Cygwin)

For Windows/Cygwin, use explicit path and disable host key checking:

```bash
cd /c/app/notes/robson/infra/ansible

# Test connectivity
podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --ask-vault-pass
```

**Notes**: 
- Cygwin's `$(pwd)` returns `/cygdrive/c/...` which Podman doesn't understand. Use Windows path `C:/...` directly.
- `-e ANSIBLE_HOST_KEY_CHECKING=False` is required because the container has no `known_hosts` file.

---

## 📋 NEXT STEPS (for new session)

### Next: STEP 5 - Install k3s Server

Open: `docs/plan/infra/COMMANDS-QUICK-REFERENCE.md` and run STEP 5.

**Important reminders**:
- Add `-e ANSIBLE_HOST_KEY_CHECKING=False` to ALL Podman commands
- Add `--ask-vault-pass` to ALL playbook commands
- Use Windows path `C:/app/notes/robson/...` instead of `$(pwd)`

---

## 📂 KEY FILES

| File | Purpose | Status |
|------|---------|--------|
| `docs/plan/infra/START-HERE.md` | Entry point | ✅ Ready |
| `docs/plan/infra/COMMANDS-QUICK-REFERENCE.md` | Command list | ✅ Ready |
| `infra/ansible/SECURE-PASSWORDS.md` | Password guide | ✅ Ready |
| `infra/ansible/inventory/contabo/passwords.yml` | VPS passwords | ✅ Created (not in Git) |
| `infra/ansible/group_vars/all/vault.yml` | Ansible vault | ⏳ Need to verify/recreate |

---

## 🎯 NEXT STEP

**STEP 5: Install k3s Server**

```bash
cd /c/app/notes/robson/infra/ansible

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

Expected: k3s server installed on `tiger` (158.220.116.31)

---

## 🆘 If You Forgot Vault Password

Delete and recreate:

```bash
rm group_vars/all/vault.yml
# Then follow STEP 2 in COMMANDS-QUICK-REFERENCE.md
```

---

**Last Updated**: 2024-12-20 16:45  
**Phase**: k3s Installation (Step 5)  
**Status**: ✅ Step 4 complete, ready for k3s server install
