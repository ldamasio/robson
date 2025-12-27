# ADR-0016: Multi-Plane Networking and Observability Architecture

## Status

**Accepted** (2024-12-26)

## Context

### Problem Statement

The Rust Stop Engine (ADR-0015) introduces critical financial infrastructure requiring **high availability, security, and auditability**. Traditional monolithic networking (single ingress, shared load balancer, mixed traffic) creates several risks:

1. **Blast Radius**: Compromise of UI WebSocket could expose critical execution infrastructure
2. **Observability Gaps**: No structured metrics/logging separation for critical vs. non-critical paths
3. **Operability**: Difficulty troubleshooting production issues without exposing sensitive admin interfaces
4. **Compliance**: Insufficient audit trails for financial operations
5. **Performance**: Mixed traffic contention between critical and non-critical workloads

### Threat Model

**Assets**:
- Stop-loss execution integrity (financial impact: potentially catastrophic)
- Client API keys stored in PostgreSQL (encrypted at rest)
- RabbitMQ message queue (contains trading commands)
- Prometheus metrics (may contain sensitive operational data)

**Threats**:
1. **T1**: Unauthorized access to RabbitMQ admin UI → Queue manipulation, message deletion
2. **T2**: DDoS on WebSocket endpoints → Execution plane degradation
3. **T3**: Lateral movement from compromised UI service → Critical infrastructure access
4. **T4**: Metrics scraping by unauthorized parties → Information disclosure
5. **T5**: AMQP protocol exposure → Direct queue access, bypassing application logic

**Mitigations** (this ADR):
- **M1**: RabbitMQ AMQP internal-only (NetworkPolicy enforcement)
- **M2**: Separate ingress for critical vs. non-critical traffic (IP allowlisting, rate limiting)
- **M3**: Least-privilege RabbitMQ users (vhost-scoped, AMQP vs. UI separation)
- **M4**: Observability plane isolation (dedicated subdomains, authentication)
- **M5**: TLS everywhere (cert-manager automation)

### Business Requirements

1. **Regulatory Compliance**: Audit trail for all stop-loss executions (SOC 2, financial regulations)
2. **Availability**: 99.9% uptime for critical execution path (RabbitMQ + Rust Stop Engine)
3. **Security**: Zero-trust networking, defense-in-depth
4. **Operability**: Real-time visibility into system health without exposing attack surface

---

## Decision

### Multi-Plane Architecture

We implement a **"service-mesh-lite"** approach with **6 distinct planes**, each with dedicated networking, authentication, and observability:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         MULTI-PLANE ARCHITECTURE                        │
└─────────────────────────────────────────────────────────────────────────┘

┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐
│   DATA PLANE    │  │ EXECUTION PLANE │  │  CONTROL PLANE  │
│   (PostgreSQL)  │  │ (Rust Stop Eng) │  │   (criticws)    │
│                 │  │                 │  │                 │
│  Internal-only  │  │  Internal-only  │  │  Hardened TLS   │
│  ClusterIP      │  │  ClusterIP      │  │  JWT/mTLS auth  │
│  Port: 5432     │  │  Consumes MQ    │  │  IP allowlist   │
│                 │  │  Writes to DB   │  │  Rate limited   │
└────────┬────────┘  └────────┬────────┘  └────────┬────────┘
         │                    │                    │
         │                    │                    │
         └────────────────────┴────────────────────┘
                              │
                    ┌─────────▼──────────┐
                    │    ADMIN PLANE     │
                    │     (RabbitMQ)     │
                    │                    │
                    │  AMQP: Internal    │
                    │  Port: 5672        │
                    │  NetworkPolicy ✓   │
                    │                    │
                    │  UI: Public        │
                    │  Port: 15672       │
                    │  BasicAuth ✓       │
                    │  IP allowlist ✓    │
                    │  TLS ✓             │
                    └─────────┬──────────┘
                              │
         ┌────────────────────┴────────────────────┐
         │                                         │
