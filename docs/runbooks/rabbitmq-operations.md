# RabbitMQ Operations Runbook

**Service**: RabbitMQ Admin Plane
**Owner**: Platform Team
**Related ADRs**: [ADR-0015](../adr/ADR-0015-rust-stop-engine-rabbitmq.md), [ADR-0016](../adr/ADR-0016-networking-and-observability-architecture.md)
**Last Updated**: 2024-12-26

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Access](#access)
4. [Common Operations](#common-operations)
5. [Monitoring](#monitoring)
6. [Troubleshooting](#troubleshooting)
7. [Emergency Procedures](#emergency-procedures)
8. [Maintenance](#maintenance)

---

## Overview

### Purpose

RabbitMQ is the **Admin Plane** in the multi-plane architecture, serving as the durable message broker for:
- **Critical Path**: Stop-loss commands (Python → RabbitMQ → Rust Stop Engine)
- **Event Fan-out**: Execution results (Rust → RabbitMQ → Fanout Service → UI)

### SLA

- **Availability**: 99.9% (3 nines)
- **Durability**: Persistent queues with disk-backed storage
- **Latency**: P95 message delivery < 100ms (internal network)

### Dependencies

- **Upstream**: Python Outbox Publisher, Rust Stop Engine
- **Downstream**: PostgreSQL (event sourcing), Binance REST API
- **Infrastructure**: Kubernetes (StatefulSet), persistent volumes

---

## Architecture

### Network Topology

```
┌──────────────────────────────────────────────────────────────┐
│                    RABBITMQ ADMIN PLANE                       │
└──────────────────────────────────────────────────────────────┘

Internal Network (AMQP - Port 5672):
┌─────────────────┐        ┌─────────────────┐
│ Python Outbox   │──AMQP─>│   RabbitMQ      │
│ Publisher       │        │   (Internal)    │
└─────────────────┘        │                 │
                           │   ClusterIP     │
┌─────────────────┐        │   NetworkPolicy │
│ Rust Stop Eng.  │<─AMQP──│   Enforced ✓    │
│ (Consumer)      │        └─────────────────┘
└─────────────────┘

Public Network (Management UI - Port 15672):
┌──────────────────────────────────────────────────────────────┐
│  https://rabbitmq.staging.rbx.ia.br                          │
│  ┌──────────────────┐                                        │
│  │  Traefik Ingress │                                        │
│  │  ✓ TLS (Let's Encrypt)                                   │
│  │  ✓ BasicAuth                                              │
│  │  ✓ IP Allowlist                                           │
│  │  ✓ Rate Limiting (60 req/min)                             │
│  └────────┬─────────┘                                        │
│           │                                                   │
│  ┌────────▼─────────┐                                        │
│  │  RabbitMQ UI     │                                        │
│  │  Port: 15672     │                                        │
│  └──────────────────┘                                        │
└──────────────────────────────────────────────────────────────┘

Observability (Prometheus - Port 15692):
┌──────────────────────────────────────────────────────────────┐
│  http://rabbitmq-prometheus.staging.svc.cluster.local:15692  │
│  /metrics endpoint (Prometheus scrape)                       │
└──────────────────────────────────────────────────────────────┘
```

### Queues and Exchanges

**Exchanges**:
- `stop_commands` (topic): Inbound commands for execution
- `stop_commands.dlx` (fanout): Dead letter exchange for failed commands
- `stop_events` (topic): Outbound execution results

**Queues**:
- `stop_commands.critical`: Critical execution commands (consumed by Rust)
- `stop_commands.dlq`: Dead letter queue (manual intervention required)
- `stop_events.audit`: Audit trail (90-day retention)
- `stop_events.notify`: UI notifications (consumed by Fanout Service)

### Users

| User | Purpose | Permissions | Access |
|------|---------|-------------|--------|
| `admin` | Management UI, troubleshooting | Administrator (all vhosts) | UI only (BasicAuth) |
| `robson-app` | Application AMQP client | configure/write/read on `/robson` vhost | AMQP only (internal) |

---

## Access

### Management UI (Public - Hardened)

**URL**: https://rabbitmq.staging.rbx.ia.br (staging), https://rabbitmq.rbx.ia.br (prod)

**Authentication**:
1. **BasicAuth**: Username/password (stored in `rabbitmq-basicauth-secret`)
2. **IP Allowlist**: Office/VPN IPs only (configured in Traefik middleware)

**Steps**:
```bash
# 1. Ensure you're on office network or VPN
# 2. Open browser: https://rabbitmq.staging.rbx.ia.br
# 3. Enter BasicAuth credentials (admin:<password>)
# 4. Login with RabbitMQ admin credentials
```

**Credential Retrieval** (K8s cluster access required):
```bash
# Get BasicAuth credentials
kubectl -n staging get secret rabbitmq-basicauth-secret -o jsonpath='{.data.users}' | base64 -d

# Get RabbitMQ admin password
kubectl -n staging get secret rabbitmq-credentials -o jsonpath='{.data.admin-password}' | base64 -d
```

### AMQP (Internal-only - Application Access)

**Endpoint**: `rabbitmq-amqp.staging.svc.cluster.local:5672` (ClusterIP)

**NetworkPolicy**: Only pods with label `tier=backend` can connect

**Connection String**:
```python
# Python (pika)
import pika

credentials = pika.PlainCredentials('robson-app', '<app-password>')
connection = pika.BlockingConnection(
    pika.ConnectionParameters(
        host='rabbitmq-amqp.staging.svc.cluster.local',
        port=5672,
        virtual_host='/robson',
        credentials=credentials
    )
)
```

```rust
// Rust (lapin)
use lapin::{Connection, ConnectionProperties};

let addr = "amqp://robson-app:<password>@rabbitmq-amqp.staging.svc.cluster.local:5672/robson";
let conn = Connection::connect(addr, ConnectionProperties::default()).await?;
```

### Prometheus Metrics (Internal)

**Endpoint**: `http://rabbitmq-prometheus.staging.svc.cluster.local:15692/metrics`

**Scrape Configuration** (Prometheus):
```yaml
- job_name: 'rabbitmq'
  static_configs:
  - targets:
    - rabbitmq-prometheus.staging.svc.cluster.local:15692
  relabel_configs:
  - source_labels: [__address__]
    target_label: instance
    replacement: rabbitmq-staging
```

---

## Common Operations

### 1. Inspect Queue Depth

**Via Management UI**:
1. Login to https://rabbitmq.staging.rbx.ia.br
2. Navigate to **Queues** tab
3. Select vhost: `/robson`
4. Check columns: `Ready`, `Unacked`, `Total`

**Via CLI**:
```bash
# Exec into RabbitMQ pod
kubectl -n staging exec -it rabbitmq-0 -- bash

# List queues with message counts
rabbitmqadmin list queues vhost name messages messages_ready messages_unacknowledged

# Example output:
# +--------+---------------------------+----------+---------------+------------------------+
# | vhost  | name                      | messages | messages_ready| messages_unacknowledged|
# +--------+---------------------------+----------+---------------+------------------------+
# | /robson| stop_commands.critical    | 0        | 0             | 0                      |
# | /robson| stop_events.audit         | 1523     | 1523          | 0                      |
# +--------+---------------------------+----------+---------------+------------------------+
```

**Alert Condition**: Queue depth > 1000 (potential backlog)

---

### 2. Check Consumer Status

**Via Management UI**:
1. **Queues** tab → Select queue → **Consumers** section
2. Verify `Consumer tag`, `Channel`, `Prefetch count`

**Via CLI**:
```bash
rabbitmqadmin list consumers queue_name vhost channel_details.connection_name

# Expected: At least 1 consumer on stop_commands.critical (Rust Stop Engine)
# If 0 consumers: Rust Stop Engine is DOWN (CRITICAL ALERT)
```

**Alert Condition**: No consumers on `stop_commands.critical` for > 1 minute

---

### 3. Publish Test Message

**Via Management UI**:
1. **Queues** tab → Select `stop_commands.critical`
2. **Publish message** section
3. Payload:
   ```json
   {
     "command_id": "test-123",
     "correlation_id": "test:95000.00:1703520934123",
     "operation_id": 999,
     "symbol": "BTCUSDC",
     "side": "SELL",
     "quantity": "0.001"
   }
   ```
4. **Publish message**

**Verify**:
- Check Rust Stop Engine logs for consumption
- Check `stop_events.audit` for result event

---

### 4. Inspect Dead Letter Queue

**When**: Alert fires for messages in `stop_commands.dlq`

**Steps**:
1. Login to Management UI
2. **Queues** → `stop_commands.dlq`
3. **Get messages** → Ack mode: `Manual` → **Get Message(s)**
4. Inspect payload and `x-death` header (failure reason)

**Common Causes**:
- `x-max-retries` exceeded (3 retries)
- Invalid payload schema
- Rust Stop Engine guardrails rejection (kill switch, circuit breaker)

**Resolution**:
- Fix root cause (update payload, reset circuit breaker)
- Move message back to `stop_commands.critical` (republish)
- OR manually execute via Django admin

---

### 5. Purge Queue (Emergency)

**⚠️ CAUTION**: This deletes all messages. Use only in emergencies.

**Via Management UI**:
1. **Queues** → Select queue
2. **Purge Messages** button
3. Confirm

**Via CLI**:
```bash
rabbitmqadmin purge queue name=stop_events.audit vhost=/robson
```

**Use Case**: Clear test messages, reset staging environment

---

### 6. Check Connection Health

**Via Management UI**:
1. **Connections** tab
2. Verify `robson-app` connection from Rust Stop Engine
3. Check `Channels`, `State` (should be `running`)

**Via CLI**:
```bash
rabbitmqctl list_connections name user vhost state channels

# Expected output:
# Listing connections ...
# robson-app-rust   robson-app   /robson   running   1
# robson-app-python robson-app   /robson   running   1
```

**Alert Condition**: No connections from Rust Stop Engine for > 2 minutes

---

## Monitoring

### Key Metrics (Prometheus)

| Metric | Meaning | Alert Threshold |
|--------|---------|----------------|
| `rabbitmq_queue_messages_ready` | Messages waiting in queue | > 1000 (backlog) |
| `rabbitmq_queue_consumers` | Number of consumers | == 0 on critical queue |
| `rabbitmq_queue_messages_unacked` | Messages awaiting ack | > 100 (processing lag) |
| `rabbitmq_connections` | Active AMQP connections | < 2 (Rust + Python down) |
| `rabbitmq_channel_messages_unacked` | Unacked messages per channel | > 50 (slow consumer) |
| `rabbitmq_node_mem_used` | Memory usage | > 80% of limit |
| `rabbitmq_node_disk_free` | Disk free space | < 2GB (critical) |

### Grafana Dashboard

**Dashboard ID**: `10991` (RabbitMQ Overview)

**Panels**:
- Queue depth (time series)
- Message rates (pub/consume rates)
- Consumer count
- Connection count
- Node resource usage (CPU, memory, disk)

**URL**: https://grafana.staging.rbx.ia.br/d/rabbitmq-overview (post-deployment)

### Alerts (Alertmanager)

```yaml
groups:
- name: rabbitmq_critical
  interval: 30s
  rules:
  - alert: RabbitMQConsumerDown
    expr: rabbitmq_queue_consumers{queue="stop_commands.critical"} == 0
    for: 1m
    severity: critical
    annotations:
      summary: "No consumers on critical queue - Rust Stop Engine DOWN"
      runbook: "docs/runbooks/rabbitmq-operations.md#consumer-down"

  - alert: RabbitMQQueueBacklog
    expr: rabbitmq_queue_messages_ready{queue=~"stop_commands.*"} > 1000
    for: 5m
    severity: warning
    annotations:
      summary: "Queue backlog > 1000 messages"

  - alert: RabbitMQHighMemoryUsage
    expr: (rabbitmq_node_mem_used / rabbitmq_node_mem_limit) > 0.8
    for: 5m
    severity: warning
    annotations:
      summary: "RabbitMQ memory usage > 80%"
```

---

## Troubleshooting

### Consumer Down

**Symptoms**:
- Alert: `RabbitMQConsumerDown`
- No consumers on `stop_commands.critical`
- Messages piling up in queue

**Diagnosis**:
```bash
# Check Rust Stop Engine pod status
kubectl -n staging get pods -l app=rust-stop-engine

# Check logs
kubectl -n staging logs -l app=rust-stop-engine --tail=100

# Common errors:
# - "Connection refused" → RabbitMQ not reachable (NetworkPolicy issue)
# - "Authentication failed" → Wrong credentials (check Secret)
# - "Vhost /robson not found" → RabbitMQ definitions not loaded
```

**Resolution**:
1. Verify RabbitMQ pod is `Running`:
   ```bash
   kubectl -n staging get pods -l app=rabbitmq
   ```
2. Check NetworkPolicy allows backend pods:
   ```bash
   kubectl -n staging get networkpolicy rabbitmq-amqp-internal-only -o yaml
   ```
3. Restart Rust Stop Engine:
   ```bash
   kubectl -n staging rollout restart deployment rust-stop-engine
   ```
4. Verify consumer reconnects (check Management UI)

---

### Queue Backlog

**Symptoms**:
- Alert: `RabbitMQQueueBacklog`
- Messages accumulating in `stop_commands.critical`
- P95 execution latency increasing

**Diagnosis**:
```bash
# Check consumer prefetch count (should be 10-50)
rabbitmqadmin list consumers queue_name prefetch_count

# Check message processing rate
# (via Grafana: Message Delivery Rate panel)

# Check Rust Stop Engine processing time
# (via Prometheus: robson_stop_execution_latency_seconds)
```

**Causes**:
1. **Slow consumer**: Rust Stop Engine processing < publish rate
2. **Guardrails blocking**: Kill switch active, circuit breakers open
3. **Binance API throttling**: 429 errors from exchange

**Resolution**:
1. **Scale Rust Stop Engine**:
   ```bash
   kubectl -n staging scale deployment rust-stop-engine --replicas=3
   ```
2. **Check guardrails** (kill switch, circuit breakers):
   ```bash
   # Redis CLI (if kill switch is in Redis)
   kubectl -n staging exec -it redis-0 -- redis-cli
   > GET kill_switch:1  # Should be 0 (off)
   > KEYS circuit:state:*  # Check circuit breaker states
   ```
3. **Increase prefetch count** (Rust configuration):
   ```rust
   // In Rust Stop Engine config
   channel.basic_qos(50, false).await?;  // Increase from 10 to 50
   ```

---

### Memory Pressure

**Symptoms**:
- Alert: `RabbitMQHighMemoryUsage`
- `rabbitmq_node_mem_used` > 80% of limit
- Potential message loss (if disk alarm triggers)

**Diagnosis**:
```bash
# Check memory usage
rabbitmqctl status | grep memory

# Check queue memory usage
rabbitmqadmin list queues vhost name messages memory

# Check if memory alarm is active
rabbitmqctl list_alarms
```

**Causes**:
1. Large message payloads
2. Too many unacked messages
3. Queue backlog (messages stored in RAM)

**Resolution**:
1. **Increase memory limit** (Kubernetes):
   ```bash
   # Edit StatefulSet
   kubectl -n staging edit statefulset rabbitmq
   # Update: resources.limits.memory: "2Gi" → "4Gi"
   ```
2. **Purge non-critical queues**:
   ```bash
   rabbitmqadmin purge queue name=stop_events.notify vhost=/robson
   ```
3. **Reduce message TTL** (shorter retention):
   ```bash
   # Update queue x-message-ttl argument
   rabbitmqadmin declare queue name=stop_events.audit vhost=/robson \
     durable=true arguments='{"x-message-ttl":3600000}'  # 1 hour
   ```

---

### Disk Full

**Symptoms**:
- Alert: `RabbitMQDiskFree < 2GB`
- RabbitMQ refuses new messages
- `rabbitmq_node_disk_free_alarm` active

**Diagnosis**:
```bash
# Check disk usage
kubectl -n staging exec rabbitmq-0 -- df -h /var/lib/rabbitmq

# Check PVC size
kubectl -n staging get pvc rabbitmq-data-rabbitmq-0
```

**Resolution**:
1. **Expand PVC** (if storage class supports):
   ```bash
   kubectl -n staging patch pvc rabbitmq-data-rabbitmq-0 \
     -p '{"spec":{"resources":{"requests":{"storage":"20Gi"}}}}'
   ```
2. **Delete old messages** (audit queue):
   ```bash
   rabbitmqadmin purge queue name=stop_events.audit vhost=/robson
   ```
3. **Restart RabbitMQ** (reload disk free limit):
   ```bash
   kubectl -n staging rollout restart statefulset rabbitmq
   ```

---

## Emergency Procedures

### Emergency Shutdown (Critical Incident)

**When**: Security breach, data corruption, cascading failure

**Steps**:
1. **Stop all publishers** (Outbox Publisher):
   ```bash
   kubectl -n staging scale deployment outbox-publisher --replicas=0
   ```
2. **Stop all consumers** (Rust Stop Engine):
   ```bash
   kubectl -n staging scale deployment rust-stop-engine --replicas=0
   ```
3. **Activate kill switch** (if implemented):
   ```bash
   # Via criticws or Redis CLI
   redis-cli SET kill_switch:1 1  # Activate global kill switch
   ```
4. **Scale down RabbitMQ** (optional, extreme):
   ```bash
   kubectl -n staging scale statefulset rabbitmq --replicas=0
   ```
5. **Notify team** (Slack, PagerDuty)

**Python CronJob Backstop**: Will execute missed stops within 5 minutes

---

### Credential Rotation (Security Event)

**When**: Credential leak, regular rotation (every 90 days)

**Steps**:
1. **Generate new passwords**:
   ```bash
   # Generate strong password
   openssl rand -base64 32

   # Generate bcrypt hash for BasicAuth
   docker run --rm httpd:2.4-alpine htpasswd -nbBC 10 admin <new-password>
   ```

2. **Update Secrets**:
   ```bash
   # Create new secret YAML (use TEMPLATE)
   cp infra/k8s/staging/rabbitmq/rabbitmq-secrets-TEMPLATE.yaml \
      infra/k8s/staging/rabbitmq/rabbitmq-secrets.yaml

   # Edit with new passwords
   vim infra/k8s/staging/rabbitmq/rabbitmq-secrets.yaml

   # Create sealed secret
   kubeseal < rabbitmq-secrets.yaml > rabbitmq-secrets-sealed.yaml

   # Apply
   kubectl apply -f rabbitmq-secrets-sealed.yaml
   ```

3. **Restart RabbitMQ**:
   ```bash
   kubectl -n staging rollout restart statefulset rabbitmq
   ```

4. **Update application config**:
   - Update `robson-app` password in Rust Stop Engine config
   - Update Outbox Publisher config
   - Restart both services

---

## Maintenance

### Upgrade RabbitMQ Version

**Schedule**: Quarterly (staging), annually (production)

**Procedure**:
1. **Backup definitions**:
   ```bash
   # Export topology
   rabbitmqadmin export /tmp/definitions.json

   # Download from pod
   kubectl -n staging cp rabbitmq-0:/tmp/definitions.json ./rabbitmq-backup.json
   ```

2. **Update image tag** (StatefulSet):
   ```yaml
   image: rabbitmq:3.13-management-alpine  # 3.12 → 3.13
   ```

3. **Test in staging first**:
   ```bash
   kubectl -n staging apply -f infra/k8s/staging/rabbitmq/
   kubectl -n staging rollout status statefulset rabbitmq
   ```

4. **Verify**:
   - Login to Management UI
   - Check queues, exchanges, bindings
   - Publish test message
   - Verify consumer reconnects

5. **Rollback if needed**:
   ```bash
   kubectl -n staging rollout undo statefulset rabbitmq
   ```

---

### Clean Up Old Messages

**Schedule**: Weekly (automated via CronJob - future)

**Manual Procedure**:
```bash
# Purge audit queue (older than 90 days, manual export first)
rabbitmqadmin export /tmp/audit-backup.json --format=long_description

# Purge queue
rabbitmqadmin purge queue name=stop_events.audit vhost=/robson
```

---

## References

- [ADR-0015: Rust Stop Engine and RabbitMQ](../adr/ADR-0015-rust-stop-engine-rabbitmq.md)
- [ADR-0016: Networking and Observability](../adr/ADR-0016-networking-and-observability-architecture.md)
- [RabbitMQ Official Documentation](https://www.rabbitmq.com/documentation.html)
- [RabbitMQ Best Practices](https://www.rabbitmq.com/production-checklist.html)

---

**Last Updated**: 2024-12-26
**Maintained By**: Platform Team
**Review Cycle**: Quarterly
