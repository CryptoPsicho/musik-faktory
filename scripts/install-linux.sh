#!/usr/bin/env bash
# Install Musik Faktory for the current user on a Chromebook (Linux / Crostini)
# or any Linux desktop. Run it from the unpacked `dist/` folder:
#
#   bash install-linux.sh            # local mode: app + server on this machine
#
# It installs:
#   ~/.local/bin/musik-faktory                         the program
#   ~/.local/share/applications/musik-faktory.desktop  launcher icon (ChromeOS shelf/launcher)
#   ~/.config/systemd/user/musik-faktory.service       starts the server with the Linux container
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="$HERE/musik-faktory"
[[ -x "$BIN" ]] || BIN="$HERE/../dist/musik-faktory"
[[ -x "$BIN" ]] || { echo "musik-faktory binary not found next to this script"; exit 1; }

arch="$(uname -m)"
if ! "$BIN" --version >/dev/null 2>&1; then
  echo "This binary does not run on this machine ($arch)."
  echo "On ARM Chromebooks build with: scripts/build.sh aarch64-unknown-linux-musl"
  exit 1
fi

install -Dm755 "$BIN" "$HOME/.local/bin/musik-faktory"
ICON_SRC="$HERE/icon-512.png"
[[ -f "$ICON_SRC" ]] || ICON_SRC="$HERE/../crates/server/static/icon-512.png"
install -Dm644 "$ICON_SRC" "$HOME/.local/share/icons/hicolor/512x512/apps/musik-faktory.png"

mkdir -p "$HOME/.local/share/applications"
cat > "$HOME/.local/share/applications/musik-faktory.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Musik Faktory
Comment=Make your own songs
Exec=$HOME/.local/bin/musik-faktory open
Icon=musik-faktory
Terminal=false
Categories=AudioVideo;Audio;Music;
DESKTOP

mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/musik-faktory.service" <<SERVICE
[Unit]
Description=Musik Faktory (music studio server)

[Service]
ExecStart=$HOME/.local/bin/musik-faktory serve
Restart=on-failure
Environment=RUST_LOG=musik_faktory=info

[Install]
WantedBy=default.target
SERVICE

if systemctl --user daemon-reload 2>/dev/null && systemctl --user enable --now musik-faktory.service 2>/dev/null; then
  echo "Server service enabled (starts together with the Linux container)."
else
  echo "systemd user services are not available; the launcher will start the server when needed."
fi

echo
echo "Installed. Open “Musik Faktory” from the launcher (Linux apps), or visit http://localhost:8787"
echo "First step: open the 🔒 Parents page and set the parent password + spending limits."
