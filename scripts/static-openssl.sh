#!/bin/sh
# Builds OpenSSL as static libraries for macOS release builds, and prints the directory to use as
# OPENSSL_DIR. The embedded database links OpenSSL; linking it statically means the app doesn't
# depend on Homebrew's copy, which most Macs don't have.
#
#   export OPENSSL_DIR="$(scripts/static-openssl.sh)"
set -eu

VERSION="3.5.8"
MIN_MACOS="13.3"
PREFIX="${1:-$(pwd)/.build/openssl-$VERSION-static}"

if [ -f "$PREFIX/lib/libssl.a" ] && [ -f "$PREFIX/lib/libcrypto.a" ]; then
  echo "$PREFIX"
  exit 0
fi

case "$(uname -m)" in
  arm64) target=darwin64-arm64-cc ;;
  x86_64) target=darwin64-x86_64-cc ;;
  *) echo "Unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL "https://github.com/openssl/openssl/releases/download/openssl-$VERSION/openssl-$VERSION.tar.gz" | tar xz -C "$work"
cd "$work/openssl-$VERSION"
MACOSX_DEPLOYMENT_TARGET="$MIN_MACOS" ./Configure "$target" no-shared no-tests no-docs no-apps \
  "-mmacosx-version-min=$MIN_MACOS" --prefix="$PREFIX" --libdir=lib >&2
make -j"$(sysctl -n hw.ncpu)" build_libs >&2
make install_dev >&2
echo "$PREFIX"
