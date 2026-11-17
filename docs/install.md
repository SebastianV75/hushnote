# Installing and uninstalling Hushnote

## Install

Linux x86-64 with an AVX2 CPU (Intel since 2013, AMD since 2015) and PipeWire, e.g. Omarchy.

```sh
curl -LO https://github.com/SebastianV75/hushnote/releases/download/v0.1.0/hushnote-0.1.0-linux-x86_64.tar.gz
tar xzf hushnote-0.1.0-linux-x86_64.tar.gz
cd hushnote-0.1.0-linux-x86_64 && ./install.sh
```

Open **Hushnote** from the launcher (Super + Space on Omarchy) or run `hushnote`. On first run,
download the speech model from Home › Set up.

From a clone of the repository, `scripts/install.sh` does the same and builds the app first if
needed.

## What gets installed, and where

| What | Where | Removed by |
|---|---|---|
| The app | `~/.local/bin/hushnote` | `--uninstall` |
| Launcher entry and icon | `~/.local/share/applications/hushnote.desktop`, `~/.local/share/icons/hicolor/256x256/apps/hushnote.png` | `--uninstall` |
| Speech models (Parakeet, Silero VAD), ~740 MB each | `~/.local/share/hushnote/` | `--uninstall --purge` |
| Settings | `~/.config/hushnote/` | `--uninstall --purge` |
| Your meetings (Markdown) | `~/Documents/Hushnote/` (or the `notes_dir` in the settings) | `--uninstall --purge`, only if you say yes |
| Summary model in Ollama (if downloaded), 2.5 GB | Ollama's store | `--uninstall --purge`, only if you say yes |
| Ollama itself (if you installed it for Hushnote) | system package | `sudo pacman -Rns ollama` |

## Uninstall

Remove the app and keep your meetings, models and settings (to reinstall later):

```sh
./install.sh --uninstall
```

## Uninstall completely

Remove the app, the speech models and the settings. It asks before deleting your meetings and
before removing the summary model from Ollama:

```sh
./install.sh --uninstall --purge
```

Run it from the folder you installed from, or from `scripts/` in a clone. If you no longer have
either, download the release again, or remove everything by hand:

```sh
rm -f ~/.local/bin/hushnote \
      ~/.local/share/applications/hushnote.desktop \
      ~/.local/share/icons/hicolor/256x256/apps/hushnote.png
rm -rf ~/.local/share/hushnote ~/.config/hushnote
rm -rf ~/Documents/Hushnote                        # your meetings: only if you want them gone
ollama rm qwen3:4b-instruct-2507-q4_K_M            # the summary model, if you downloaded it
sudo systemctl disable --now ollama && sudo pacman -Rns ollama   # Ollama, if only Hushnote used it
```

Hushnote doesn't install anything else: no system packages, services or files outside your home.
