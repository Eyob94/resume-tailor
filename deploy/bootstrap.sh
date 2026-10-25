#!/usr/bin/env bash
set -euo pipefail

if [[ "$EUID" -ne 0 ]]; then
  echo "Run this setup script with sudo: sudo $0 DEPLOY_USER" >&2
  exit 1
fi

deploy_user="${1:?Usage: sudo ./deploy/bootstrap.sh DEPLOY_USER}"
id "$deploy_user" >/dev/null
command -v systemctl >/dev/null || { echo "systemd is required" >&2; exit 1; }

missing_packages=()
command -v caddy >/dev/null || missing_packages+=(caddy)
command -v curl >/dev/null || missing_packages+=(curl)
if (( ${#missing_packages[@]} > 0 )); then
  command -v apt-get >/dev/null || { echo "Install ${missing_packages[*]} first; apt-get is not available" >&2; exit 1; }
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -q
  apt-get install -y -q "${missing_packages[@]}"
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
deploy_group="$(id -gn "$deploy_user")"
if ! id resume-tailor >/dev/null 2>&1; then
  useradd --system --home-dir /var/lib/resume-tailor --shell /usr/sbin/nologin resume-tailor
fi

install -d -o "$deploy_user" -g "$deploy_group" -m 755 /opt/resume-tailor /opt/resume-tailor/releases
install -d -o resume-tailor -g resume-tailor -m 750 /var/lib/resume-tailor
install -m 644 "$repo_root/deploy/resume-tailor.service" /etc/systemd/system/resume-tailor.service
install -m 755 "$repo_root/deploy/resume-tailor-deploy.sh" /usr/local/sbin/resume-tailor-deploy

if [[ ! -e /etc/resume-tailor.env ]]; then
  cat > /etc/resume-tailor.env <<'EOF'
PORT=8787
DATA_DIR=/var/lib/resume-tailor
WEB_DIST=/opt/resume-tailor/current/web/dist
OLLAMA_URL=http://127.0.0.1:11434
EOF
  chown root:resume-tailor /etc/resume-tailor.env
  chmod 640 /etc/resume-tailor.env
fi

install -m 644 "$repo_root/Caddyfile" /etc/caddy/Caddyfile
caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
systemctl daemon-reload
systemctl enable resume-tailor.service
systemctl enable --now caddy
systemctl reload caddy

if [[ "$deploy_user" != root ]]; then
  systemctl_path="$(command -v systemctl)"
  printf '%s ALL=(root) NOPASSWD: %s restart resume-tailor.service\n' \
    "$deploy_user" "$systemctl_path" > /etc/sudoers.d/resume-tailor-deploy
  chmod 440 /etc/sudoers.d/resume-tailor-deploy
  visudo -cf /etc/sudoers.d/resume-tailor-deploy
fi

echo "Server setup complete."
