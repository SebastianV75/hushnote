#!/usr/bin/env bash
# Installs Hushnote for this user: the binary in ~/.local/bin and an entry in the app launcher
# (Walker on Omarchy, or any launcher that reads ~/.local/share/applications).
#
# From the release tarball:   ./install.sh
# From the repository:        scripts/install.sh   (builds it first if needed)
# Remove:                     ./install.sh --uninstall
#
# The speech model downloads from the app in one click (Home › Set up).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop="$data/applications/hushnote.desktop"
icon="$data/icons/hicolor/256x256/apps/hushnote.png"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$bin_dir/hushnote" "$desktop" "$icon"
    echo "Removed Hushnote. Your meetings and models stay in ~/Documents/Hushnote and $data/hushnote."
    exit 0
fi

if [[ -x "$here/hushnote" ]]; then
    binary="$here/hushnote"
    logo="$here/hushnote.png"
else
    root="$(cd "$here/.." && pwd)"
    if [[ ! -x "$root/target/release/hushnote" ]]; then
        echo "Building Hushnote (first time takes a few minutes)…"
        (cd "$root" && cargo build --release)
    fi
    binary="$root/target/release/hushnote"
    logo="$root/assets/logo.png"
fi

install -Dm755 "$binary" "$bin_dir/hushnote"
[[ -f "$logo" ]] && install -Dm644 "$logo" "$icon"
mkdir -p "$(dirname "$desktop")"
cat > "$desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Hushnote
Comment=Meeting notes, transcribed on your machine
Exec=$bin_dir/hushnote
Icon=hushnote
Terminal=false
Categories=Office;AudioVideo;
Keywords=meeting;notes;transcribe;transcript;
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" >/dev/null 2>&1 || true

echo "Installed: $bin_dir/hushnote"
echo "Open it from the launcher (Super + Space on Omarchy) or run: hushnote"
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "Note: $bin_dir is not on your PATH." ;; esac
command -v pw-record >/dev/null || echo "Note: pw-record not found; install pipewire to record."
