Short and practical: point the wildcard to the Gateway public endpoint (for now, use the "tiger" IP), not to the entire cluster or specific services.

How to configure at Registro.br (Advanced DNS) for rbx.ia.br

Goal: resolve any h-<branch>.robson.rbx.ia.br to the same Gateway public IP (HTTP-01 TLS, previews, etc.).
Main rule (wildcard):
Subdomain: *.robson
Type: A
Value: 158.220.116.31 (public IP of "tiger", temporary)
TTL: 300–600
IPv6 (if available): repeat as AAAA for the "tiger" IPv6.
What this enables: h-anything.robson.rbx.ia.br → 158.220.116.31 (covers the hosts created by ApplicationSet).

Yes — also configure the apex rbx.ia.br pointing to the gateway public IP (for now, "tiger").

What to create in Registro.br (Advanced DNS)

A (apex)
Name: @ (or leave the subdomain field blank)
Type: A
Value: 158.220.116.31
TTL: 300–600
A (wildcard for previews)
Name: *.robson
Type: A
Value: 158.220.116.31
TTL: 300–600
(Optional) AAAA for IPv6, if "tiger" has public IPv6
Name: @ and *.robson
Type: AAAA
Value: <tiger IPv6>
(Optional) Convenience records for management/SSH
tiger.rbx.ia.br → 158.220.116.31
bengal.rbx.ia.br → 164.68.96.68
pantera.rbx.ia.br → 149.102.139.33
eagle.rbx.ia.br → 167.86.92.97
Quick validation (after saving)

dig +short rbx.ia.br
dig +short feature-x.robson.rbx.ia.br
Both should return 158.220.116.31.
To serve content at rbx.ia.br (once Istio/Gateway is installed)

We need:
Gateway with a listener for rbx.ia.br (HTTP 80 and HTTPS 443),
HTTPRoute routing rbx.ia.br to the target service (frontend or a landing),
Certificate for rbx.ia.br (cert-manager HTTP-01). Our charts already create a Certificate for the host defined in values.
Options:
Adjust the frontend chart values so host: rbx.ia.br (prod).
Or create a separate release/route for rbx.ia.br (landing), keeping app.robson.rbx.ia.br for the SPA.
Important

We have already opened 80/443 on the local UFW (server/gateway), but if there is a Contabo Cloud Firewall, allow 80/443 TCP as well (and later 41753/22 for SSH, per the plan).
Until we install the Gateway/Routes/Certificates, DNS will resolve but there will not yet be an HTTP response. That is expected until the platform stage.
Next step

Once you create the records in Registro.br, let me know "ok" and we will run the join of the first agent (bengal) to validate the cluster, then continue with pantera and eagle.
Suggested commit message (if you want to record this decision in the docs):
docs(infra): add DNS guidance for apex rbx.ia.br and wildcard *.robson.rbx.ia.br pointing to gateway IP
