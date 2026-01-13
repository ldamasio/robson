# ktop Operations Guide

**Unix/Linux top-inspired Kubernetes cluster monitoring tool**

## Overview

### What is ktop?

ktop is a Unix/Linux `top`-inspired utility designed for Kubernetes environments. It displays useful metrics information about nodes, pods, and other workload resources operating within your cluster. Unlike traditional monitoring dashboards, ktop provides a familiar terminal-based experience for real-time cluster observation.

**Key Features**:
- **Real-time monitoring** with continuously refreshed data
- **Hierarchical navigation** allowing exploration from cluster overview down through nodes, pods, and container logs
- **Zero installation burden** on clusters - runs locally via your kubeconfig
- **Compatible everywhere** kubectl operates, requiring no direct node access
- **Live log streaming** with filtering, timestamps, and full-screen capabilities
- **Flexible metrics sources** supporting Prometheus, Metrics-Server, or degraded operation
- **Single executable** requiring only a valid kubeconfig file

**Important**: ktop is an **operational monitoring tool**, not Infrastructure-as-Code. It is used for:
- Real-time cluster resource monitoring (CPU, memory, network)
- Quick health checks of nodes and pods
- Resource usage analysis and capacity planning
- Troubleshooting performance bottlenecks

ktop does NOT replace GitOps workflows. All permanent changes to the cluster must go through the standard GitOps pipeline (GitHub Actions -> ArgoCD -> k3s).

### Role in Robson Bot

Robson Bot's production deployments are fully automated via GitOps:
- **Source of Truth**: GitHub repository (manifests, Helm charts)
- **Deployment**: ArgoCD (App of Apps + ApplicationSet for per-branch previews)
- **Cluster**: k3s (4 Contabo VPS nodes)
- **Service Mesh**: Istio Ambient Mode + Gateway API

**ktop fits into this stack as**:
- A `top`-style resource monitor for quick cluster health assessment
- A lightweight alternative to K9s when you only need metrics and resource usage
- A capacity planning tool to identify resource-hungry workloads
- An observability tool complementing Prometheus/Grafana dashboards

**Golden Rule**: ktop is for observing cluster resources. GitOps remains the source of truth. Use ktop for monitoring and K9s for deeper inspection and debugging.

### ktop vs K9s

Both tools serve different purposes in the Robson Bot operations workflow:

| Aspect | ktop | K9s |
|--------|------|-----|
| **Primary Use** | Resource monitoring (CPU/Memory/Network) | Cluster management and debugging |
| **Interface** | `top`-style, metrics-focused | Full terminal UI with CRUD operations |
| **Navigation** | Hierarchical drill-down | Free-form resource exploration |
| **Log Viewing** | Integrated with live streaming | Full log tailing with filtering |
| **Editing Resources** | Read-only | Can edit/delete resources |
| **Best For** | Quick health checks, capacity planning | Debugging, log inspection, pod management |

**Recommendation**: Use ktop for quick resource monitoring and K9s for detailed debugging and cluster operations.

---

## Installation

ktop runs on your local machine (or a jump-box with cluster access). It does NOT need to be installed inside the cluster.

### Installation Methods

**kubectl plugin (via Krew)**:
```bash
kubectl krew install ktop
kubectl ktop
```

**Homebrew (macOS/Linux)**:
```bash
brew tap vladimirvivien/oss-tools
brew install ktop
```

**Go Install**:
```bash
go install github.com/vladimirvivien/ktop@latest
```

