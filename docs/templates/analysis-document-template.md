# [Title] — Analysis & Execution Plan

**Date**: YYYY-MM-DD  
**Author**: [Name/Agent]  
**Status**: Draft | Final  
**Related**: [ADRs, MIGs, specs, etc.]

---

## Executive Summary

**Problem Statement**: [1-2 sentences describing what triggered this analysis]

**Key Findings**:
- [Finding 1]
- [Finding 2]
- [Finding 3]

**Recommended Action**: [Primary path forward]

**Estimated Effort**: [Total time/complexity]

---

## Current State

### System Overview
[Brief description of relevant system components, architecture, current behavior]

### Observed Behavior
[What is currently happening — logs, metrics, user reports, test results]

### Expected Behavior
[What should be happening according to specs/requirements]

### Root Cause Analysis
[If applicable — why the gap exists]

---

## Gaps

### Documentation Gaps

| Priority | File/Location | Issue | Impact |
|----------|---------------|-------|--------|
| P0       | path/to/file  | Description | HIGH/MED/LOW |
| P1       | path/to/file  | Description | HIGH/MED/LOW |

### Code Gaps

| Priority | Component | Issue | Blocker For |
|----------|-----------|-------|-------------|
| P0       | module/file:line | Description | [Feature/Milestone] |
| P1       | module/file:line | Description | [Feature/Milestone] |

### Infrastructure Gaps

| Priority | Resource | Issue | Impact |
|----------|----------|-------|--------|
| P0       | service/config | Description | HIGH/MED/LOW |

---

## Priority Tracks

### Track 1: [Name] — [Objective]
**Effort**: [hours/days]  
**Dependencies**: [Prerequisites]  
**Deliverables**:
- [Deliverable 1]
- [Deliverable 2]

**Tasks**:
1. [Task 1]
2. [Task 2]

### Track 2: [Name] — [Objective]
**Effort**: [hours/days]  
**Dependencies**: [Prerequisites]  
**Deliverables**:
- [Deliverable 1]

**Tasks**:
1. [Task 1]

### Track 3: [Name] — [Objective]
**Effort**: [hours/days]  
**Dependencies**: [Prerequisites]  
**Deliverables**:
- [Deliverable 1]

**Tasks**:
1. [Task 1]

---

## Execution Selector

Choose the correct entrypoint based on objective:

| Objective                         | Entry Point | Effort    |
|-----------------------------------|-------------|-----------|
| [Quick win / urgent fix]          | EP-001      | 30m-1h    |
| [Core functionality fix]          | EP-002      | 2-4h      |
| [Infrastructure improvement]      | EP-003      | 4-8h      |
| [Feature implementation]          | EP-004      | 1-3 days  |

### Default Execution Order (if unsure)

1. EP-001 ([reason])
2. EP-002 ([reason])
3. EP-003 ([reason])

---

## Entry Points

### EP-001: [ID] — [Short Description]

**Objective**: [What this accomplishes]

**Preconditions**:
```bash
# Verify precondition 1
command | grep -q "expected" ; # EXIT CODE 0 = met

# Verify precondition 2
test -f path/to/file ; # EXIT CODE 0 = met
```

**Inputs** (explicit):
- `VARIABLE_NAME`: [description] (e.g., `btcusdt`, `ethusdt`)
- `CONFIG_FILE`: [description] (default: `path/to/default`)

**Steps**:
```bash
# Step 1: [What this does]
command arg1 arg2

# Step 2: [What this does]
command arg1 arg2

# Step 3: Verify outcome
command | grep -q "success"
```

**Expected Outcome**:
```bash
# PASS condition 1: [Description]
command | grep -q "expected"

# PASS condition 2: [Description]
test -f path/to/file

# PASS condition 3: [Description]
cargo test --all 2>&1 | grep -q "test result: ok"
```

**Failure Detection**:
```bash
# FAIL if [condition]
# FAIL if [condition]
# FAIL if [condition]
```

**Rollback**:
```bash
git restore path/to/file1 path/to/file2
rm -f path/to/created-file
cargo build --all
```

---

### EP-002: [ID] — [Short Description]

[Follow same structure as EP-001]

---

## Verification Commands Reference

**Check if service running**:
```bash
curl -s http://localhost:PORT/status | jq '.status' | grep -q "running"
```

**Check if build successful**:
```bash
cargo build --all 2>&1 | grep -q "Finished" && echo "PASS" || echo "FAIL"
```

**Check if tests pass**:
```bash
cargo test --all 2>&1 | tail -1 | grep -q "test result: ok" && echo "PASS" || echo "FAIL"
```

**Check if migration applied**:
```bash
psql $DATABASE_URL -c "\d table_name" | grep -q "Table" && echo "PASS" || echo "FAIL"
```

**Check if config correct**:
```bash
grep -q 'expected_value' path/to/config && echo "PASS" || echo "FAIL"
```

---

## Rollback Notes

### Rollback Pattern 1: Code Changes
```bash
git restore path/to/modified/file1 path/to/modified/file2
rm -f path/to/new/file
cargo build --all
```

### Rollback Pattern 2: Database Migration
```bash
# Assuming migration N was applied
psql $DATABASE_URL -c "DROP TABLE IF EXISTS new_table CASCADE;"
# OR: revert to previous migration
diesel migration revert
```

### Rollback Pattern 3: Configuration
```bash
cp path/to/config.backup path/to/config
systemctl restart service-name
```

### Rollback Pattern 4: Infrastructure
```bash
kubectl delete -f manifests/new-resource.yaml
# OR: restore previous version
kubectl apply -f manifests/previous-resource.yaml
```

---

## Appendices

### Appendix A: [Detailed Technical Analysis]
[Optional — detailed logs, stack traces, metrics, benchmarks]

### Appendix B: [Reference Materials]
[Links to external docs, ADRs, RFCs, issues, PRs]

### Appendix C: [Decision Log]
[Alternatives considered and rejected, with rationale]

---

## Changelog

| Date       | Change                          | Author |
|------------|---------------------------------|--------|
| YYYY-MM-DD | Initial draft                   | [Name] |
| YYYY-MM-DD | Added EP-003, EP-004            | [Name] |
| YYYY-MM-DD | Updated verification commands   | [Name] |
