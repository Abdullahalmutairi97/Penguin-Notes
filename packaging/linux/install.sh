#!/bin/sh
# Installs Penguin Notes for you (no sudo needed). Either run it from an unpacked release:
#   sh install.sh
# or straight from the internet:
#   curl -fsSL https://github.com/Abdullahalmutairi97/Penguin-Notes/releases/latest/download/install.sh | sh
# To remove the app again (your notes stay):
#   sh install.sh --uninstall
set -eu

REPO=Abdullahalmutairi97/Penguin-Notes
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
APP="$DATA/penguin-notes-app"
BIN="$HOME/.local/bin"
APPS="$DATA/applications"
ICONS="$DATA/icons/hicolor/256x256/apps"

if [ "${1:-}" = "--uninstall" ]; then
    rm -rf "$APP"
    rm -f "$BIN/penguin-notes" "$APPS/penguin-notes.desktop" "$ICONS/penguin-notes.png"
    echo "Penguin Notes is removed. Your notes are still in $DATA/penguin-notes."
    exit 0
fi

if [ "$(uname -m)" != x86_64 ]; then
    echo "Sorry: Penguin Notes is only built for 64-bit Intel/AMD computers (x86_64) for now."
    exit 1
fi

# Use the files next to this script when it came in a release download; fetch them otherwise.
here=$(cd "$(dirname "$0")" 2>/dev/null && pwd || true)
if [ -n "$here" ] && [ -x "$here/penguin-notes" ] && [ -d "$here/whisper" ]; then
    src=$here
else
    command -v curl >/dev/null || { echo "Please install curl first."; exit 1; }
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    echo "Downloading Penguin Notes..."
    curl -fL# "https://github.com/$REPO/releases/latest/download/penguin-notes-linux-x86_64.tar.gz" | tar -xz -C "$tmp"
    src="$tmp/penguin-notes"
fi

rm -rf "$APP"
mkdir -p "$APP" "$BIN" "$APPS" "$ICONS"
cp -a "$src/." "$APP/"
ln -sf "$APP/penguin-notes" "$BIN/penguin-notes"
cp "$APP/icon.png" "$ICONS/penguin-notes.png"
cat > "$APPS/penguin-notes.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Penguin Notes
Comment=Markdown notes with a local AI sidebar
Exec=$APP/penguin-notes
Icon=penguin-notes
Terminal=false
Categories=Office;TextEditor;
StartupWMClass=penguin-notes
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APPS" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$DATA/icons/hicolor" 2>/dev/null || true

echo "Penguin Notes is installed. Open it from your app menu."
if ! command -v pw-record >/dev/null && ! command -v arecord >/dev/null; then
    echo "For talking to Pengu, install pipewire-bin or alsa-utils (e.g. sudo apt install alsa-utils)."
fi
