# K9s Operations Guide

**Terminal UI for Kubernetes cluster operations and debugging**

## Overview

### What is K9s?

K9s is a terminal-based UI for interacting with Kubernetes clusters. It provides a fast, keyboard-driven interface for inspecting and managing cluster resources without writing verbose kubectl commands.

**Important**: K9s is an **operational tool**, not Infrastructure-as-Code. It is used for:
- Real-time cluster inspection and debugging
- Log tailing and pod inspection
- Resource monitoring and troubleshooting
- Ad-hoc queries during development and operations

K9s does NOT replace GitOps workflows. All permanent changes to the cluster must go through the standard GitOps pipeline (GitHub Actions → ArgoCD → k3s).

### Role in Robson Bot

Robson Bot's production deployments are fully automated via GitOps:
- **Source of Truth**: GitHub repository (manifests, Helm charts)
- **Deployment**: ArgoCD (App of Apps + ApplicationSet for per-branch previews)
- **Cluster**: k3s (4 Contabo VPS nodes)
- **Service Mesh**: Istio Ambient Mode + Gateway API

**K9s fits into this stack as**:
- A read-mostly inspection tool for day-to-day operations
- A debugging interface for troubleshooting pods, logs, and resources
- A quick way to verify ArgoCD deployments and per-branch preview environments
- An interactive dashboard for observing cluster health

**Golden Rule**: K9s is for observing and manual debugging. GitOps remains the source of truth. Any manual changes made via K9s can (and will) be overwritten by ArgoCD reconciliation.

---

## Installation

K9s runs on your local machine (or a jump-box with cluster access). It does NOT need to be installed inside the cluster.

### General Installation

K9s is distributed as a standalone binary. Installation methods:

**Official releases**:
- Download from the K9s GitHub releases page
- Extract binary and add to your PATH

**Package managers**:
- **macOS (Homebrew)**: `brew install derailed/k9s/k9s`
- **Linux (popular distros)**: Check your package manager (apt, yum, pacman, etc.)
- **Windows**: Use Chocolatey, Scoop, or download binary directly

**Verification**:
```bash
k9s version
```

For detailed installation instructions, refer to the official K9s documentation.

---

## Configuration and Connection

### Prerequisites

K9s relies on a valid `kubeconfig` file to connect to the cluster. Before using K9s:

1. **Obtain kubeconfig from the k3s cluster**:
   - Typically retrieved from `/etc/rancher/k3s/k3s.yaml` on the k3s server node
   - You may use Ansible to copy it, or manually scp it to your local machine

2. **Set the KUBECONFIG environment variable** (if not using the default `~/.kube/config`):
   ```bash
   export KUBECONFIG=/path/to/robson-k3s.yaml
   ```

3. **Verify cluster access**:
   ```bash
   kubectl cluster-info
   kubectl get nodes
   ```

Once `kubectl` works, K9s will automatically use the same context.

### Integration with Robson's k3s + ArgoCD + Istio Stack

K9s allows you to inspect:
- **Nodes**: Verify all 4 Contabo VPS nodes are healthy
- **Namespaces**: See all application namespaces and per-branch preview namespaces (`h-<branch>`)
- **Pods**: Inspect backend, frontend, ArgoCD, Istio components (istiod, ztunnel, Gateway)
- **Services**: View LoadBalancer/ClusterIP services and their endpoints
- **Deployments/StatefulSets**: Check replica counts and rollout status
- **ConfigMaps/Secrets**: Inspect (view only, do not edit in production)
- **Gateway API resources**: View Gateway and HTTPRoute objects managed by Helm charts

K9s does not modify GitOps state. It shows you what ArgoCD has deployed, and allows you to debug live resources.

---

## Typical Workflows

### 1. Checking Overall Cluster Health

**Objective**: Quickly verify nodes, namespaces, and key workloads are healthy.

**Steps**:
1. Launch K9s:
   ```bash
   k9s
   ```
2. Default view shows **Pods**. Press `:nodes` to view node status.
3. Press `:namespaces` to list all namespaces. Look for:
   - `argocd` (GitOps controller)
   - `istio-system` (service mesh components)
   - Application namespaces (backend, frontend, etc.)
   - Preview namespaces (`h-<branch>`)
