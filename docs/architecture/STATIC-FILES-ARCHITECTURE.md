# Static Files Architecture

## Overview

Robson uses a layered architecture for serving web traffic, combining
**Traefik** (ingress controller) with **Nginx** (static file server).

## Components

### Traefik (Ingress Controller)

- **Role**: Routes external traffic to internal services
- **Scope**: Cluster-level, manages all Ingress resources
- **Features**: TLS termination, load balancing, routing rules

### Nginx (Static File Server)

- **Role**: Serves Django static files (`/static/*`)
- **Deployment**: `rbs-backend-nginx-prod-deploy`
- **Service**: `rbs-backend-nginx-prod-svc`
- **Content**: CSS, JavaScript, images, Django admin assets

### Gunicorn (Django App Server)

- **Role**: Processes dynamic API requests (`/api/*`)
- **Deployment**: `rbs-backend-monolith-prod-deploy`

## Traffic Flow

```
                    Internet
                        │
                   ┌────▼────┐
                   │ Traefik │  ← Ingress Controller (TLS)
                   └────┬────┘
                        │
         ┌──────────────┼──────────────┐
         │              │              │
    ┌────▼────┐   ┌─────▼─────┐   ┌────▼────┐
    │Frontend │   │  Backend  │   │  Nginx  │
    │ (React) │   │ (Django)  │   │ (Static)│
    └─────────┘   └───────────┘   └─────────┘
         │              │              │
      /app/*        /api/*        /static/*
```

## Ingress Configuration

From `rbs-backend-prod-ingress.yml`:

```yaml
rules:
  - host: api.robson.rbx.ia.br
    http:
      paths:
        - path: "/"           # API requests → Django/Gunicorn
          backend:
            service:
              name: rbs-backend-monolith-prod-svc
              port: 8000
        - path: "/static"     # Static files → Nginx
          backend:
            service:
              name: rbs-backend-nginx-prod-svc
              port: 80
```

## Why This Architecture?

1. **Performance**: Nginx is optimized for serving static files
2. **Resource Efficiency**: Gunicorn workers handle only dynamic requests
3. **Best Practice**: Recommended Django production setup
4. **Caching**: Nginx can cache static assets efficiently

## Related Files

- `infra/k8s/prod/rbs-backend-nginx-prod-deploy.yml`
- `infra/k8s/prod/rbs-backend-nginx-prod-svc.yml`
- `infra/k8s/prod/rbs-backend-prod-ingress.yml`
- `apps/backend/monolith/docker/Dockerfile_nginx`

## Service Types

| Service | Type | Reason |
|---------|------|--------|
| `rbs-backend-nginx-prod-svc` | ClusterIP | Internal only, accessed via Ingress |
| `rbs-backend-monolith-prod-svc` | ClusterIP | Internal only, accessed via Ingress |
| `rbs-frontend-prod-svc` | ClusterIP | Internal only, accessed via Ingress |

> **Note**: We use `ClusterIP` (not `LoadBalancer`) because external traffic
> is routed through Traefik Ingress Controller.

## Historical Note

On 2025-12-29, the nginx service was changed from `LoadBalancer` to `ClusterIP`
to fix ArgoCD sync issues. The `LoadBalancer` type was causing the service to
remain in "Progressing" state, blocking automatic deployments.