┌────────▼────────┐                  ┌─────────▼──────────┐
│   UI PLANE      │                  │ OBSERVABILITY PLANE│
│   (ws - Go WS)  │                  │ (Prometheus/Grafana)│
│                 │                  │                    │
│  Public TLS     │                  │  Hardened TLS      │
│  Ephemeral      │                  │  BasicAuth ✓       │
│  Can fail ✓     │                  │  IP allowlist ✓    │
│  ws.staging.*   │                  │  *.staging.rbx.*   │
└─────────────────┘                  └────────────────────┘
```

---

### Plane Definitions

#### 1. **DATA PLANE** (PostgreSQL)

**Purpose**: Durable state storage (event sourcing, projections, outbox)

**Network Configuration**:
- **Internal-only**: ClusterIP service
- **Port**: 5432 (PostgreSQL)
- **Access**: Application pods only (NetworkPolicy enforcement)
- **Encryption**: TLS connections (mutual TLS future consideration)

**Security**:
- Database credentials via Kubernetes Secrets (sealed-secrets in GitOps)
- Row-level security for multi-tenancy
- Connection pooling via PgBouncer (future)

**Observability**:
- Prometheus postgres_exporter (metrics)
- pgBadger for slow query analysis
- WAL archiving to S3 (PITR recovery)

---

#### 2. **EXECUTION PLANE** (Rust Stop Engine)

**Purpose**: Critical stop-loss order execution

**Network Configuration**:
- **Internal-only**: ClusterIP service
- **Ports**:
  - 9090 (Prometheus metrics)
  - No public ingress
- **Inbound**: Consumes from RabbitMQ (AMQP internal)
- **Outbound**:
  - PostgreSQL (event writes)
  - Binance REST API (order execution)
  - RabbitMQ (result events)

**Security**:
- Least-privilege RabbitMQ credentials (vhost-scoped, consume-only on `stop_commands.critical`)
- No secrets in environment variables (mounted from Secrets)
- Read-only filesystem (immutable pods)

**Observability**:
- **Metrics**: `/metrics` endpoint (Prometheus scrape)
  - `robson_stop_executions_total` (counter, by status)
  - `robson_stop_execution_latency_seconds` (histogram)
  - `robson_circuit_breaker_state` (gauge, by symbol)
- **Logging**: Structured JSON logs to stdout (Loki aggregation future)
- **Tracing**: OpenTelemetry (future)

**Availability**:
- Replicas: 2 (horizontal scaling)
- PodDisruptionBudget: minAvailable=1
- Health checks: liveness (HTTP /health), readiness (RabbitMQ connection)

---

#### 3. **CONTROL PLANE** (criticws - Rust WebSocket)

**Purpose**: Real-time visibility and control for critical operations (NOT durable backbone)

**Network Configuration**:
- **Public ingress**: `criticws.staging.rbx.ia.br` (staging), `criticws.rbx.ia.br` (prod)
- **Protocol**: WebSocket over TLS
- **Port**: 8080 (internal), 443 (external via Traefik)

**Security** (HARDENED):
- **TLS**: Required (cert-manager Let's Encrypt)
- **Authentication**:
  - JWT tokens (issued by backend API)
  - OR mutual TLS (client certificates) for admin users
  - Token validation on connection upgrade
- **Authorization**: Role-based access control (admin, operator, viewer)
- **IP Allowlist** (Traefik middleware):
  - Office IPs: `203.0.113.0/24` (example)
  - VPN IPs: `198.51.100.0/24` (example)
  - Deny all others
- **Rate Limiting** (Traefik middleware):
  - 100 connections/min per IP
  - 1000 messages/min per connection
- **NetworkPolicy**: Ingress from Traefik only, egress to RabbitMQ + Redis

**Functionality** (CRITICAL):
- **Read-only operations** (default):
  - Subscribe to stop events (from RabbitMQ `stop_events.notify` queue)
  - Real-time dashboard updates
  - System health status
- **Write operations** (privileged, audit-logged):
  - Kill switch activation (writes to Redis, audited to PostgreSQL)
  - Circuit breaker manual reset (admin-only)
  - Emergency stop-all (requires dual confirmation)

**Failure Mode**:
- If criticws is unavailable: **Execution continues via RabbitMQ**
- UI dashboards show stale data or connection error
- **NO impact on stop-loss execution**

**Observability**:
- Metrics: Active connections, message throughput, auth failures
- Alerts: Connection spikes, repeated auth failures (possible attack)

---

#### 4. **ADMIN PLANE** (RabbitMQ)

**Purpose**: Durable message broker for critical commands/events

**Network Configuration**:

**AMQP (5672) - INTERNAL ONLY**:
```yaml
# ClusterIP service
apiVersion: v1
kind: Service
metadata:
  name: rabbitmq-amqp
  namespace: robson
