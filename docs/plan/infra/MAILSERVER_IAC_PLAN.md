Mailserver IaC — Execution Plan

Status: Draft
Scope: Provision and harden a standalone Ubuntu mail host (Postfix + Dovecot) via Ansible, integrate with existing Robson observability and secrets workflow.

Target host (TBD → IP)
- codename: jaguar (Ubuntu 22.04 LTS, 4 vCPU, 8GB RAM, 100GB SSD)
- services: Postfix (SMTP), Dovecot (IMAP/POP3), OpenDKIM, SpamAssassin/Rspamd, Fail2ban, UFW, node_exporter, Filebeat/Fluent Bit
- network: public static IPv4; open ports 25/465/587/993 (+80/443 for ACME if needed)

Decisions
- Configuration via dedicated Ansible role `roles/mailserver`
- TLS certificates acquired with Let’s Encrypt HTTP-01 (or reuse Istio CA bundle if ACME blocked)
- Secrets stored under `infra/ansible/group_vars/mail/vault.yml` (encrypted) and referenced through defaults/vars
- Mail users managed through virtual mailboxes (Dovecot + Postfix, backed by system users for now)

Inventory additions (`infra/ansible/inventory/production/mail.yml`)
```
mail
├─ hosts
│  └─ jaguar ansible_host=<IP> ansible_user=robson ansible_port=<vault_ssh_port>
└─ vars
   └─ mail_domain: robson.rbx.ia.br
```
Tag execution example:
```
ansible-playbook -i inventory/production/mail.yml site.yml --tags mail \
  -u robson --private-key ~/.ssh/id_ed25519
```

Role layout (`infra/ansible/roles/mailserver`)
- defaults/main.yml: public defaults, feature toggles (antispam, monitoring)
- vars/main.yml: OS-specific settings/paths
- handlers/main.yml: restart postfix/dovecot, reload dovecot, restart opendkim, restart rspamd/spamassassin
- tasks/
  - main.yml → include setup.yml, postfix.yml, dovecot.yml, antispam.yml, monitoring.yml, hardening.yml
  - setup.yml → prerequisite packages, system users/groups (`vmail`), directory tree, timezone/locale, fail2ban baseline
  - postfix.yml → install Postfix, templates (`main.cf.j2`, `master.cf.j2`), SASL auth, TLS certificates (`/etc/ssl/mail`)
  - dovecot.yml → install Dovecot, configure protocols, authentication, maildir storage under `/var/mail/vhosts/{{ mail_domain }}`
  - antispam.yml → install OpenDKIM + Rspamd (or SpamAssassin + Amavis fallback), publish keys, integrate with Postfix
  - monitoring.yml → node_exporter, Filebeat/Fluent Bit shipping to Robson stack (or journald target)
  - hardening.yml → UFW rules, Fail2ban jails for Postfix/Dovecot, sysctl adjustments, logrotate policies
- templates/ (main.cf.j2, master.cf.j2, dovecot.conf.j2, ssl config, DKIM key/selector, rspamd local.d configs)
- files/ (ACME hook scripts, default fail2ban jail overrides)
- molecule/ (scenario `default` using Ubuntu 22.04 container with Postfix smoke tests)

Group vars (`infra/ansible/group_vars/mail/`)
- main.yml → domain, hostnames, TLS mode, monitoring toggle, default aliases
- users.yml → virtual mailbox list (email → hashed password, mailbox path)
- vault.yml (encrypted) → hashed passwords, DKIM private keys, API tokens for DNS/ACME

Prerequisites
1. Confirm DNS control for `robson.rbx.ia.br` (MX, SPF, DKIM, DMARC records)
2. Ensure firewall/provider allows outbound 25/tcp
3. Generate SSH access via existing bootstrap role (reuse `roles/bootstrap` before applying mail)

Execution steps
- Step 0: Bootstrap host security via existing Ansible workflow (user, SSH hardening, updates)
- Step 1: Populate `group_vars/mail/{main,users}.yml` and encrypt `vault.yml` (`ansible-vault encrypt ...`)
- Step 2: Generate/validate TLS (Let’s Encrypt via acme.sh role or copy certs to `/etc/letsencrypt/live/mail.{{ mail_domain }}`)
- Step 3: Run Molecule tests (`cd infra/ansible/roles/mailserver && molecule test`)
- Step 4: Apply playbook with `--tags mail`, verify idempotency with second run
- Step 5: Functional tests: SMTP submission (587/TLS), IMAP login (993), anti-spam scoring, DKIM signing
- Step 6: Integrate monitoring/alerts in Prometheus/Grafana, centralize logs in existing ELK/Loki

Verification checklist
- `postfix status` and `dovecot status` active
- `openssl s_client -starttls smtp -connect mail.{{ mail_domain }}:587` passes with valid cert
- Test user login via IMAP (`openssl s_client -connect mail.{{ mail_domain }}:993`)
- DKIM signature validated via `opendkim-testkey`
- Spam tests using GTUBE/GTUBED commands

Observability integration
- node_exporter scraped by existing Prometheus (include new target in `infra/k8s/monitoring`)
- Filebeat/Fluent Bit shipping `/var/log/mail.log` and `/var/log/dovecot.log` to central stack
- Fail2ban actions pushing metrics/logs

Open questions
- Preferred antispam engine: Rspamd vs SpamAssassin + Amavis?
- Mail storage backend: systemd-homed vs Maildir on ext4 with quotas?
- Need for Sieve server/roundcube webmail in scope?

Next (out of scope for this plan)
- High availability via secondary MX relay
- Backup strategy (restic, snapshots)
- Webmail UI deployment
