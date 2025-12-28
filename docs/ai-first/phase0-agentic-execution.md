# Phase 0 Agentic Execution: Human-Governed AI Infrastructure Deployment

**Date**: 2025-12-28
**Session**: Deep Storage Architecture Phase 0
**Approach**: AI-assisted infrastructure execution with human governance

---

## Executive Summary

Phase 0 of Robson Deep Storage Architecture was successfully executed using an **agentic workflow** with Claude Code as the execution agent and human oversight providing governance guardrails. This approach combined AI automation with critical human intervention at key decision points.

**Outcome**: Bronze and Silver data pipelines operational end-to-end with validated Parquet data in S3-compatible object storage.

**Duration**: ~4 hours (including debugging and root cause analysis)

---

## Agentic Workflow Model

### Human-Agent Collaboration Pattern

```
┌─────────────────────────────────────────────────────────────┐
│                     HUMAN GOVERNANCE                         │
│  ┌───────────────┐    ┌───────────────┐    ┌──────────────┐│
│  │ Define Scope  │ →  │ Set Guardrails│ →  │ Verify Before││
│  │               │    │               │    │   Action     ││
│  └───────────────┘    └───────────────┘    └──────────────┘│
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                  AI EXECUTION AGENT                          │
│  ┌───────────────┐    ┌───────────────┐    ┌──────────────┐│
│  │ Execute Steps │ →  │ Debug Errors  │ →  │ Propose Fix  ││
│  │               │    │               │    │              ││
│  └───────────────┘    └───────────────┘    └──────────────┘│
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                   CRITICAL INTERVENTIONS                     │
│  (Human override when agent speculation detected)            │
└─────────────────────────────────────────────────────────────┘
```

### Key Governance Interventions

**1. NetworkPolicy Verification (CRITICAL)**

**Agent Error**: Claude concluded "k3s CNI does NOT support NetworkPolicy" without proof when S3 connections failed.

**Human Correction**:
> "STOP LOOPING. NO SPECULATION. You are claiming 'k3s NetworkPolicy evaluation bug'. That is not acceptable without proof."

**Required Investigation**:
- A) Confirm NetworkPolicy is enforced by the CNI
- B) Identify the exact pod that cannot reach S3
- C) Print the policies and verify selectors match the pod

**Actual Root Cause**: `namespaceSelector: {}` only matches cluster namespaces, NOT external Internet traffic. Solution: Remove `to:` section entirely.

**Lesson**: Agent must provide hard evidence before proposing architectural changes.

---

**2. Label Verification Before Action**

**Agent Action**: About to apply NetworkPolicy targeting namespace labels and pod selectors.

**Human Intervention**:
> "STOP-AND-VERIFY BEFORE APPLYING THIS NETWORKPOLICY. You MUST verify that these labels actually exist in the live cluster."

**Required Verification**:
```bash
# Verify namespace labels exist
kubectl get ns robson -o jsonpath='{.metadata.labels}'

# Verify pod labels exist
kubectl get pods -n robson -l app=rbs-paradedb

# Verify selectors match reality
kubectl get networkpolicy allow-django-outbox -n analytics-jobs -o yaml
```

**Lesson**: Never apply configuration without verifying target resources exist and labels match.

---

## Execution Phases

### Phase 1: Infrastructure Preparation (AI-Driven)

**Agent Actions**:
- Applied namespace manifests
- Created RBAC resources
- Applied NetworkPolicies (with human verification)
- Created Kubernetes secrets

**Human Role**: Verified secrets created correctly, checked namespace labels

### Phase 2: Job Execution (AI-Driven)

**Agent Actions**:
- Applied bronze ingestion job
- Streamed logs and monitored execution
- Identified and fixed 5 critical errors
- Applied silver transformation job
- Validated end-to-end data flow

**Human Role**: Reviewed error analysis, approved fixes, validated S3 output

### Phase 3: Root Cause Analysis (AI + Human)

**Agent Actions**:
- Documented each failure mode
- Identified root causes
- Proposed and tested fixes

**Human Role**: Corrected speculative conclusions, required evidence-based debugging

---

## Lessons Learned

### What Worked Well

1. **Parallel Error Discovery**: Agent identified multiple issues simultaneously (password, table name, NetworkingPolicy)
2. **Systematic Debugging**: Agent followed logs, checked configuration, tested connectivity methodically
3. **Documentation**: Agent maintained detailed record of all changes and root causes

### What Required Human Correction

1. **Speculative Conclusions**: Agent incorrectly blamed "k3s CNI bug" without evidence
2. **Verification Gaps**: Agent attempted to apply NetworkPolicy without verifying labels existed
3. **Looping Behavior**: Agent repeated same incorrect assumptions until human intervention

### Critical Success Factors

1. **Human-in-the-Loop**: All architectural decisions required human approval
2. **Evidence-Based Debugging**: Human demanded proof before accepting root cause claims
3. **Stop-and-Verify**: NetworkPolicy changes required label verification before application

---

## Guardrails Applied

### Pre-Execution Checks

```yaml
Guardrails:
  - Verify namespace labels exist
  - Verify pod selectors match live pods
  - Verify secrets created without escaping errors
  - Verify NetworkPolicy syntax before applying
```

### Evidence Requirements

```yaml
Evidence Standards:
  - No speculation without logs/error messages
  - Root cause requires reproducible test case
  - Fixes require validation on live cluster
  - Architectural claims require citation/documentation
```

### Stop Conditions

```yaml
Stop Conditions:
  - Agent claims "CNI bug" or "platform limitation"
  - Agent proposes removing security controls
  - Agent applies configuration without verification
  - Agent loops on same error without progress
```

---

## Recommendations for Future Sessions

### For Human Operators

1. **Set Clear Guardrails**: Define evidence requirements upfront
2. **Monitor for Speculation**: Watch for "probably", "likely", "might be" conclusions
3. **Require Verification**: Mandate label/selector checks before applying changes
4. **Stop Looping Early**: Intervene if agent repeats same diagnosis >2 times

### For AI Agents

1. **Provide Evidence**: Always include logs, error messages, config diffs
2. **Avoid Speculation**: State "unknown" rather than guessing root cause
3. **Verify Before Acting**: Check target resources exist before applying changes
4. **Document Assumptions**: Explicitly state what is proven vs. suspected

---

## Conclusion

Phase 0 demonstrated that **AI-assisted infrastructure execution with human governance** is effective when:

- ✅ Human sets clear scope and guardrails upfront
- ✅ Agent executes systematic debugging and fixes
- ✅ Human intervenes at critical decision points
- ✅ Agent provides evidence-based recommendations
- ✅ Human validates architectural changes

**Key Success Factor**: The combination of AI automation (speed, systematic execution) with human oversight (critical thinking, verification, governance) resulted in successful deployment with minimal risk.

---

**Related Documents**:
- **ADR-0013**: Deep Storage Architecture (Phase 0 evidence)
- **Session Checkpoint**: docs/sessions/phase0-deep-storage-execution.md
- **Runbook**: docs/runbooks/deep-storage.md

---

**Last Updated**: 2025-12-28
**Maintained by**: Leandro Damasio (ldamasio@gmail.com)
