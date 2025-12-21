# Vault Reset - December 2024

## Context

Previous Ansible Vault password was unavailable, preventing infrastructure operations.

## Investigation

- SSH access to 4 VPS failed (root password lost + SSH keys not authorized)
- Vault file contains only reconstructible configuration values:
  - SSH port setting
  - Admin SSH public key
  - k3s cluster token (obtainable from server)
- No irrecoverable secrets

## Decision

**RESET ALL 4 VPS** via Contabo control panel:
- tiger (158.220.116.31) - Ubuntu 24.04
- bengal (164.68.96.68) - Ubuntu 24.04
- pantera (149.102.139.33) - Ubuntu 24.04
- eagle (167.86.92.97) - Ubuntu 24.04

**CREATE NEW VAULT** with new password under current maintainer control.

## Actions Taken

1. ✅ Reinstalled all 4 VPS (Ubuntu 24.04 fresh install)
2. ✅ Verified SSH root access on port 22
3. ✅ Archived old vault: `vault.yml.orphaned-2024-12`
4. ⏳ Create new vault with reconstructed values
5. ⏳ Deploy k3s cluster from clean state

## Rationale

- Faster than password recovery or forensics
- Fresh start ensures clean production state
- All vault values are reconstructible
- Hardening can be applied incrementally after deployment

## Timeline

- **Investigation**: 2024-12-20
- **VPS Reset**: 2024-12-20
- **Vault Recreation**: 2024-12-20 (in progress)
- **Cluster Deployment**: 2024-12-20 (planned)

## Security Notes

This reset follows best practices:
- No irrecoverable data lost
- Clear ownership transfer
- Documented decision trail
- Temporary simplified security for quick deployment
- Full hardening planned for Phase 3 (post-deployment)

## References

- Main deployment plan: `docs/plan/infra/QUICK-DEPLOY-2024-12.md`
- Vault template: `infra/ansible/VAULT-TEMPLATE.md`
- Inventory template: `infra/ansible/INVENTORY-TEMPLATE.md`

---

**Date**: 2024-12-20  
**Maintainer**: Leandro Damásio (@ldamasio)  
**Status**: In Progress
