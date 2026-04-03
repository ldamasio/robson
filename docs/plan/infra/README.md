# Infrastructure Planning Moved

Shared infrastructure planning was intentionally removed from the Robson application repository.

- Cluster bootstrap, DNS, ingress, TLS platform setup, and other cross-project infrastructure work now belong in `rbx-infra`.
- This repository keeps application architecture, feature planning, and app-adjacent operational runbooks.
- Use `docs/infra/` and `docs/runbooks/` for the remaining deployment and operations context that is still relevant to the application itself.
