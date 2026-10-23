#!/usr/bin/env bash
set -euo pipefail

release_id="${1:?Usage: resume-tailor-deploy RELEASE_ID ARCHIVE}"
archive="${2:?Usage: resume-tailor-deploy RELEASE_ID ARCHIVE}"

if [[ ! "$release_id" =~ ^[a-f0-9]{40}$ ]]; then
  echo "Invalid release id" >&2
  exit 2
fi
if [[ ! -f "$archive" ]]; then
  echo "Release archive not found: $archive" >&2
  exit 2
fi

base=/opt/resume-tailor
releases="$base/releases"
release="$releases/$release_id"
staging="$releases/$release_id.staging"
current="$base/current"
previous="$(readlink -f "$current" 2>/dev/null || true)"

install -d "$releases"
rm -rf "$staging"
install -d "$staging"
tar -xzf "$archive" -C "$staging"
test -x "$staging/tailor-server"
test -f "$staging/web/dist/index.html"
chmod -R u=rwX,go=rX "$staging"
chmod 755 "$staging/tailor-server"
rm -rf "$release"
mv "$staging" "$release"

ln -sfn "$release" "$base/current.next"
mv -Tf "$base/current.next" "$current"

rollback() {
  echo "Deployment failed; restoring previous release" >&2
  if [[ -n "$previous" && -d "$previous" ]]; then
    ln -sfn "$previous" "$base/current.rollback"
    mv -Tf "$base/current.rollback" "$current"
  else
    rm -f "$current"
  fi
  sudo -n systemctl restart resume-tailor.service || true
}

if ! sudo -n systemctl restart resume-tailor.service; then
  rollback
  exit 1
fi

healthy=false
for _ in {1..12}; do
  if curl --fail --silent --max-time 5 http://127.0.0.1:8787/api/health >/dev/null; then
    healthy=true
    break
  fi
  sleep 5
done

if [[ "$healthy" != true ]]; then
  rollback
  exit 1
fi

rm -f "$archive"
find "$releases" -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' \
  | sort -nr \
  | awk 'NR > 5 { sub(/^[^ ]+ /, ""); print }' \
  | while IFS= read -r old_release; do rm -rf -- "$old_release"; done

echo "Activated release $release_id"
