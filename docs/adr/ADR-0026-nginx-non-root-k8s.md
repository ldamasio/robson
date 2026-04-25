# ADR-0026: Nginx Non-Root Container in k3s

**Date:** 2026-04-25
**Status:** Accepted

## Context

The v3 frontend is served by an `nginx:1.27-alpine` container in
the rbx-infra k3s cluster (see ADR-0028 for the hosting pivot).

The cluster baseline applies a security context that requires
`runAsNonRoot: true`. The default nginx alpine image starts as
root, then drops to the `nginx` user (UID 101) for worker
processes. With `runAsNonRoot` enforced at the pod level, the
container cannot start at all — kubelet refuses to schedule it.

Additionally, even when the container is forced to run as `nginx`
from PID 1, several nginx defaults still attempt to write to
locations owned by root:

- `/var/cache/nginx/*` (proxy/fastcgi caches)
- `/run/nginx.pid` (master pidfile)
- `/var/log/nginx/*.log` (access/error logs)

A naive `runAsUser: 101` causes `CrashLoopBackOff` with errors
like `mkdir() ".../proxy_temp" failed (13: Permission denied)`
or `open() "/run/nginx.pid" failed (13: Permission denied)`.

## Decision

Run nginx as UID 101 explicitly, with writable `emptyDir` volumes
mounted over the cache and runtime directories, and direct logs to
stdout/stderr so the cluster log pipeline owns log rotation.

The Deployment spec for `robson-frontend-v2` sets:

```yaml
securityContext:
  runAsNonRoot: true
  runAsUser: 101
  runAsGroup: 101
  fsGroup: 101
containers:
  - name: nginx
    image: ghcr.io/ldamasio/robson-frontend-v2:latest
    securityContext:
      allowPrivilegeEscalation: false
      readOnlyRootFilesystem: false
      capabilities:
        drop: ["ALL"]
    volumeMounts:
      - name: cache
        mountPath: /var/cache/nginx
      - name: run
        mountPath: /run
volumes:
  - name: cache
    emptyDir: {}
  - name: run
    emptyDir: {}
```

The `nginx.conf` shipped in the image:

- Listens on port 8080 (port 80 requires `CAP_NET_BIND_SERVICE`,
  which is dropped).
- Sends access and error logs to stdout/stderr.
- Sets `pid /run/nginx.pid;` so the writable `emptyDir` covers it.

## Consequences

**Positive**

- Container starts cleanly under cluster `runAsNonRoot` baseline.
- No persistent filesystem state — pod restarts get a clean cache.
- Capability set is empty; nginx cannot bind privileged ports or
  acquire any other Linux capabilities.
- Logs flow to the standard k8s log pipeline (Promtail/Loki) with
  no extra agent.

**Negative / trade-offs**

- `emptyDir` is per-pod. There is no shared cache across replicas;
  each pod warms its own. Acceptable for a static asset server.
- `readOnlyRootFilesystem: false` is set because nginx writes
  small temp files in `/var/lib/nginx/` during reload. Tightening
  to read-only-root would require additional mounts and is
  deferred to a follow-up.

## Alternatives

- **Run as root** — rejected; violates cluster baseline and
  exposes container breakout risk.
- **Custom nginx-unprivileged image** (e.g., `nginxinc/nginx-unprivileged`)
  — viable; not adopted because vanilla `nginx:alpine` plus the
  volume strategy keeps the image sourcing standard and avoids a
  second supply chain.
- **OpenResty / Caddy** — overkill for a static SPA; introduces a
  new operational surface.

## Implementation Notes

- Manifest: `rbx-infra/apps/prod/robson/robson-frontend-v2-deploy.yml`.
- nginx config (in the image): `apps/frontend/nginx.conf`.
- The same pattern applies to any future static-asset container in
  the cluster.

**Failure mode to remember.** If a future engineer copies this
deployment and forgets the `emptyDir` mounts, the symptom is
`CrashLoopBackOff` with `nginx: [emerg] mkdir() ".../proxy_temp"
failed (13: Permission denied)`. Diagnose with `kubectl logs
<pod> --previous`.
