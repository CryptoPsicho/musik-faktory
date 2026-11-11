#!/usr/bin/env bash
# Build a Debian package (.deb) of Musik Faktory.
#
#   scripts/package-deb.sh <binary> <rust-target-or-arch>
#
# On a Chromebook: open the Files app, double-click the .deb (or right-click →
# "Install with Linux"). It adds "Musik Faktory" to the launcher; uninstall it
# by right-clicking the icon → Uninstall. (Songs and settings stay in
# ~/.local/share/musik-faktory.)
set -euo pipefail
umask 022
cd "$(dirname "$0")/.."
BIN="${1:-dist/musik-faktory}"
TARGET="${2:-$(uname -m)}"
MAINTAINER="${DEB_MAINTAINER:-Musik Faktory <musik-faktory@localhost>}"

case "$TARGET" in
  x86_64*|amd64) ARCH=amd64 ;;
  aarch64*|arm64) ARCH=arm64 ;;
  armv7*|armhf) ARCH=armhf ;;
  *) echo "Unknown architecture for $TARGET"; exit 1 ;;
esac
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
NAME="musik-faktory_${VERSION}_${ARCH}"
STAGE="target/deb/$NAME"
rm -rf "$STAGE"

install -Dm755 "$BIN" "$STAGE/usr/bin/musik-faktory"
install -Dm644 crates/server/static/icon-512.png "$STAGE/usr/share/icons/hicolor/512x512/apps/musik-faktory.png"
install -Dm644 crates/server/static/icon-192.png "$STAGE/usr/share/icons/hicolor/192x192/apps/musik-faktory.png"
install -Dm644 crates/server/static/icon.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/musik-faktory.svg"
install -Dm644 README.md "$STAGE/usr/share/doc/musik-faktory/README.md"

install -d "$STAGE/usr/share/applications"
cat > "$STAGE/usr/share/applications/musik-faktory.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Musik Faktory
Comment=Make your own songs
Exec=/usr/bin/musik-faktory open
Icon=musik-faktory
Terminal=false
Categories=AudioVideo;Audio;Music;
DESKTOP

# The server runs as the user (its data lives in the user's home folder).
install -d "$STAGE/usr/lib/systemd/user"
cat > "$STAGE/usr/lib/systemd/user/musik-faktory.service" <<SERVICE
[Unit]
Description=Musik Faktory (music studio server)

[Service]
ExecStart=/usr/bin/musik-faktory serve
Restart=on-failure
Environment=RUST_LOG=musik_faktory=info

[Install]
WantedBy=default.target
SERVICE

chmod -R u+rwX,go+rX,go-w "$STAGE"
install -d "$STAGE/DEBIAN"
cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: musik-faktory
Version: $VERSION
Architecture: $ARCH
Maintainer: $MAINTAINER
Section: sound
Priority: optional
Installed-Size: $(du -sk "$STAGE/usr" | cut -f1)
Description: Kid-friendly music studio for the ElevenLabs Music API
 Make songs with ElevenLabs Music in a studio-style app, with parent-controlled
 weekly/monthly spending limits, a maximum track length and versioned takes.
 After installing, open "Musik Faktory" from the launcher and set it up on the
 Parents page (password, limits, ElevenLabs API key).
CONTROL

cat > "$STAGE/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if [ "$1" = "configure" ]; then
  # Start the server together with every user session (the launcher also
  # starts it on demand).
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --global enable musik-faktory.service >/dev/null 2>&1 || true
  fi
  if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database -q /usr/share/applications || true; fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t /usr/share/icons/hicolor || true; fi
fi
exit 0
POSTINST

cat > "$STAGE/DEBIAN/prerm" <<'PRERM'
#!/bin/sh
set -e
if [ "$1" = "remove" ] && command -v systemctl >/dev/null 2>&1; then
  systemctl --global disable musik-faktory.service >/dev/null 2>&1 || true
fi
exit 0
PRERM
chmod 755 "$STAGE/DEBIAN/postinst" "$STAGE/DEBIAN/prerm"

mkdir -p dist
dpkg-deb --root-owner-group --build "$STAGE" "dist/$NAME.deb" >/dev/null
echo "dist/$NAME.deb"
