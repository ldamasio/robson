# Runbooks

**Operational procedures and playbooks for Robson Bot platform.**

## Purpose

Runbooks provide step-by-step instructions for:
- **Deployment**: Production and preview environment deployments
- **Troubleshooting**: Common issues and resolution steps
- **Monitoring**: Observability and alerting procedures
- **Incident Response**: Emergency procedures and escalation
- **Maintenance**: Routine operational tasks

## Structure

```
runbooks/
├── README.md
├── deployment.md           # Deployment procedures
├── troubleshooting.md      # Common issues and fixes
├── monitoring.md           # Monitoring and alerting
├── incident-response.md    # Emergency procedures
└── maintenance.md          # Routine tasks
```

## Runbook Format

Each runbook follows this structure:

```markdown
# [Procedure Name]

**Severity**: [Low | Medium | High | Critical]
**Time to Execute**: [Estimated minutes/hours]
**Required Access**: [K8s admin, DB access, etc.]

## Prerequisites

- Access to X
- Tool Y installed
- Permissions for Z

## Procedure

### Step 1: [Action]

[Detailed instructions]

**Command**:
\`\`\`bash
kubectl get pods -n robson
\`\`\`

**Expected Output**:
\`\`\`
NAME                    READY   STATUS    RESTARTS   AGE
robson-backend-xxx      1/1     Running   0          5m
\`\`\`

**If this fails**: [Recovery steps]

### Step 2: [Action]

...

## Validation

How to verify the procedure succeeded:
- [ ] Check 1
- [ ] Check 2

## Rollback

If something goes wrong:
1. Step 1
2. Step 2

## Related Documentation

- [ADR-XXXX](../adr/ADR-XXXX.md)
- [Spec](../specs/...)
```

## Quick Reference

### Deployment

| Scenario | Runbook | Time |
|----------|---------|------|
| Production deployment | [deployment.md](deployment.md#production) | 15 min |
| Preview environment | [deployment.md](deployment.md#preview) | 5 min |
| Rollback | [deployment.md](deployment.md#rollback) | 10 min |

### Troubleshooting

| Issue | Runbook | Priority |
|-------|---------|----------|
| Pod crash loop | [troubleshooting.md](troubleshooting.md#pod-crash) | High |
| Database connection | [troubleshooting.md](troubleshooting.md#database) | High |
| Slow response time | [troubleshooting.md](troubleshooting.md#performance) | Medium |

### Incidents

| Severity | Response Time | Runbook |
|----------|--------------|---------|
| **Critical**: Service down | 5 minutes | [incident-response.md](incident-response.md#critical) |
| **High**: Degraded performance | 15 minutes | [incident-response.md](incident-response.md#high) |
| **Medium**: Non-critical failure | 1 hour | [incident-response.md](incident-response.md#medium) |

## Best Practices

1. **Test Runbooks**: Validate procedures in staging
2. **Keep Updated**: Review quarterly, update after incidents
3. **Be Specific**: Include exact commands, not generic instructions
4. **Add Context**: Explain WHY, not just HOW
5. **Include Validation**: Always verify success
6. **Plan Rollback**: Every procedure needs undo steps

## Contributing

When creating a new runbook:

1. Use the template format above
2. Test in non-production first
3. Get peer review
4. Update this index
5. Link related ADRs/specs

## Emergency Contacts

| Role | Contact | Escalation |
|------|---------|------------|
| On-Call Engineer | [Slack: @oncall] | Immediate |
| Database Admin | [Email] | 15 min |
| Security Team | [Email] | 30 min |
| CTO | [Phone] | Major incidents only |

## Tools

- `kubectl` - Kubernetes CLI
- `helm` - Helm package manager
- `argocd` - GitOps controller
- `psql` - PostgreSQL client
- `gh` - GitHub CLI

## Monitoring Dashboards

- **Grafana**: https://grafana.example.com
- **ArgoCD**: https://argocd.example.com
- **Prometheus**: https://prometheus.example.com

## Status Page

Public status: https://status.robsonbot.com (when available)

---

**Note**: This directory is critical for operations. Keep it up to date!
