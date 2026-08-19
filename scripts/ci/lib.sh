# Shared helpers for root mise test/CI tasks. Source from the repository root.
#   . ./scripts/ci/lib.sh

# Install Playwright browser binaries. `--with-deps` (apt) is used only when
# FINSTACK_PLAYWRIGHT_WITH_DEPS=1, typically a CI cache miss. `playwright
# install` without that flag is incremental and no-ops present browsers.
ensure_playwright_browsers() {
  local prefix="bindings/finstack-ai-wasm/js"
  if [ "${FINSTACK_PLAYWRIGHT_SKIP_INSTALL:-0}" = "1" ]; then
    return 0
  fi
  if [ "$#" -eq 0 ]; then
    set -- chromium firefox webkit
  fi
  if [ "${FINSTACK_PLAYWRIGHT_WITH_DEPS:-0}" = "1" ]; then
    npx --prefix "${prefix}" playwright install --with-deps "$@"
    return
  fi
  npx --prefix "${prefix}" playwright install "$@"
}

# True when consecutive WASM glue builds must be compared.
# FINSTACK_WASM_REPRO=1 forces on; =0 forces off. Unset runs the compare only
# when glue-related paths changed versus the merge base (fail open).
wasm_repro_needed() {
  case "${FINSTACK_WASM_REPRO:-}" in
    1) return 0 ;;
    0) return 1 ;;
  esac
  local range=""
  if [ -n "${GITHUB_EVENT_BEFORE:-}" ] \
    && [ "${GITHUB_EVENT_BEFORE}" != "null" ] \
    && [ "${GITHUB_EVENT_BEFORE}" != "0000000000000000000000000000000000000000" ]; then
    range="${GITHUB_EVENT_BEFORE}...HEAD"
  elif [ -n "${GITHUB_BASE_REF:-}" ] && git rev-parse --verify "origin/${GITHUB_BASE_REF}" >/dev/null 2>&1; then
    range="origin/${GITHUB_BASE_REF}...HEAD"
  elif git rev-parse --verify origin/main >/dev/null 2>&1; then
    range="origin/main...HEAD"
  else
    return 0
  fi
  local changed=""
  changed="$(git diff --name-only "${range}" 2>/dev/null)" || return 0
  printf '%s\n' "${changed}" | grep -Eq \
    '^(bindings/finstack-ai-wasm/|scripts/wasm_package/|mise.toml|Cargo.toml|Cargo.lock)'
}
