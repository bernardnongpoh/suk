#!/bin/sh
# Installs Suk on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/bernardnongpoh/suk/main/install.sh | sh
#
# Options (after `sh -s --` when piping):
#   --version v0.1.0   install a specific release instead of the latest
#   --appimage         Linux: install the AppImage into ~/.local/bin (no sudo)
#
# Environment:
#   SUK_INSTALL_DIR   macOS: where the app goes (default /Applications, or ~/Applications)

set -eu

REPO="bernardnongpoh/suk"
APP_NAME="Suk"
VERSION="latest"
USE_APPIMAGE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION="$2"; shift 2 ;;
    --appimage) USE_APPIMAGE=1; shift ;;
    -h|--help)
      echo "Usage: install.sh [--version vX.Y.Z] [--appimage]"
      echo "  --version  install a specific release instead of the latest"
      echo "  --appimage Linux: install the AppImage into ~/.local/bin (no sudo)"
      exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
done

bold() { printf '\033[1m%s\033[0m\n' "$1"; }
step() { printf '  \033[35m›\033[0m %s\n' "$1"; }
fail() { printf '\033[31mError:\033[0m %s\n' "$1" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || fail "$1 is needed to install Suk."; }
need curl
need uname

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) fail "Unsupported processor: $arch. Suk runs on 64-bit Intel/AMD and ARM." ;;
esac

if [ "$VERSION" = "latest" ]; then
  api="https://api.github.com/repos/$REPO/releases/latest"
else
  api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
fi

bold "Installing $APP_NAME"
step "Looking up the ${VERSION} release…"
assets="$(curl -fsSL "$api" | grep -o '"browser_download_url": *"[^"]*"' | sed 's/.*"\(https[^"]*\)"/\1/')" \
  || fail "Couldn't reach GitHub to find the release."
[ -n "$assets" ] || fail "No release files found for ${VERSION}."

# The download for this computer, by file name pattern.
pick() { printf '%s\n' "$assets" | grep -E "$1" | head -n 1; }

download() {
  step "Downloading $(basename "$1")…"
  curl -fL --progress-bar "$1" -o "$2" || fail "Download failed."
}

install_macos() {
  case "$arch" in
    aarch64) url="$(pick '_aarch64\.dmg$')" ;;
    x86_64) url="$(pick '_x64\.dmg$')" ;;
  esac
  [ -n "$url" ] || fail "This release has no download for macOS on $arch."
  download "$url" "$TMP/app.dmg"

  dest="${SUK_INSTALL_DIR:-/Applications}"
  if [ -z "${SUK_INSTALL_DIR:-}" ] && [ ! -w "$dest" ]; then
    dest="$HOME/Applications"
  fi
  mkdir -p "$dest"

  step "Installing into ${dest}…"
  mount="$TMP/mount"
  mkdir -p "$mount"
  hdiutil attach -nobrowse -quiet -mountpoint "$mount" "$TMP/app.dmg" || fail "Couldn't open the disk image."
  rm -rf "$dest/$APP_NAME.app"
  cp -R "$mount/$APP_NAME.app" "$dest/" || { hdiutil detach -quiet "$mount"; fail "Couldn't copy the app."; }
  hdiutil detach -quiet "$mount" || true
  # Downloaded with curl, so macOS doesn't quarantine it; clear the flag in case it's there.
  xattr -dr com.apple.quarantine "$dest/$APP_NAME.app" 2>/dev/null || true

  bold "Installed $APP_NAME in $dest."
  echo "Open it from Launchpad or Spotlight, or run: open \"$dest/$APP_NAME.app\""
}

sudo_cmd() {
  if [ "$(id -u)" -eq 0 ]; then "$@"; else need sudo; sudo "$@"; fi
}

install_appimage() {
  case "$arch" in
    aarch64) url="$(pick '_aarch64\.AppImage$')" ;;
    x86_64) url="$(pick '_amd64\.AppImage$')" ;;
  esac
  [ -n "$url" ] || fail "This release has no AppImage for $arch."
  bin="$HOME/.local/bin"
  mkdir -p "$bin" "$HOME/.local/share/applications"
  download "$url" "$bin/suk"
  chmod +x "$bin/suk"
  cat > "$HOME/.local/share/applications/suk.desktop" <<DESKTOP
[Desktop Entry]
Name=$APP_NAME
Comment=Students, research, teaching and admin in one calm place
Exec=$bin/suk
Terminal=false
Type=Application
Categories=Office;Utility;
DESKTOP
  bold "Installed $APP_NAME in $bin/suk."
  echo "Start it from your applications menu, or run: suk"
  case ":$PATH:" in *":$bin:"*) ;; *) echo "(Add $bin to your PATH to run it by name.)" ;; esac
  if ! ls /usr/lib*/libfuse.so.2 /usr/lib/*/libfuse.so.2 >/dev/null 2>&1; then
    echo "If it doesn't start, install FUSE 2 (Ubuntu: sudo apt install libfuse2t64 or libfuse2)."
  fi
}

install_linux() {
  if [ "$USE_APPIMAGE" -eq 0 ] && command -v apt-get >/dev/null 2>&1; then
    case "$arch" in aarch64) pattern='_arm64\.deb$' ;; x86_64) pattern='_amd64\.deb$' ;; esac
    url="$(pick "$pattern")"
    if [ -n "$url" ]; then
      download "$url" "$TMP/suk.deb"
      step "Installing the package (you may be asked for your password)…"
      chmod 644 "$TMP/suk.deb"
      sudo_cmd apt-get install -y "$TMP/suk.deb" || fail "The package couldn't be installed."
      bold "Installed $APP_NAME."
      echo "Start it from your applications menu, or run: suk"
      return
    fi
  fi
  if [ "$USE_APPIMAGE" -eq 0 ] && { command -v dnf >/dev/null 2>&1 || command -v zypper >/dev/null 2>&1; }; then
    case "$arch" in aarch64) pattern='\.aarch64\.rpm$' ;; x86_64) pattern='\.x86_64\.rpm$' ;; esac
    url="$(pick "$pattern")"
    if [ -n "$url" ]; then
      download "$url" "$TMP/suk.rpm"
      step "Installing the package (you may be asked for your password)…"
      if command -v dnf >/dev/null 2>&1; then
        sudo_cmd dnf install -y "$TMP/suk.rpm" || fail "The package couldn't be installed."
      else
        sudo_cmd zypper --non-interactive install --allow-unsigned-rpm "$TMP/suk.rpm" || fail "The package couldn't be installed."
      fi
      bold "Installed $APP_NAME."
      echo "Start it from your applications menu, or run: suk"
      return
    fi
  fi
  install_appimage
}

case "$os" in
  Darwin) install_macos ;;
  Linux) install_linux ;;
  *) fail "Suk runs on macOS and Linux; this is $os." ;;
esac

echo
if command -v claude >/dev/null 2>&1 || [ -x "$HOME/.local/bin/claude" ] || command -v codex >/dev/null 2>&1 || [ -x "$HOME/.local/bin/codex" ]; then
  echo "When it opens, choose the assistant you use and you're ready."
else
  echo "Suk runs through Claude Code or Codex. When it opens, it will help you"
  echo "install one and sign in. You'll need a Claude or ChatGPT plan."
fi
