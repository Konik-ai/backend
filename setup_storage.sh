#!/usr/bin/env bash
set -Eeuo pipefail

# Storage server identity
STORAGE_HOSTNAME="storage.api1.ca"
STORAGE_PUBLIC_IP="212.244.158.38"

# Immich server identity
IMMICH_HOSTNAME="vps.api1.ca"
IMMICH_PUBLIC_IP="144.126.142.139"

# WireGuard settings
WG_IF="wg-storage"
WG_PORT="51820"
WG_SUBNET="10.77.0.0/24"
WG_STORAGE_CIDR="10.77.0.1/32"
WG_STORAGE_IP="10.77.0.1"
WG_IMMICH_CIDR="10.77.0.2/32"
WG_IMMICH_IP="10.77.0.2"

# Storage path and ownership
RAID_DIR="/mnt/raid5"
CIPHER_DIR="/mnt/raid5/.cache/.thumbs"
SVC_USER="scv"

# Files
WG_PRIVATE_KEY_FILE="/etc/wireguard/storage_private.key"
WG_PUBLIC_KEY_FILE="/etc/wireguard/storage_public.key"
WG_CONF_FILE="/etc/wireguard/wg-storage.conf"
WG_FIREWALL_HELPER="/usr/local/sbin/wg-storage-firewall.sh"
NFS_CONF_FILE="/etc/nfs.conf.d/wg-storage.conf"
EXPORTS_FILE="/etc/exports.d/immich-cipher.exports"
NFS_DROPIN_DIR="/etc/systemd/system/nfs-kernel-server.service.d"
NFS_DROPIN_FILE="${NFS_DROPIN_DIR}/wg-order.conf"

SUDO=""
if [[ "${EUID}" -ne 0 ]]; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "Run as root or install sudo." >&2
    exit 1
  fi
  SUDO="sudo"
fi

log() {
  printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required command not found: $1" >&2
    exit 1
  fi
}

log "Validating prerequisites..."
require_cmd apt-get
require_cmd systemctl
require_cmd tee
require_cmd awk

if [[ ! -d "${RAID_DIR}" ]]; then
  echo "Expected RAID mount path missing: ${RAID_DIR}" >&2
  exit 1
fi

log "Installing packages..."
${SUDO} apt-get update
${SUDO} DEBIAN_FRONTEND=noninteractive apt-get install -y \
  wireguard nfs-kernel-server nfs-common rpcbind iptables

log "Creating service user and ciphertext directory..."
if ! id "${SVC_USER}" >/dev/null 2>&1; then
  ${SUDO} adduser --system --group --home /nonexistent --shell /usr/sbin/nologin "${SVC_USER}"
fi
${SUDO} install -d -m 700 -o "${SVC_USER}" -g "${SVC_USER}" "${CIPHER_DIR}"
${SUDO} chown -R "${SVC_USER}:${SVC_USER}" "${CIPHER_DIR}"
${SUDO} chmod 700 "${CIPHER_DIR}"

log "Generating WireGuard keys (if missing)..."
${SUDO} install -d -m 700 /etc/wireguard
if [[ ! -s "${WG_PRIVATE_KEY_FILE}" ]]; then
  ${SUDO} sh -c "umask 077; wg genkey > '${WG_PRIVATE_KEY_FILE}'"
fi
if [[ ! -s "${WG_PUBLIC_KEY_FILE}" ]]; then
  ${SUDO} sh -c "wg pubkey < '${WG_PRIVATE_KEY_FILE}' > '${WG_PUBLIC_KEY_FILE}'"
fi
${SUDO} chmod 600 "${WG_PRIVATE_KEY_FILE}" "${WG_PUBLIC_KEY_FILE}"

IMMICH_WG_PUBLIC_KEY="${IMMICH_WG_PUBLIC_KEY:-}"
if [[ -z "${IMMICH_WG_PUBLIC_KEY}" ]]; then
  read -r -p "Paste Immich WireGuard public key: " IMMICH_WG_PUBLIC_KEY
fi
if [[ -z "${IMMICH_WG_PUBLIC_KEY}" ]]; then
  echo "Immich WireGuard public key is required." >&2
  exit 1
fi

log "Installing firewall helper for wg-storage..."
${SUDO} tee "${WG_FIREWALL_HELPER}" >/dev/null <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

CHAIN="WG_STORAGE_NFS_ONLY"
WG_IF="wg-storage"
WG_PORT="51820"
WG_CLIENT_IP="10.77.0.2"
NFS_TCP_PORTS="111,2049,20048,32765,32766,32768"
NFS_UDP_PORTS="111,20048,32765,32766,32768"

