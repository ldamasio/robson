# external-dns Provider Options

We need automated DNS records for preview envs `h-<branch>.robson.rbx.ia.br` and production hosts. external-dns supports multiple providers.

Registro.br “DNS Avançado” may not be directly supported by external-dns. Options:

- Delegate a subzone to a supported provider (recommended):
  - Create a subzone (e.g., `rbx.ia.br` or `robson.rbx.ia.br`) in Cloudflare, Route53, DigitalOcean, etc.
  - Delegate NS records from Registro.br to that provider.
  - Configure external-dns with the provider’s API credentials (SealedSecret/SOPS).

- Use RFC2136 provider (if your authoritative DNS supports TSIG updates):
  - external-dns `--provider=rfc2136` with TSIG key and target DNS server.
  - Works if your DNS service allows dynamic updates. Verify Registro.br support; often not available.

Next steps
- Decide provider: Cloudflare/Route53 (delegate subdomain) or RFC2136 with TSIG.
- Create Kubernetes Secret with credentials (use SealedSecrets in Git).
- Helm values for external-dns will set `provider`, `domainFilters`, and credentials refs.

References
- https://kubernetes-sigs.github.io/external-dns/latest/