spec:
  type: ClusterIP  # ⚠️ NEVER LoadBalancer or NodePort
  selector:
    app: rabbitmq
  ports:
  - name: amqp
    port: 5672
    targetPort: 5672

# NetworkPolicy: Allow AMQP only from app pods
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: rabbitmq-amqp-internal-only
  namespace: robson
spec:
  podSelector:
    matchLabels:
      app: rabbitmq
  policyTypes:
  - Ingress
  ingress:
  - from:
    - podSelector:
        matchLabels:
          tier: backend  # Only backend pods (Rust, Python)
    ports:
    - protocol: TCP
      port: 5672
```

**Management UI (15672) - HARDENED PUBLIC**:
```yaml
# Ingress with Traefik middlewares
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: rabbitmq-management
  namespace: robson
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    traefik.ingress.kubernetes.io/router.middlewares: >-
      robson-rabbitmq-basicauth@kubernetescrd,
      robson-rabbitmq-ipallowlist@kubernetescrd,
      robson-ratelimit-admin@kubernetescrd
    traefik.ingress.kubernetes.io/router.tls: "true"
spec:
  ingressClassName: traefik
  tls:
  - hosts:
    - rabbitmq.staging.rbx.ia.br
    secretName: rabbitmq-management-tls
  rules:
  - host: rabbitmq.staging.rbx.ia.br
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: rabbitmq-management
            port:
              number: 15672
```

**Security**:
- **Users** (least privilege):
  ```
  Admin User (UI only):
    - Username: admin
    - Permissions: Administrator tag
    - Access: Management UI only (HTTP Basic Auth)
    - Vhosts: All

  App User (AMQP only):
    - Username: robson-app
    - Permissions: configure/write/read on /robson vhost
    - Access: AMQP only (no UI login)
    - Vhosts: /robson
    - Tags: monitoring (for prometheus plugin)
  ```
- **BasicAuth Middleware** (Traefik):
  - Credentials: `admin:$apr1$...` (bcrypt hash in Secret)
  - Stored in: `traefik-basicauth-secret`
- **IP Allowlist Middleware** (Traefik):
  - Office: `203.0.113.0/24`
  - VPN: `198.51.100.0/24`
  - Deny all others
- **Rate Limiting Middleware** (Traefik):
  - 60 requests/min per IP (prevent brute force)

**High Availability**:
- Replicas: 3 (quorum queues)
- Persistent volume: 10GB per replica
- Anti-affinity: Spread across nodes

**Observability**:
- **Prometheus Plugin**: Enabled (`rabbitmq_prometheus`)
  - Endpoint: `http://rabbitmq-management:15692/metrics`
  - Metrics: Queue depth, consumer count, message rates
- **Alerts**:
  - Queue depth > 1000 (backlog warning)
  - Consumer lag > 60s (processing delay)
  - No consumers on critical queue (execution engine down)

---

#### 5. **UI PLANE** (ws - Go WebSocket)

**Purpose**: Non-critical dashboard updates (ephemeral, can fail)

**Network Configuration**:
- **Public ingress**: `ws.staging.rbx.ia.br` (staging), `ws.robson.rbx.ia.br` (prod)
- **Protocol**: WebSocket over TLS
- **Port**: 8080 (internal), 443 (external via Traefik)

