#!/bin/bash
# DNS Port Redirect Configuration for NodePort Deployment
# This script configures iptables to redirect public DNS traffic (port 53) to NodePort

set -e

echo "=================================================="
echo "DNS Port 53 -> NodePort Redirect Configuration"
echo "=================================================="
echo ""

# Detect hostname
HOSTNAME=$(hostname)

case "$HOSTNAME" in
  tiger)
    echo "Configuring tiger (NS1) - 158.220.116.31"
    echo "Redirect: 53 -> 30053"

    # UDP redirect
    iptables -t nat -C PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053 2>/dev/null || \
      iptables -t nat -A PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 30053

    # TCP redirect
    iptables -t nat -C PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053 2>/dev/null || \
      iptables -t nat -A PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 30053

    echo "✓ Redirect configured for tiger"
    ;;

  bengal)
    echo "Configuring bengal (NS2) - 164.68.96.68"
    echo "Redirect: 53 -> 31053"

    # UDP redirect
    iptables -t nat -C PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053 2>/dev/null || \
      iptables -t nat -A PREROUTING -p udp --dport 53 -j REDIRECT --to-ports 31053

    # TCP redirect
    iptables -t nat -C PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053 2>/dev/null || \
      iptables -t nat -A PREROUTING -p tcp --dport 53 -j REDIRECT --to-ports 31053

    echo "✓ Redirect configured for bengal"
    ;;

  *)
    echo "ERROR: Unknown hostname: $HOSTNAME"
    echo "This script should only run on tiger or bengal"
    exit 1
    ;;
esac

echo ""
echo "Current NAT rules for DNS:"
iptables -t nat -L PREROUTING -n -v | grep -E ":53|:30053|:31053"

echo ""
echo "=================================================="
echo "Persisting iptables rules..."
echo "=================================================="

# Check if iptables-persistent is installed
if ! dpkg -l | grep -q iptables-persistent; then
    echo "Installing iptables-persistent..."
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y iptables-persistent
else
    echo "iptables-persistent already installed"
fi

# Save rules
netfilter-persistent save
echo "✓ Rules persisted"

echo ""
echo "=================================================="
echo "Configuration complete!"
echo "=================================================="
echo ""
echo "To verify DNS is working:"
echo "  dig @$(hostname -I | awk '{print $1}') strategos.gr NS"
echo "  dig @$(hostname -I | awk '{print $1}') strategos.gr SOA"
echo ""
