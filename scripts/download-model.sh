#!/usr/bin/env bash
# Downloads a speech model for Hushnote into ~/.local/share/hushnote/models/, checked with sha256.
# These are the exact files Hex pins (same repositories, revisions and checksums):
#
#   scripts/download-model.sh            Parakeet v3, 25 European languages incl. Spanish (default)
#   scripts/download-model.sh english    Parakeet Unified English, Hex's pick for English
#   scripts/download-model.sh whisper    Whisper large-v3-turbo, Hex's pick for other languages
#
# Then point `speech_model` in ~/.config/hushnote/config.json at the file (or pick it in the app).
# The summaries use a local LLM you install separately, for example: ollama pull qwen3:8b
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
  -h | --help)
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 0 ;;
  *)
    echo "Unknown model '$1'. Use: v3, english or whisper." >&2
    exit 2 ;;
esac

URL="https://huggingface.co/${REPO}/resolve/${REV}/${FILE}"
DIR="${XDG_DATA_HOME:-$HOME/.local/share}/hushnote/models"
DEST="$DIR/$FILE"

check() {
  echo "${SHA256}  $1" | sha256sum --check --status
}

mkdir -p "$DIR"
if [[ -f "$DEST" ]] && check "$DEST"; then
  echo "Already downloaded: $DEST"
  exit 0
fi

echo "Downloading $FILE (~$SIZE) to $DIR"
# Resumes a partial download if there is one.
curl --fail --location --continue-at - --progress-bar --output "$DEST.part" "$URL"

if ! check "$DEST.part"; then
  echo "The checksum does not match. Delete $DEST.part and run this again." >&2
  exit 1
fi
mv "$DEST.part" "$DEST"
echo "Done: $DEST"
