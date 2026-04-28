#!/usr/bin/env bash
set -euo pipefail

# ── colors ────────────────────────────────────────────────────────────────────
PURPLE='\033[1;35m'
BLUE='\033[0;34m'
GREEN='\033[0;32m'
RED='\033[0;31m'
DIM='\033[2;37m'
RESET='\033[0m'

# ── helpers ───────────────────────────────────────────────────────────────────
info()    { printf "  ${BLUE}•${RESET} %b\n" "$*"; }
success() { printf "  ${GREEN}✓${RESET} %b\n" "$*"; }
die()     { printf "\n  ${RED}✗ error:${RESET} %s\n\n" "$*" >&2; exit 1; }
dim()     { printf "    ${DIM}%s${RESET}\n" "$*"; }

banner() {
  printf "\n"
  printf "  ${PURPLE}██╗  ██╗ ██████╗  ██████╗ █████╗${RESET}\n"
  printf "  ${PURPLE}██║ ██╔╝██╔═══██╗██╔════╝██╔══██╗${RESET}\n"
  printf "  ${PURPLE}█████╔╝ ██║   ██║██║     ███████║${RESET}\n"
  printf "  ${PURPLE}██╔═██╗ ██║   ██║██║     ██╔══██║${RESET}\n"
  printf "  ${PURPLE}██║  ██╗╚██████╔╝╚██████╗██║  ██║${RESET}\n"
  printf "  ${PURPLE}╚═╝  ╚═╝ ╚═════╝  ╚═════╝╚═╝  ╚═╝${RESET}\n"
  printf "\n"
  printf "  ${DIM}the koca installer${RESET}\n"
  printf "\n"
}

# ── resolve version ───────────────────────────────────────────────────────────
resolve_version() {
  if [[ -n "${KOCA_VERSION:-}" ]]; then
    echo "${KOCA_VERSION#v}"
    return
  fi
  local tag
  tag=$(curl -fsSL "https://api.github.com/repos/koca-build/koca/releases/latest" \
    | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"v\?\([^"]*\)".*/\1/')
  [[ -n "$tag" ]] || die "could not resolve latest release version"
  echo "$tag"
}

# ── detect arch ───────────────────────────────────────────────────────────────
detect_arch() {
  local arch
  arch="$(uname -m)"
  case "$arch" in
    x86_64)  echo "x86_64" ;;
    aarch64) echo "aarch64" ;;
    arm64)   echo "aarch64" ;;
    *)       die "unsupported architecture: $arch" ;;
  esac
}

# nfpm uses amd64/arm64 for .deb, x86_64/aarch64 for .rpm
deb_arch() {
  case "$1" in
    x86_64)  echo "amd64" ;;
    aarch64) echo "arm64" ;;
  esac
}

# ── detect install method ─────────────────────────────────────────────────────
detect_method() {
  if command -v dpkg &>/dev/null; then
    echo "deb"
  elif command -v rpm &>/dev/null; then
    echo "rpm"
  else
    echo "binary"
  fi
}

# ── download helper ───────────────────────────────────────────────────────────
download() {
  local url="$1" dest="$2"
  dim "$url"
  curl -fsSL -o "$dest" "$url" || die "download failed: $url"
}

# ── install ───────────────────────────────────────────────────────────────────
main() {
  banner

  local version method arch tmpdir
  version="$(resolve_version)"
  arch="$(detect_arch)"
  method="$(detect_method)"

  info "version  ${PURPLE}v${version}${RESET}"
  info "arch     ${arch}"

  tmpdir="$(mktemp -d)"
  trap '[[ -d "${tmpdir:-}" ]] && rm -rf "$tmpdir"' EXIT

  local base_url="https://github.com/koca-build/koca/releases/download/v${version}"

  case "$method" in
    deb)
      local arch_deb
      arch_deb="$(deb_arch "$arch")"
      info "method   deb package"

      local koca_deb="${tmpdir}/koca.deb"
      download "${base_url}/koca_${version}-1_${arch_deb}.deb" "$koca_deb"

      local backend_deb="${tmpdir}/koca-backend-apt.deb"
      download "${base_url}/koca-backend-apt_${version}-1_${arch_deb}.deb" "$backend_deb"

      info "installing via dpkg..."
      sudo dpkg -i "$koca_deb" "$backend_deb"
      ;;
    rpm)
      info "method   rpm package"

      local koca_rpm="${tmpdir}/koca.rpm"
      download "${base_url}/koca_${version}-1_${arch}.rpm" "$koca_rpm"

      local backend_rpm="${tmpdir}/koca-backend-apt.rpm"
      download "${base_url}/koca-backend-apt_${version}-1_${arch}.rpm" "$backend_rpm"

      info "installing via rpm..."
      if command -v dnf &>/dev/null; then
        sudo dnf install -y "$koca_rpm" "$backend_rpm"
      elif command -v yum &>/dev/null; then
        sudo yum install -y "$koca_rpm" "$backend_rpm"
      else
        sudo rpm -i "$koca_rpm" "$backend_rpm"
      fi
      ;;
    binary)
      info "method   binary"
      local bin_file="${tmpdir}/koca"
      download "${base_url}/koca-${arch}" "$bin_file"
      chmod +x "$bin_file"
      info "installing to /usr/local/bin/koca..."
      sudo mv "$bin_file" /usr/local/bin/koca
      ;;
  esac

  printf "\n"
  success "koca installed successfully"
  dim "$(koca --version 2>/dev/null || true)"
  printf "\n"
}

main "$@"
