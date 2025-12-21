# kubectl Setup Guide (Windows/Cygwin/Git Bash)

## 🎯 Quick Setup

This guide shows how to use `kubectl` on Windows via Podman (no local kubectl installation needed).

---

## ✅ Prerequisites

- ✅ Podman Desktop installed
- ✅ Cygwin or Git Bash
- ✅ Kubeconfig file at `C:/app/notes/kubeconfig`

---

## 📝 One-Time Setup

### 1. Get kubeconfig from k3s server

```bash
# Create directory
mkdir -p ~/.kube

# Copy kubeconfig
scp root@158.220.116.31:/etc/rancher/k3s/k3s.yaml ~/.kube/config-robson

# Fix server IP
sed -i 's/127.0.0.1/158.220.116.31/' ~/.kube/config-robson

# Copy to accessible location (outside Git repo)
cp ~/.kube/config-robson /cygdrive/c/app/notes/kubeconfig
```

### 2. Configure alias

Add to `~/.bashrc` (works in both Cygwin and Git Bash):

```bash
# kubectl via Podman (Robson Production Cluster)
alias kubectl='podman run --rm -it -v "C:/app/notes/kubeconfig:/kubeconfig:ro" -e KUBECONFIG=/kubeconfig docker.io/bitnami/kubectl:latest'
```

### 3. Activate alias

```bash
source ~/.bashrc
```

---

## 🚀 Usage

```bash
# Get nodes
kubectl get nodes

# Get pods in all namespaces
kubectl get pods -A

# Get services
kubectl get svc -n robson

# Apply manifest
kubectl apply -f deployment.yaml

# View logs
kubectl logs -n robson deployment/backend

# Exec into pod
kubectl exec -it -n robson deployment/backend -- bash
```

---

## 🔧 Troubleshooting

### Error: "connection refused"

Check if kubeconfig has correct server IP:

```bash
grep "server:" /cygdrive/c/app/notes/kubeconfig
```

Should show: `server: https://158.220.116.31:6443`

If not:
```bash
sed -i 's/127.0.0.1/158.220.116.31/g' /cygdrive/c/app/notes/kubeconfig
```

### Error: "no such file or directory"

Podman path issue. Verify file exists:

```bash
ls -la /cygdrive/c/app/notes/kubeconfig
```

### Alias not working

Reload bashrc:
```bash
source ~/.bashrc
```

---

## 🧠 Why This Approach?

✅ **No local kubectl install needed** - runs in container  
✅ **Consistent across Windows/Linux** - same Podman everywhere  
✅ **Matches Ansible approach** - all tools via containers  
✅ **Works with Podman Desktop** - leverages existing setup  

---

## 📚 Related Files

- `C:/app/notes/kubeconfig` - Main kubeconfig file (not in Git)
- `SESSION-STATE.md` - Deployment status
- `COMMANDS-QUICK-REFERENCE.md` - All deployment commands

---

**Last Updated**: 2024-12-21  
**Cluster**: Robson Production (4 nodes k3s)
