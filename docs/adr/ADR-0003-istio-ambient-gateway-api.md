ADR-0003: Istio (Ambient Mode) with Gateway API

Status: Accepted
Date: 2025-09-14

Context
- We need mesh security (mTLS), traffic management, and modern ingress across preview and prod environments on k3s.
- Sidecar injection increases operational overhead. Gateway API is the Kubernetes-native way to manage ingress/egress.

Decision
- Use Istio in Ambient Mode (sidecarless) as the service mesh.
- Use Gateway API (GatewayClass `istio`, `Gateway`, `HTTPRoute`) for ingress instead of classic Ingress resources.
- Opt-in namespaces to Ambient with labels/annotations and use Waypoint proxies only where L7 policy is required.

Consequences
- Positive: reduced operational overhead vs sidecars; unified ingress model with Gateway API; native mTLS and policy.
- Trade-offs: Ambient Mode features differ from sidecar mode; requires ztunnel + CNI and cluster-wide install.

Implementation Notes
- Platform Helm installs: istio-base/istiod with Ambient, ztunnel DaemonSet, CNI.
- Gateway API CRDs; Gateway resources managed per environment via Helm charts.
- Enable mTLS by default; per-namespace opt-in to Ambient.
- Replace Ingress with Gateway/HTTPRoute in application charts.

Related
- ADR-0002 Hexagonal Architecture
- ADR-0004 GitOps Preview Environments (per-branch)
- docs/ARCHITECTURE.md; infra/README.md

