#!/usr/bin/env bash
# Downloads a speech model for Hushnote into ~/.local/share/hushnote/models/, checked with sha256.
# These are the exact files Hex pins (same repositories, revisions and checksums):
#
#   scripts/download-model.sh            Parakeet v3, 25 European languages incl. Spanish (default)
#   scripts/download-model.sh english    Parakeet Unified English, Hex's pick for English
#   scripts/download-model.sh whisper    Whisper large-v3-turbo, Hex's pick for other languages
#   scripts/download-model.sh vad        only Silero VAD (1.2 MB; every model above also gets it)
#
# Then point `speech_model` in ~/.config/hushnote/config.json at the file (or pick it in the app).
# The summaries use a local LLM you install separately, for example: ollama pull qwen3:4b-instruct-2507-q4_K_M (or from Home in the app)
set -euo pipefail

case "${1:-v3}" in
  v3 | parakeet)
    REPO="handy-computer/parakeet-tdt-0.6b-v3-gguf"
    REV="85ac09ea12fc4b1112fa76810059364bc6adc9de"
    FILE="parakeet-tdt-0.6b-v3-Q8_0.gguf"
    SHA256="5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7"
    SIZE="740 MB" ;;
  english | en)
    REPO="handy-computer/parakeet-unified-en-0.6b-gguf"
    REV="7e948f21b7bdbac698d3318db9d350f1096f3b6c"
    FILE="parakeet-unified-en-0.6b-Q8_0.gguf"
    SHA256="4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795"
    SIZE="731 MB" ;;
  whisper)
    REPO="handy-computer/whisper-large-v3-turbo-gguf"
    REV="d222c9f621c1128299248f2ded4d8a1820519780"
    FILE="whisper-large-v3-turbo-Q8_0.gguf"
    SHA256="d5e65f2b0828802ae2c231673d31982cebe3a778c95d9494a9f3efee6bd17448"
    SIZE="886 MB" ;;
  vad)
    REPO="" ;;
  -h | --help)
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
    exit 0 ;;
  *)
    echo "Unknown model '$1'. Use: v3, english, whisper or vad." >&2
    exit 2 ;;
esac

DIR="${XDG_DATA_HOME:-$HOME/.local/share}/hushnote/models"
mkdir -p "$DIR"

# fetch REPO REV FILE SHA256 SIZE
fetch() {
  local repo=$1 rev=$2 file=$3 sha=$4 size=$5
  local dest="$DIR/$file"
  if [[ -f "$dest" ]] && echo "$sha  $dest" | sha256sum --check --status; then
    echo "Already downloaded: $dest"
    return
  fi
  echo "Downloading $file (~$size) to $DIR"
  # Resumes a partial download if there is one.
  curl --fail --location --continue-at - --progress-bar --output "$dest.part" \
    "https://huggingface.co/$repo/resolve/$rev/$file"
  if ! echo "$sha  $dest.part" | sha256sum --check --status; then
    echo "The checksum of $file does not match. Delete $dest.part and run this again." >&2
    exit 1
  fi
  mv "$dest.part" "$dest"
  echo "Done: $dest"
}

if [[ -n "$REPO" ]]; then
  fetch "$REPO" "$REV" "$FILE" "$SHA256" "$SIZE"
fi
# Silero VAD v6.2, which Hex downloads with every model: phrases without speech are skipped
# instead of transcribed (models invent text for silence).
fetch handy-computer/silero-vad-v6.2-gguf a9b2d61c20ae4ddf3644edb34969e8185a485448 \
  silero-vad-v6.2-F32.gguf 0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6 "1.2 MB"
