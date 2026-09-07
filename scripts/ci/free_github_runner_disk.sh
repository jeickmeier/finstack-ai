#!/usr/bin/env bash
# Reclaim GitHub-hosted Ubuntu image bulk that this repository never uses.
# Hosted ci-rust compiles the workspace several times on one runner; the
# stock image's leftover Android/CodeQL/.NET/Haskell/Docker layers leave
# too little space for debug artifacts ("No space left on device").
set -euo pipefail

if [ "${GITHUB_ACTIONS:-}" != "true" ]; then
  echo "scripts/ci/free_github_runner_disk.sh runs only on GitHub Actions" >&2
  exit 1
fi

df -h

remove_path() {
  local path="$1"
  if [ -e "${path}" ]; then
    echo "removing ${path}"
    sudo rm -rf "${path}"
  fi
}

remove_path /usr/share/dotnet
remove_path /usr/local/lib/android
remove_path /opt/ghc
remove_path /usr/local/.ghcup
remove_path /usr/share/swift
remove_path /usr/local/share/boost
remove_path /opt/hostedtoolcache/CodeQL
if [ -n "${AGENT_TOOLSDIRECTORY:-}" ]; then
  remove_path "${AGENT_TOOLSDIRECTORY}"
fi

if command -v docker >/dev/null 2>&1; then
  # Leaves images that still have a running container (the ci-rust Postgres
  # service). Pre-provisioned unused images are the bulk.
  sudo docker image prune --all --force >/dev/null
fi

sudo apt-get clean >/dev/null 2>&1 || true

df -h