4. Press `0` (zero) to show pods in all namespaces.
5. Check for CrashLoopBackOff, ImagePullBackOff, or Pending pods.

**Tip**: Use `/` to filter by resource name or namespace.

### 2. Inspecting Application Namespaces

**Objective**: Drill into a specific application namespace to check pods, logs, and resource usage.

**Steps**:
1. Launch K9s (optionally scoped to a namespace):
   ```bash
   k9s -n <namespace>
   ```
   Example: `k9s -n robson-backend`
2. View pods in the namespace (default view).
3. Highlight a pod and press `l` to tail logs in real-time.
4. Press `d` to describe the pod (view events, volumes, etc.).
5. Press `s` to drop into a shell inside the pod (use with caution; prefer logs for debugging).
6. Press `esc` to go back.

**Tip**: K9s supports vim-style navigation (`j`/`k` to move up/down, `/` to search).

### 3. Inspecting Per-Branch Preview Environments

**Objective**: Verify a feature branch deployment in its dedicated preview namespace.

**Context**: Robson uses ArgoCD ApplicationSet to create per-branch preview environments:
- Branch: `feature/new-order-flow`
- Normalized namespace: `h-feature-new-order-flow`
- Host: `h-feature-new-order-flow.robson.rbx.ia.br`

**Steps**:
1. Launch K9s scoped to the preview namespace:
   ```bash
   k9s -n h-feature-new-order-flow
   ```
2. Check pods are running (backend, frontend, etc.).
3. Press `l` on a pod to tail logs and verify the application is serving requests.
4. Press `:svc` to view services.
5. Press `:httproutes` (if Gateway API CRD is installed) to verify routing configuration.
6. Exit with `:quit` or `Ctrl+C`.

**Tip**: Use the `make k9s-preview BRANCH=feature/new-order-flow` helper (see Makefile) to automatically compute the namespace.

### 4. Tailing Logs While Testing a Feature Branch

**Objective**: Monitor logs in real-time while running integration tests or manual testing against a preview environment.

**Steps**:
1. Launch K9s in the preview namespace:
   ```bash
   make k9s-preview BRANCH=<branch-name>
   ```
2. Highlight the backend pod.
3. Press `l` to tail logs.
4. In another terminal, run tests or make API requests to the preview host.
5. Observe logs in K9s in real-time to debug issues.
6. Press `esc` to stop tailing and return to the pod list.

**Advanced**: K9s allows filtering logs by regex. After pressing `l`, press `/` to enter a filter pattern.

### 5. Viewing Basic Resource Usage

**Objective**: Check CPU and memory usage of pods (requires metrics-server installed in the cluster).

**Steps**:
1. Launch K9s:
   ```bash
   k9s
   ```
2. Navigate to pods (default view or `:pods`).
3. If metrics-server is installed, K9s will display CPU/Memory columns.
4. Press `:nodes` to view node-level resource usage.
5. Identify resource-hungry pods or nodes approaching capacity.

**Note**: If metrics are not visible, ensure metrics-server is deployed in the cluster (typically in the `kube-system` namespace).

---

## Safety and Policy Notes

### Read-Mostly, Debug-Only

K9s is powerful, but **must be used responsibly**:

1. **Inspection First**: Use K9s primarily for viewing resources, logs, and metrics.
2. **Manual Changes are Ephemeral**: Any manual edit (e.g., scaling a Deployment, deleting a pod) will be reverted by ArgoCD on the next sync.
3. **GitOps is the Source of Truth**: Permanent changes MUST be made via:
   - Updating Helm chart values in the repository
   - Modifying Kubernetes manifests
   - Merging a PR that triggers GitOps reconciliation

### When Manual Actions Are Acceptable

- **Restarting a pod**: Delete a CrashLoopBackOff pod to trigger a restart (ArgoCD will recreate it).
- **Testing a temporary fix**: Scale a Deployment to 0 and back to test a hypothesis (but revert via GitOps).
- **Emergency troubleshooting**: Exec into a pod to inspect filesystem or test network connectivity (read-only operations).

