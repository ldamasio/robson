# Frontend nginx Troubleshooting (k3s)

**Audience:** engineers debugging `robson-frontend-v2` pod failures.

The SvelteKit frontend runs as static files served by nginx inside a
container. The pod is configured to run as non-root (UID 101), which
introduces specific failure modes.

---

## Common failures

### CrashLoopBackOff: mkdir /var/cache/nginx permission denied

**Symptom:** pod crashes immediately, logs show:

```
mkdir(): "/var/cache/nginx/client_temp" failed (13: Permission denied)
```

**Cause:** nginx tries to create cache directories under
`/var/cache/nginx`, but the filesystem is owned by root and the
container runs as UID 101.

**Fix:** Add an emptyDir volume at `/var/cache/nginx`:

```yaml
spec:
  containers:
  - name: nginx
    volumeMounts:
    - name: nginx-cache
      mountPath: /var/cache/nginx
  volumes:
  - name: nginx-cache
    emptyDir: {}
```

### CrashLoopBackOff: /run/nginx.pid permission denied

**Symptom:** pod crashes, logs show:

```
open() "/run/nginx.pid" failed (13: Permission denied)
```

**Cause:** nginx writes its PID file to `/run`, which is owned by
root.

**Fix:** Add an emptyDir volume at `/run`:

```yaml
spec:
  containers:
  - name: nginx
    volumeMounts:
    - name: nginx-run
      mountPath: /run
  volumes:
  - name: nginx-run
      emptyDir: {}
```

### CreateContainerConfigError: runAsNonRoot with non-numeric user

**Symptom:** pod never starts, event shows:

```
Error: container has runAsNonRoot and image will run as uid 0
```

or:

```
Error: compute effective security context: non-numeric user (nginx)
```

**Cause:** `securityContext.runAsNonRoot: true` is set but no
explicit `runAsUser` is specified. The kubelet cannot verify the
container will run as non-root when the image uses a named user
(like `nginx`) rather than a numeric UID.

**Fix:** Add `runAsUser: 101` to the securityContext:

```yaml
securityContext:
  allowPrivilegeEscalation: false
  runAsNonRoot: true
  runAsUser: 101
  capabilities:
    drop:
      - ALL
```

### ImagePullBackOff

**Symptom:** pod shows `ImagePullBackOff` or `ErrImagePull`.

**Cause:** The image tag doesn't exist in GHCR, or the tag format
is wrong.

**Fix:** Verify the image exists:

```bash
gh api /users/ldamasio/packages/container/robson-frontend-v2/versions \
  --jq '.[].metadata.container.tags'
```

Ensure the deployment uses a valid SHA tag
(`ghcr.io/ldamasio/robson-frontend-v2:sha-XXXXXXXX`).

---

## Diagnostic commands

```bash
# Check pod status
kubectl get pods -n robson -l app.kubernetes.io/name=robson-frontend-v2

# View crash logs (previous container)
kubectl logs -n robson -l app.kubernetes.io/name=robson-frontend-v2 --previous

# View events
kubectl get events -n robson --sort-by='.lastTimestamp'

# Describe the pod for detailed error
kubectl describe pod -n robson -l app.kubernetes.io/name=robson-frontend-v2

# Check deployment config
kubectl get deploy robson-frontend-v2 -n robson -o yaml
```

---

## Reference

- ADR-0033: hosting pivot from Contabo S3 to k3s
- `docs/runbooks/frontend-deploy.md`: deployment procedure
- `rbx-infra/apps/prod/robson/robson-frontend-v2-deploy.yml`: current
  deployment manifest with both emptyDir volumes applied
