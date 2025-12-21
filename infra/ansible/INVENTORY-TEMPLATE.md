# Inventory Template

Copy this to `inventory/contabo/hosts.ini` and fill in your actual root passwords.

```ini
[k3s_server]
# 8GB node (server)
tiger  ansible_host=158.220.116.31 ansible_user=root ansible_ssh_pass=YOUR_TIGER_PASSWORD

[k3s_agent]
# 8GB node (agent)
bengal ansible_host=164.68.96.68 ansible_user=root ansible_ssh_pass=YOUR_BENGAL_PASSWORD
# 4GB nodes (agents)
pantera ansible_host=149.102.139.33 ansible_user=root ansible_ssh_pass=YOUR_PANTERA_PASSWORD
eagle   ansible_host=167.86.92.97 ansible_user=root ansible_ssh_pass=YOUR_EAGLE_PASSWORD

[k3s_gateway]
tiger
```

## Security Notes:

⚠️ **This is temporary for quick deployment**

After production is working, you should:
1. Remove passwords from this file
2. Use SSH key authentication only
3. Create non-root admin user
4. Disable root SSH login

See Phase 3 in `docs/plan/infra/QUICK-DEPLOY-2024-12.md`
