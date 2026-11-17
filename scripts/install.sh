#!/usr/bin/env bash
# Installs Hushnote for this user: the binary in ~/.local/bin and an entry in the app launcher
# (Walker on Omarchy, or any launcher that reads ~/.local/share/applications).
#
# From the release tarball:   ./install.sh
# From the repository:        scripts/install.sh   (builds it first if needed)
# Remove the app:             ./install.sh --uninstall
# Remove everything:          ./install.sh --uninstall --purge   (see docs/install.md)
#
# The speech model downloads from the app in one click (Home › Set up).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop="$data/applications/hushnote.desktop"
icon="$data/icons/hicolor/256x256/apps/hushnote.png"

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/hushnote"
app_data="$data/hushnote"

if [[ " $* " == *" --uninstall "* ]]; then
    # The meetings folder, from the settings (it may have been moved).
    notes=""
    if [[ -f "$config_dir/config.json" ]]; then
        notes=$(sed -n 's/^ *"notes_dir": *"\(.*\)",\{0,1\}$/\1/p' "$config_dir/config.json" | head -1)
    fi
    if [[ -z "$notes" ]]; then
        docs=$(command -v xdg-user-dir >/dev/null && xdg-user-dir DOCUMENTS || echo "$HOME/Documents")
        notes="$docs/Hushnote"
    fi

    rm -f "$bin_dir/hushnote" "$desktop" "$icon"
    command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    echo "Removed the app."

    if [[ " $* " != *" --purge "* ]]; then
        echo "Kept your meetings ($notes), the speech models ($app_data) and settings ($config_dir)."
        echo "Run with --uninstall --purge to remove those too."
        exit 0
    fi

    rm -rf "$app_data" "$config_dir"
    echo "Removed the speech models and settings."

    if [[ -d "$notes" ]]; then
        count=$(find "$notes" -maxdepth 1 -name '*.md' | wc -l)
        read -r -p "Delete your $count meeting(s) in $notes? This can't be undone. [y/N] " answer
        if [[ "$answer" =~ ^[yY] ]]; then
            rm -rf "$notes"
            echo "Deleted the meetings."
        else
            echo "Kept the meetings in $notes."
        fi
    fi

    if command -v ollama >/dev/null && ollama list 2>/dev/null | grep -q '^qwen3:4b-instruct-2507'; then
        read -r -p "Remove the summary model from Ollama (qwen3:4b-instruct-2507, 2.5 GB)? [y/N] " answer
        if [[ "$answer" =~ ^[yY] ]]; then
            ollama rm qwen3:4b-instruct-2507-q4_K_M >/dev/null && echo "Removed the summary model."
        fi
    fi
    echo "Done. Ollama itself, if you installed it for Hushnote, is removed with: sudo pacman -Rns ollama"
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
