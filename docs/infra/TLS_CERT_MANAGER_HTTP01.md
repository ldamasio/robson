# TLS via cert-manager (HTTP-01) with Gateway API

We will use cert-manager to issue certificates automatically for hosts under `robson.rbx.ia.br` using the HTTP-01 challenge with Gateway API.

Prerequisites
- Wildcard DNS `*.robson.rbx.ia.br` pointing to the Gateway LB IP (see `DNS_WILDCARD_REGISTROBR.md`).
- cert-manager >= v1.13 installed in the cluster.
- Gateway API CRDs installed (Istio + gateway.networking.k8s.io/v1beta1).

Install cert-manager (example)
```
helm repo add jetstack https://charts.jetstack.io
helm repo update
helm upgrade --install cert-manager jetstack/cert-manager \
  --namespace cert-manager --create-namespace \
  --set installCRDs=true
```

Create a ClusterIssuer (Let’s Encrypt HTTP-01)
- Replace `you@example.com` with your email.
- This example uses the Gateway HTTP-01 solver; reference the Gateway by name/namespace.

```
apiVersion: cert-manager.io/v1
kind: ClusterIssuer
metadata:
  name: letsencrypt-http01
spec:
  acme:
    email: you@example.com
    server: https://acme-v02.api.letsencrypt.org/directory
    privateKeySecretRef:
      name: letsencrypt-http01
    solvers:
    - http01:
        gatewayHTTPRoute:
          parentRefs:
          - name: robson-backend-gateway
            namespace: robson
```

Issue certificates via Helm chart templates
- Each app chart defines a Certificate referencing the ClusterIssuer and the app host.
- The Gateway listeners include HTTPS (port 443) referencing the Secret produced by the Certificate.

Troubleshooting
- Pending challenges: `kubectl describe challenge -A` and `kubectl logs -n cert-manager deploy/cert-manager`.
- Ensure HTTP (port 80) listener exists and routes /.well-known/acme-challenge.
- Rate limits: use Let’s Encrypt staging URL during tests: `https://acme-staging-v02.api.letsencrypt.org/directory`.

References
- https://cert-manager.io/docs/configuration/gateway/
- https://cert-manager.io/docs/usage/certificate/