**Binary Download**:
- Download from [GitHub Releases](https://github.com/vladimirvivien/ktop/releases/latest)
- Extract binary and add to your PATH

**Verification**:
```bash
ktop --version
```

For detailed installation instructions, refer to the [official ktop repository](https://github.com/vladimirvivien/ktop).

---

## Configuration and Connection

### Prerequisites

ktop relies on a valid `kubeconfig` file to connect to the cluster. Before using ktop:

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

Once `kubectl` works, ktop will automatically use the same context.

### Metrics Sources

ktop supports multiple metrics sources for resource usage data:

1. **Metrics-Server** (recommended): Standard Kubernetes metrics API
2. **Prometheus**: If available in the cluster, ktop can query Prometheus
3. **Degraded Mode**: Without metrics infrastructure, ktop displays resource requests/limits instead of actual usage

To check if metrics-server is installed:
```bash
kubectl get deployment -n kube-system metrics-server
```

---

## Typical Workflows

### 1. Quick Cluster Health Check

**Objective**: Get an immediate overview of cluster resource usage.

**Steps**:
1. Launch ktop:
   ```bash
   ktop
   ```
2. Default view shows cluster-wide metrics: nodes, CPU, memory usage
3. View node-level breakdown of resources
4. Identify nodes approaching capacity limits
5. Press `q` to exit

**Tip**: Use ktop for daily health checks before starting development work.

### 2. Monitoring Node Resources

**Objective**: Check resource utilization across all cluster nodes.

**Steps**:
1. Launch ktop:
   ```bash
   ktop
   ```
2. Navigate to nodes view to see:
   - CPU usage percentage
   - Memory usage percentage
   - Pod count per node
   - Network I/O (if available)
3. Identify imbalanced nodes or nodes nearing capacity
4. Use this information to inform scaling decisions

**Note**: In Robson Bot's 4-node k3s cluster (Contabo VPS), monitor for even distribution of workloads.

### 3. Investigating Namespace Resource Usage

**Objective**: Check resource consumption in a specific namespace.

**Steps**:
1. Launch ktop scoped to a namespace:
   ```bash
   ktop --namespace robson
   ```
   Or use the Makefile helper:
   ```bash
   make ktop-ns NAMESPACE=robson
   ```
2. View pods running in the namespace
3. Check CPU and memory usage per pod
4. Identify resource-heavy pods that may need optimization
5. Navigate to container-level metrics for detailed breakdown

### 4. Monitoring Preview Environments

**Objective**: Check resource usage in a feature branch preview namespace.

**Context**: Robson uses ArgoCD ApplicationSet to create per-branch preview environments:
- Branch: `feature/new-order-flow`
- Normalized namespace: `h-feature-new-order-flow`

**Steps**:
1. Launch ktop for the preview namespace:
   ```bash
   make ktop-preview BRANCH=feature/new-order-flow
   ```
2. Monitor resource consumption during feature testing
3. Identify if the feature introduces resource-heavy operations
4. Compare with production namespace baseline

### 5. Capacity Planning

**Objective**: Analyze cluster capacity and plan for scaling.

**Steps**:
1. Launch ktop to view cluster-wide metrics
2. Note overall CPU and memory utilization percentages
3. Navigate to individual nodes to identify utilization patterns
4. Document peak usage times and resource hotspots
5. Use findings to inform:
   - Node scaling decisions (add/remove nodes)
   - Pod resource limits adjustments
   - Workload distribution optimization

---

## Safety and Policy Notes

### Read-Only Monitoring

ktop is a monitoring tool and does NOT modify cluster state:

1. **Observation Only**: ktop displays metrics; it cannot edit, delete, or create resources
2. **No GitOps Conflicts**: Since ktop is read-only, there's no risk of conflicting with ArgoCD reconciliation
3. **Safe for Production**: Use ktop freely in production environments without concerns about accidental changes

### Complementary Tools

Use ktop alongside other monitoring tools:

| Tool | Use Case |
|------|----------|
| **ktop** | Quick resource monitoring, capacity planning |
| **K9s** | Debugging, log inspection, pod management |
| **Prometheus/Grafana** | Historical metrics, alerting, dashboards |
| **ArgoCD UI** | GitOps state, sync status, deployment health |

---

## Quick Reference

### Common ktop Commands

| Key/Action | Description |
|------------|-------------|
| `ktop` | Launch with current kubeconfig context |
| `ktop --namespace <ns>` | Monitor specific namespace |
| `ktop --all-namespaces` | Monitor all namespaces |
| Navigation keys | Navigate through hierarchical views |
| `q` | Quit ktop |

### Robson-Specific Helpers

From the repository root, use these Make targets:

| Target | Usage | Description |
|--------|-------|-------------|
| `make ktop` | `make ktop` | Launch ktop with current kubeconfig context |
| `make ktop-ns` | `make ktop-ns NAMESPACE=<name>` | Launch ktop scoped to a specific namespace |
| `make ktop-preview` | `make ktop-preview BRANCH=<branch>` | Launch ktop for a preview environment (auto-computes `h-<branch>` namespace) |

Example:
```bash
make ktop-preview BRANCH=feature/stop-loss-orders
```

---

## Troubleshooting

### ktop Cannot Connect to Cluster

**Symptom**: ktop shows connection errors or cannot retrieve metrics.

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

**Symptom**: Resource usage columns show zeros or "N/A".

**Solution**:
1. Check if metrics-server is installed:
   ```bash
   kubectl get deployment -n kube-system metrics-server
   ```
2. If not installed, ktop will operate in degraded mode showing requests/limits
3. Install metrics-server for accurate resource metrics

### ktop Not Found

**Symptom**: `command not found: ktop` when running ktop commands.

**Solution**:
1. Verify ktop is installed:
   ```bash
   which ktop
   ```
2. If using kubectl plugin:
   ```bash
   kubectl ktop
   ```
3. Reinstall ktop using one of the installation methods above

---

## Best Practices

1. **Use ktop for Quick Checks**: Start your day with `make ktop` to assess cluster health
2. **Scope to Namespaces**: When investigating a specific app, use `make ktop-ns NAMESPACE=<name>`
3. **Combine with K9s**: Use ktop for metrics, then switch to K9s for deeper debugging
4. **Monitor Before/After Deployments**: Check resource usage before and after deployments to catch regressions
5. **Document Capacity Findings**: Record ktop observations in capacity planning documents

---

## Additional Resources

- **ktop Official Repository**: [github.com/vladimirvivien/ktop](https://github.com/vladimirvivien/ktop)
- **K9s Operations Guide**: [K9S-OPERATIONS.md](K9S-OPERATIONS.md) - For detailed cluster debugging
- **Robson Infra Docs**: [README.md](README.md) for GitOps, Ansible, and k3s setup
- **Metrics-Server**: [Kubernetes metrics-server](https://github.com/kubernetes-sigs/metrics-server) for accurate resource metrics

---

**Last Updated**: 2026-01-13
**Maintained by**: Robson Bot Infrastructure Team