**Security** (MODERATE):
- **TLS**: Required (cert-manager Let's Encrypt)
- **Authentication**: JWT tokens (issued by backend API, less strict than criticws)
- **IP Allowlist**: None (public dashboard access)
- **Rate Limiting** (Traefik):
  - 500 connections/min per IP (higher than criticws, less critical)

**Failure Mode**:
- If ws is unavailable: **Dashboards show connection error**
- Users can refresh to reconnect
- **NO impact on stop-loss execution**

**Observability**:
- Metrics: Active connections, message throughput
- No critical alerts (non-critical service)

---

#### 6. **OBSERVABILITY PLANE** (Prometheus + Grafana)

**Purpose**: Centralized metrics aggregation and visualization

**Network Configuration**:

**Prometheus** (`prometheus.staging.rbx.ia.br`):
- **Ingress**: Hardened public (BasicAuth, IP allowlist)
- **Scrape Targets**:
  - RabbitMQ: `http://rabbitmq-management:15692/metrics`
  - Rust Stop Engine: `http://rust-stop-engine:9090/metrics`
  - criticws: `http://criticws:9091/metrics`
  - PostgreSQL: `http://postgres-exporter:9187/metrics`
  - Node Exporter: `http://<node-ip>:9100/metrics`

**Grafana** (`grafana.staging.rbx.ia.br`):
- **Ingress**: Hardened public (BasicAuth OR OAuth, IP allowlist)
- **Dashboards**:
  - **Critical Backbone Health** (ADR-0015 compliance):
    - RabbitMQ queue depth, consumer lag
    - Rust Stop Engine execution latency (P50, P95, P99)
    - Circuit breaker states by symbol
    - PostgreSQL transaction rate, connection pool
  - **System Overview**:
    - Node metrics (CPU, memory, disk)
    - Network traffic
    - Pod restarts

**Security**:
- **BasicAuth Middleware**: Same as RabbitMQ (admin credentials)
- **IP Allowlist**: Office + VPN IPs only
- **Data Retention**: 30 days (Prometheus), 90 days (Thanos long-term storage future)

**Alerts** (Prometheus Alertmanager):
```yaml
groups:
- name: critical_backbone
  interval: 30s
  rules:
  - alert: RabbitMQConsumerDown
    expr: rabbitmq_queue_consumers{queue="stop_commands.critical"} == 0
    for: 1m
    severity: critical
    annotations:
      summary: "No consumers on critical stop commands queue"

  - alert: StopExecutionLatencyHigh
    expr: histogram_quantile(0.95, robson_stop_execution_latency_seconds) > 1.0
    for: 5m
    severity: warning
    annotations:
      summary: "P95 stop execution latency > 1 second"

  - alert: CircuitBreakerOpen
    expr: robson_circuit_breaker_state == 2  # OPEN state
    for: 5m
    severity: warning
    annotations:
      summary: "Circuit breaker open for {{ $labels.symbol }}"
```

---

## Architecture Principles

### 1. **Defense in Depth**

Multiple layers of security:
```
Layer 1: Network (NetworkPolicy, ClusterIP internal-only)
Layer 2: Ingress (TLS, IP allowlist, rate limiting)
Layer 3: Authentication (JWT, BasicAuth, mTLS)
Layer 4: Authorization (RBAC, vhost-scoped RabbitMQ users)
Layer 5: Application (input validation, circuit breakers)
Layer 6: Data (encryption at rest, audit logging)
```

### 2. **Least Privilege**

- RabbitMQ app user: consume-only on critical queue, no admin
- Rust Stop Engine: read-only filesystem, no shell
- NetworkPolicies: deny-by-default, explicit allow rules

### 3. **Fail-Safe Defaults**

- If criticws fails: Execution continues via RabbitMQ
- If ws fails: UI shows error, execution continues
- If Prometheus fails: No metrics, but execution continues
- If RabbitMQ fails: Python CronJob backstop executes within 5 minutes

### 4. **Separation of Concerns**

- **Critical path** (RabbitMQ + Rust): Never shares infrastructure with non-critical
- **Admin plane** (RabbitMQ UI): Separate subdomain, hardened auth
- **Observability plane**: Read-only access, cannot affect execution

### 5. **Auditability**

Every action is logged:
- **PostgreSQL**: Audit trail of all stop executions (event sourcing)
- **RabbitMQ**: Message acknowledgements logged
- **criticws**: Admin actions (kill switch, circuit breaker) logged to DB
- **Prometheus**: Metrics for compliance reporting

---

## Implementation Roadmap

### Phase 1: Core Infrastructure (Week 2)

- [x] Create ADR-0016 (this document)
- [ ] Deploy RabbitMQ StatefulSet with NetworkPolicy
- [ ] Create Traefik middlewares (BasicAuth, IP allowlist, rate limiting)
- [ ] Configure RabbitMQ users (admin, robson-app)
- [ ] Enable rabbitmq_prometheus plugin
- [ ] Create RabbitMQ management ingress with hardening

### Phase 2: Execution Plane (Weeks 3-5)

- [ ] Deploy Rust Stop Engine (2 replicas)
- [ ] Configure AMQP connection (internal ClusterIP)
- [ ] Implement Prometheus metrics endpoint
- [ ] Create ServiceMonitor (Prometheus CRD)

### Phase 3: Control Plane (Week 5)

- [ ] Deploy criticws (Rust WebSocket)
- [ ] Implement JWT authentication
- [ ] Create criticws ingress with hardening (IP allowlist, rate limiting)
- [ ] Implement kill switch + audit logging

### Phase 4: Observability Plane (Week 6)

- [ ] Deploy Prometheus (Helm chart)
- [ ] Deploy Grafana (Helm chart)
- [ ] Create dashboards (Critical Backbone Health, System Overview)
- [ ] Configure Alertmanager rules
- [ ] Create ingresses with hardening

### Phase 5: Documentation (Week 8)

- [ ] Runbook: RabbitMQ operations (user management, queue inspection)
- [ ] Runbook: Rust Stop Engine troubleshooting
- [ ] Runbook: criticws operations
- [ ] Runbook: Prometheus/Grafana access
- [ ] Security review and penetration testing

---

## Consequences

### Positive

✅ **Security**:
- Reduced attack surface (AMQP internal-only, IP allowlists)
- Defense in depth (multiple security layers)
- Least privilege (scoped RabbitMQ users)

✅ **Reliability**:
- Failure isolation (UI plane failures don't affect execution)
- Observability for proactive issue detection
- Audit trail for compliance

✅ **Operability**:
- Dedicated admin interfaces (RabbitMQ UI, Grafana)
- Real-time visibility (Prometheus metrics, criticws)
- Structured troubleshooting (runbooks per plane)

✅ **Compliance**:
- Complete audit trail (event sourcing + metrics)
- IP-based access controls (SOC 2 compliance)
- Encrypted traffic (TLS everywhere)

### Negative

❌ **Complexity**:
- More Kubernetes manifests to maintain
- Multiple subdomains to manage (DNS, TLS certs)
- More middleware configuration (Traefik)

❌ **Operational Overhead**:
- Need to manage IP allowlists (office/VPN IP changes)
- BasicAuth credential rotation
- Prometheus storage management

❌ **Learning Curve**:
- Team must understand multi-plane architecture
- More troubleshooting paths (which plane is failing?)

### Risks and Mitigations

| Risk | Mitigation |
|------|-----------|
| IP allowlist blocks legitimate users | VPN access + runbook for emergency bypass |
| BasicAuth credentials leaked | Credential rotation every 90 days, audit logs |
| Prometheus storage exhaustion | 30-day retention, Thanos long-term storage |
| NetworkPolicy misconfiguration | Automated tests, staged rollout (staging → prod) |
| TLS certificate expiration | cert-manager auto-renewal, alerts 7 days before expiry |

---

## Alternatives Considered

### Alternative 1: Single Public Ingress (REJECTED)

**Approach**: Expose all services via single ingress (api.robson.rbx.ia.br/rabbitmq, api.robson.rbx.ia.br/ws)

**Rejected because**:
- Shared blast radius (compromise of one service affects all)
- No IP-based isolation (can't restrict RabbitMQ to office IPs only)
- Difficult to apply different rate limits per service
- Poor auditability (mixed logs)

### Alternative 2: Full Service Mesh (Istio/Linkerd) (DEFERRED)

**Approach**: Deploy full service mesh with mTLS, traffic splitting, observability

**Deferred because**:
- Overkill for current scale (10 services, 1 cluster)
- Operational complexity (need dedicated platform team)
- Can revisit when scale requires (100+ services, multi-cluster)
- Current "service-mesh-lite" approach provides 80% of benefits with 20% of complexity

### Alternative 3: VPN-Only Access (REJECTED)

**Approach**: All admin interfaces (RabbitMQ, Grafana) accessible only via VPN

**Rejected because**:
- Poor developer experience (must be on VPN to troubleshoot prod)
- VPN single point of failure (if VPN down, no prod access)
- IP allowlist + public ingress is sufficient for current threat model

---

## References

- [ADR-0015: Rust Stop Engine and RabbitMQ Architecture](ADR-0015-rust-stop-engine-rabbitmq.md)
- [ADR-0003: Istio Ambient + Gateway API](ADR-0003-istio-ambient-gateway-api.md) (future service mesh)
- [Traefik Middlewares Documentation](https://doc.traefik.io/traefik/middlewares/overview/)
- [Kubernetes NetworkPolicy Documentation](https://kubernetes.io/docs/concepts/services-networking/network-policies/)
- [RabbitMQ Access Control](https://www.rabbitmq.com/access-control.html)
- [Prometheus Best Practices](https://prometheus.io/docs/practices/)

---

**Last Updated**: 2024-12-26
**Authors**: Development Team + Claude Code
**Status**: Accepted - Implementation in progress
