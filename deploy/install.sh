#!/usr/bin/env bash
# Install or update Harmony on a Linux server from a GitHub release bundle.
#
#   sudo bash install.sh v0.1.0
#
# Safe to run again: it installs the new release next to the old ones, points
# /opt/harmony/current at it and restarts the service. Settings in
# /etc/harmony/harmony.env and the database in /var/lib/harmony are kept.
# It does not touch the web server or firewall; see docs/deploy.md for those.
set -euo pipefail

REPO="DanteMarone/harmony"
VERSION="${1:?usage: install.sh <release tag, e.g. v0.1.0>}"
ARCH="$(uname -m)"
[ "$ARCH" = "x86_64" ] || { echo "Only x86_64 bundles are published so far (this is $ARCH)." >&2; exit 1; }
[ "$(id -u)" -eq 0 ] || { echo "Run as root (sudo)." >&2; exit 1; }

NAME="harmony-${VERSION}-linux-x86_64"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${NAME}.tar.gz"
DEST="/opt/harmony/releases/${VERSION}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading ${NAME}..."
curl -fsSL -o "$tmp/${NAME}.tar.gz" "$URL"
curl -fsSL -o "$tmp/${NAME}.tar.gz.sha256" "$URL.sha256"
(cd "$tmp" && sha256sum -c "${NAME}.tar.gz.sha256")

tar -C "$tmp" -xzf "$tmp/${NAME}.tar.gz"
rm -rf "$DEST"
mkdir -p /opt/harmony/releases
mv "$tmp/$NAME" "$DEST"
chown -R root:root "$DEST"
chmod -R a+rX,go-w "$DEST"

# A system user with no shell and no home that only runs the server.
if ! id harmony >/dev/null 2>&1; then
  useradd --system --no-create-home --shell /usr/sbin/nologin harmony
fi

mkdir -p /etc/harmony
if [ ! -f /etc/harmony/harmony.env ]; then
  install -m 0640 -o root -g harmony "$DEST/deploy/harmony.env" /etc/harmony/harmony.env
fi
install -m 0644 "$DEST/deploy/harmony.service" /etc/systemd/system/harmony.service

ln -sfn "$DEST" /opt/harmony/current
systemctl daemon-reload
systemctl enable harmony >/dev/null
systemctl restart harmony

sleep 2
if curl -fsS http://127.0.0.1:8080/api/health >/dev/null; then
  echo "Harmony ${VERSION} is running."
else
  echo "Harmony did not answer on 127.0.0.1:8080. Check: journalctl -u harmony -n 50" >&2
  exit 1
fi

# Keep the three newest releases.
ls -1dt /opt/harmony/releases/*/ | tail -n +4 | xargs -r rm -rf

# On the very first start the server prints a one-time admin invite.
if journalctl -u harmony --since "-1 min" -o cat | grep -q '/#/invite/'; then
  echo
  echo "First start: open this link (with your own address in front) to create the admin account:"
  journalctl -u harmony --since "-1 min" -o cat | grep '/#/invite/' | tail -n 1
fi
