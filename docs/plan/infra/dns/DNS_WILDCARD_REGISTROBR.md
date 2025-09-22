# Wildcard DNS at Registro.br

Objective
- Resolve all preview and production hosts like `h-<branch>.robson.rbx.ia.br` and `app.robson.rbx.ia.br` to the same public IP of your Gateway (Istio) without needing a DNS API.

Steps
1) Find your Gateway public IP (LoadBalancer):
   - After installing Istio + Gateway, run: `kubectl get svc -A | rg -i loadbalancer`
   - Identify the external IP assigned to the Gateway data-plane (e.g., istio ingress/gateway service).
2) In Registro.br (DNS Avançado):
   - Create an A record: `*.robson.rbx.ia.br` → `<GATEWAY_LB_IP>`.
   - If IPv6 is used, add AAAA: `*.robson.rbx.ia.br` → `<GATEWAY_LB_IPV6>`.
   - TTL: 300–600 seconds is a good starting point.
3) Propagation
   - Wait DNS propagation (usually a few minutes).
   - Verify with `dig h-test.robson.rbx.ia.br A +short`.

Notes
- This wildcard covers all preview hosts `h-<branch>.robson.rbx.ia.br` automatically.
- If the LoadBalancer IP changes, update the single wildcard record.
- TLS is handled separately via cert-manager HTTP-01 or a wildcard certificate.