### When Manual Actions Are NOT Acceptable

- **Editing production ConfigMaps/Secrets**: Always update via GitOps (SealedSecrets, SOPS, or Ansible Vault).
- **Scaling production workloads permanently**: Update `replicaCount` in Helm chart values.
- **Changing resource limits**: Update `resources` in Helm chart values.
- **Modifying Gateway/HTTPRoute objects**: Update chart templates and let ArgoCD apply.

**Remember**: ArgoCD will reconcile changes. If you make a manual edit, it will be overwritten. Always follow up with a GitOps commit.

---

## Quick Reference

### Common K9s Commands

| Key | Action |
|-----|--------|
| `:pods` | View pods |
| `:svc` | View services |
| `:deploy` | View deployments |
| `:ns` | View namespaces |
| `:nodes` | View nodes |
| `0` | Show all namespaces |
| `/` | Filter by name |
| `l` | Tail logs |
| `d` | Describe resource |
| `e` | Edit resource (use with caution) |
| `s` | Shell into pod |
| `Ctrl+D` | Delete resource (use with extreme caution) |
| `esc` | Go back |
| `:quit` | Exit K9s |

### Robson-Specific Helpers

From the repository root, use these Make targets:

| Target | Usage | Description |
|--------|-------|-------------|
| `make k9s` | `make k9s` | Launch K9s with current kubeconfig context |
| `make k9s-ns` | `make k9s-ns NAMESPACE=<name>` | Launch K9s scoped to a specific namespace |
| `make k9s-preview` | `make k9s-preview BRANCH=<branch>` | Launch K9s for a preview environment (auto-computes `h-<branch>` namespace) |

Example:
```bash
make k9s-preview BRANCH=feature/stop-loss-orders
```

---

## Troubleshooting

### K9s Cannot Connect to Cluster

**Symptom**: K9s shows "Unable to connect to cluster" or similar error.

**Solution**:
1. Verify `kubectl` works:
   ```bash
   kubectl cluster-info
   ```
2. Check KUBECONFIG is set:
   ```bash
   echo $KUBECONFIG
   ```
3. Ensure kubeconfig has valid credentials and the cluster IP is reachable.

### No Metrics Displayed

**Symptom**: CPU/Memory columns are empty in K9s.

**Solution**:
1. Check if metrics-server is installed:
   ```bash
   kubectl get deployment -n kube-system metrics-server
   ```
2. If not installed, deploy metrics-server via Helm or kubectl.

### Cannot View Gateway API Resources

**Symptom**: `:gateways` or `:httproutes` show "resource not found".

**Solution**:
1. Verify Gateway API CRDs are installed:
   ```bash
   kubectl get crd gateways.gateway.networking.k8s.io
   ```
2. If missing, install Gateway API CRDs (see infra/README.md for Istio Ambient setup).

---

## Best Practices

1. **Use K9s for Inspection, Not Administration**: Prefer `kubectl` or GitOps for scripted changes.
2. **Scope to Namespaces**: When debugging a specific app, launch K9s with `-n <namespace>` to reduce clutter.
3. **Filter Aggressively**: Use `/` to filter by pod name, label, or status (e.g., `/CrashLoop`).
4. **Tail Logs, Don't Exec**: Prefer `l` (logs) over `s` (shell) for debugging. Logs are non-intrusive.
5. **Document Manual Changes**: If you make a manual change in K9s during an incident, document it in the incident report and follow up with a GitOps PR.

---

## Additional Resources

- **K9s Official Documentation**: Refer to the official K9s GitHub repository for advanced features and configuration.
- **Robson Infra Docs**: [infra/README.md](README.md) for GitOps, Ansible, and k3s setup.
- **ArgoCD Console**: Use ArgoCD UI for app-level sync status and GitOps state comparison.
- **Istio Debugging**: Refer to Istio documentation for Ambient Mode troubleshooting (sidecarless mesh).

---

**Last Updated**: 2025-12-08
**Maintained by**: Robson Bot Infrastructure Team