case "${1:-}" in
  up)
    iptables -N "${CHAIN}" 2>/dev/null || true
    iptables -F "${CHAIN}"
    iptables -A "${CHAIN}" -p udp --dport "${WG_PORT}" -j ACCEPT
    iptables -A "${CHAIN}" -i lo -p tcp -m multiport --dports "${NFS_TCP_PORTS}" -j ACCEPT
    iptables -A "${CHAIN}" -i lo -p udp -m multiport --dports "${NFS_UDP_PORTS}" -j ACCEPT
    iptables -A "${CHAIN}" -i "${WG_IF}" -s "${WG_CLIENT_IP}" -p tcp -m multiport --dports "${NFS_TCP_PORTS}" -j ACCEPT
    iptables -A "${CHAIN}" -i "${WG_IF}" -s "${WG_CLIENT_IP}" -p udp -m multiport --dports "${NFS_UDP_PORTS}" -j ACCEPT
    iptables -A "${CHAIN}" -p tcp -m multiport --dports "${NFS_TCP_PORTS}" -j DROP
    iptables -A "${CHAIN}" -p udp -m multiport --dports "${NFS_UDP_PORTS}" -j DROP
    iptables -A "${CHAIN}" -j RETURN
    iptables -C INPUT -j "${CHAIN}" 2>/dev/null || iptables -I INPUT 1 -j "${CHAIN}"
    ;;
  down)
    iptables -D INPUT -j "${CHAIN}" 2>/dev/null || true
    iptables -F "${CHAIN}" 2>/dev/null || true
    iptables -X "${CHAIN}" 2>/dev/null || true
    ;;
  *)
    echo "Usage: $0 up|down" >&2
    exit 1
    ;;
esac
EOF
${SUDO} chmod 700 "${WG_FIREWALL_HELPER}"

log "Writing ${WG_CONF_FILE}..."
WG_PRIVATE_KEY="$(${SUDO} cat "${WG_PRIVATE_KEY_FILE}")"
${SUDO} tee "${WG_CONF_FILE}" >/dev/null <<EOF
[Interface]
Address = ${WG_STORAGE_CIDR}
ListenPort = ${WG_PORT}
PrivateKey = ${WG_PRIVATE_KEY}
PostUp = ${WG_FIREWALL_HELPER} up
PostDown = ${WG_FIREWALL_HELPER} down

[Peer]
PublicKey = ${IMMICH_WG_PUBLIC_KEY}
AllowedIPs = ${WG_IMMICH_CIDR}
PersistentKeepalive = 25
EOF
unset WG_PRIVATE_KEY
${SUDO} chmod 600 "${WG_CONF_FILE}"

log "Writing pinned NFS config..."
${SUDO} install -d /etc/nfs.conf.d
${SUDO} tee "${NFS_CONF_FILE}" >/dev/null <<EOF
[nfsd]
host=${WG_STORAGE_IP}
port=2049
tcp=y
udp=n
vers2=n
vers3=n
vers4=y
vers4.1=y
vers4.2=y

[mountd]
port=20048

[statd]
port=32765
outgoing-port=32766

[lockd]
port=32768
udp-port=32768
EOF

log "Writing NFS export..."
SVC_UID="$(id -u "${SVC_USER}")"
SVC_GID="$(id -g "${SVC_USER}")"
${SUDO} install -d /etc/exports.d
${SUDO} tee "${EXPORTS_FILE}" >/dev/null <<EOF
${CIPHER_DIR} ${WG_IMMICH_IP}(rw,sync,no_subtree_check,all_squash,anonuid=${SVC_UID},anongid=${SVC_GID},sec=sys)
EOF
unset SVC_UID SVC_GID

log "Setting NFS dependency on WireGuard..."
${SUDO} install -d "${NFS_DROPIN_DIR}"
${SUDO} tee "${NFS_DROPIN_FILE}" >/dev/null <<EOF
[Unit]
Requires=wg-quick@${WG_IF}.service
After=wg-quick@${WG_IF}.service
PartOf=wg-quick@${WG_IF}.service
EOF

log "Enabling and starting services..."
${SUDO} systemctl daemon-reload
${SUDO} systemctl enable --now "wg-quick@${WG_IF}"
${SUDO} systemctl enable --now rpcbind
${SUDO} systemctl enable --now nfs-kernel-server
${SUDO} exportfs -ra
${SUDO} systemctl restart nfs-kernel-server

log "Running verification..."
${SUDO} wg show "${WG_IF}" || true
${SUDO} ss -lntu | grep -E ':(51820|111|2049|20048|32765|32766|32768)\b' || true
${SUDO} exportfs -v || true
${SUDO} iptables -S WG_STORAGE_NFS_ONLY || true
log "Skipping live showmount/rpcinfo probes to avoid hangs on strict NFSv4/firewalled setups."

STORAGE_WG_PUBLIC_KEY="$(${SUDO} cat "${WG_PUBLIC_KEY_FILE}")"

cat <<EOF

Setup complete.

Storage host: ${STORAGE_HOSTNAME} (${STORAGE_PUBLIC_IP})
Immich host:  ${IMMICH_HOSTNAME} (${IMMICH_PUBLIC_IP})
WireGuard interface: ${WG_IF}
WireGuard subnet: ${WG_SUBNET}
Storage WG IP: ${WG_STORAGE_CIDR}
Immich WG IP:  ${WG_IMMICH_CIDR}
NFS export: ${CIPHER_DIR} -> ${WG_IMMICH_IP} only

Paste these into Immich WireGuard config:
- Storage server public key: ${STORAGE_WG_PUBLIC_KEY}
- Endpoint: ${STORAGE_PUBLIC_IP}:${WG_PORT}
- AllowedIPs on Immich peer: ${WG_STORAGE_CIDR}

Ciphertext-only workflow:
1) Mount this NFS export from Immich over WireGuard.
2) Run: gocryptfs -init <mounted-cipher-dir>  (on Immich)
3) Mount plaintext only on Immich with gocryptfs.
EOF
