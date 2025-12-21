# Secure Password Management Guide

## ⚠️ SECURITY ISSUE

Putting passwords in `hosts.ini` is dangerous because you might accidentally commit them.

## ✅ SAFE SOLUTION

We use a **separate passwords file** that is NOT tracked by Git.

---

## Setup (One Time)

### 1. Create your passwords file

```bash
cd /c/app/notes/robson/infra/ansible

# Copy template
cp inventory/contabo/passwords.yml.template \
   inventory/contabo/passwords.yml

# Edit with your REAL passwords
nano inventory/contabo/passwords.yml
# or
code inventory/contabo/passwords.yml
```

Edit and replace:
```yaml
---
# Single password for all 4 VPS
ansible_ssh_pass: "YOUR_REAL_VPS_PASSWORD_HERE"
```

**Example** (with fake password):
```yaml
---
ansible_ssh_pass: "M9jK3nP8qL2x"
```

### 2. Verify it's ignored by Git

```bash
git status

# passwords.yml should NOT appear (it's in .gitignore)
```

---

## Usage

### When running Ansible commands, add: `--extra-vars`

**BEFORE** (unsafe):
```bash
ansible -i inventory/contabo/hosts.ini all -m ping
```

**AFTER** (safe):
```bash
ansible -i inventory/contabo/hosts.ini all -m ping \
  --extra-vars "@inventory/contabo/passwords.yml"
```

**With Podman**:
```bash
podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible -i inventory/contabo/hosts.ini all -m ping \
  --extra-vars "@inventory/contabo/passwords.yml"
```

**Note**: `-e ANSIBLE_HOST_KEY_CHECKING=False` is required for fresh VPS installs.

---

## Updated Commands (STEP 4 onwards)

### STEP 4: Test Connectivity

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

### STEP 5: Install k3s Server

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

### STEP 8: Install k3s Agents

```bash
podman run --rm -it \
  -e ANSIBLE_HOST_KEY_CHECKING=False \
  -v "C:/app/notes/robson/infra/ansible:/work" -w /work \
  docker.io/alpine/ansible:latest \
  ansible-playbook -i inventory/contabo/hosts.ini \
  playbooks/k3s-simple-install.yml \
  --ask-vault-pass \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --limit k3s_agent
```

---

## Why This is Safe

✅ **passwords.yml** is in `.gitignore` → Can't accidentally commit  
✅ **hosts.ini** has NO passwords → Safe to commit  
✅ **Template** exists for reference → Easy to recreate  
✅ **Same functionality** → Just add `--extra-vars`  

---

## Backup Your Passwords

Store `passwords.yml` securely:

1. **Copy to password manager** (1Password, KeePass)
2. **Encrypted backup**: 
   ```bash
   # Encrypt with GPG
   gpg -c inventory/contabo/passwords.yml
   # Creates: passwords.yml.gpg (safe to store anywhere)
   ```

---

## Verification

```bash
# Check what will be committed
git status

# passwords.yml should NOT be listed
# If it appears, DON'T COMMIT!
```

---

## OPTION 2: Even More Secure (Ansible Vault)

If you want maximum security, encrypt the passwords file:

```bash
# Encrypt passwords.yml with Ansible Vault
ansible-vault encrypt inventory/contabo/passwords.yml

# Use it (asks for vault password)
ansible-playbook ... \
  --extra-vars "@inventory/contabo/passwords.yml" \
  --ask-vault-pass
```

This way even if `passwords.yml` leaks, it's encrypted!
