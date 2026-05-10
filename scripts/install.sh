#!/usr/bin/env bash
# Install dragon-head-mcp from GitHub Releases.
# Usage: curl -fsSL https://raw.githubusercontent.com/takurot/dragon-head/main/scripts/install.sh | bash
set -euo pipefail

REPO="takurot/dragon-head"
BINARY="dragon-head-mcp"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"

# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
  Darwin)
    case "${ARCH}" in
      arm64)  ARTIFACT="${BINARY}-macos-arm64" ;;
      x86_64) ARTIFACT="${BINARY}-macos-x64" ;;
      *)      echo "Unsupported macOS arch: ${ARCH}" >&2; exit 1 ;;
    esac
    ;;
  Linux)
    case "${ARCH}" in
      x86_64)  ARTIFACT="${BINARY}-linux-x64" ;;
      aarch64) ARTIFACT="${BINARY}-linux-arm64" ;;
      *)       echo "Unsupported Linux arch: ${ARCH}" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: ${OS}. Use the GitHub Releases page to download manually." >&2
    exit 1
    ;;
esac

# Resolve latest release tag
TAG="${VERSION:-}"
if [ -z "${TAG}" ]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/')"
fi

if [ -z "${TAG}" ]; then
  echo "Could not determine latest release tag. Set VERSION to override." >&2
  exit 1
fi

BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
DOWNLOAD_URL="${BASE_URL}/${ARTIFACT}"
CHECKSUM_URL="${BASE_URL}/${ARTIFACT}.sha256"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

echo "Downloading ${ARTIFACT} ${TAG}..."
curl -fsSL -o "${TMP_DIR}/${BINARY}" "${DOWNLOAD_URL}"
curl -fsSL -o "${TMP_DIR}/${ARTIFACT}.sha256" "${CHECKSUM_URL}"

echo "Verifying checksum..."
EXPECTED="$(awk '{print $1}' "${TMP_DIR}/${ARTIFACT}.sha256")"
# sha256sum is standard on Linux; macOS < 13 uses shasum -a 256
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "${TMP_DIR}/${BINARY}" | awk '{print $1}')"
else
  ACTUAL="$(shasum -a 256 "${TMP_DIR}/${BINARY}" | awk '{print $1}')"
fi
if [ "${EXPECTED}" != "${ACTUAL}" ]; then
  echo "Checksum mismatch! Expected: ${EXPECTED}  Got: ${ACTUAL}" >&2
  exit 1
fi

chmod +x "${TMP_DIR}/${BINARY}"

mkdir -p "${INSTALL_DIR}"
echo "Installing to ${INSTALL_DIR}/${BINARY}..."
if [ -w "${INSTALL_DIR}" ]; then
  mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  sudo mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

echo ""
echo "Installed ${BINARY} ${TAG} to ${INSTALL_DIR}/${BINARY}"
echo "Run '${BINARY} --doctor' to verify the installation."
