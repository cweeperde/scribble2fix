#!/usr/bin/env bash
# Usage: check-release.sh 1.2.3
# Fails unless the app version is 1.2.3 everywhere and CHANGELOG.md has a
# "## 1.2.3 (YYYY-MM-DD)" entry. Runs for every release tag (see build.yml).
set -euo pipefail
version="$1"
ok=true

cargo_toml=$(sed -n 's/^version = "\(.*\)"$/\1/p' app/Cargo.toml | head -n1)
tauri_conf=$(sed -n 's/^  "version": "\(.*\)",*$/\1/p' app/tauri.conf.json | head -n1)
cargo_lock=$(grep -A1 '^name = "scribble2fix"$' Cargo.lock | sed -n 's/^version = "\(.*\)"$/\1/p')

for pair in "app/Cargo.toml:$cargo_toml" "app/tauri.conf.json:$tauri_conf" "Cargo.lock:$cargo_lock"; do
  if [ "${pair#*:}" != "$version" ]; then
    echo "::error::${pair%%:*} has version '${pair#*:}', the tag says '$version'"
    ok=false
  fi
done

if ! grep -qxE "## ${version//./\\.} \([0-9]{4}-[0-9]{2}-[0-9]{2}\)" CHANGELOG.md; then
  echo "::error::CHANGELOG.md has no '## $version (YYYY-MM-DD)' entry"
  ok=false
elif ! .github/scripts/release-notes.sh "$version" CHANGELOG.md >/dev/null; then
  ok=false
fi

$ok && echo "Release $version: versions and changelog are consistent"
$ok
