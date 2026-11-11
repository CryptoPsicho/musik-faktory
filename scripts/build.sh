#!/usr/bin/env bash
# Build Musik Faktory: the Dioxus web UI, then the server with the UI embedded,
# then a Debian package (the Chromebook installer).
#
#   scripts/build.sh                              # for this machine's CPU (x86_64 → amd64 .deb)
#   scripts/build.sh aarch64-unknown-linux-musl   # ARM Chromebooks (needs cargo-zigbuild + zig)
#
# The server is linked statically (musl), so it runs on any Linux of that CPU
# type, including the Chromebook's Debian, whatever glibc it has.
#
# Results in dist/:
#   musik-faktory                     the program (one self-contained file)
#   musik-faktory_<ver>_<arch>.deb    installer for the Chromebook / Debian / Ubuntu
#   install-linux.sh                  installer without root rights (alternative)
set -euo pipefail
cd "$(dirname "$0")/.."
HOST_ARCH="$(uname -m)"
TARGET="${1:-${HOST_ARCH}-unknown-linux-musl}"
TARGET_ARCH="${TARGET%%-*}"

command -v dx >/dev/null || { echo "dx (Dioxus CLI) not found: cargo install dioxus-cli --version 0.7.9 --locked"; exit 1; }
rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown || rustup target add wasm32-unknown-unknown
rustup target list --installed 2>/dev/null | grep -qx "$TARGET" || rustup target add "$TARGET"

echo "==> Building the UI (Dioxus → WebAssembly)"
rm -rf target/dx/musik-ui/release/web/public   # drop stale hashed bundles
(cd crates/ui && dx build --platform web --release)
rm -rf dist/public
mkdir -p dist
cp -r target/dx/musik-ui/release/web/public dist/public

echo "==> Building the server for $TARGET (UI embedded)"
if [[ "$TARGET_ARCH" != "$HOST_ARCH" ]]; then
  command -v cargo-zigbuild >/dev/null || {
    echo "Cross-building for $TARGET needs cargo-zigbuild and zig:"
    echo "  cargo install cargo-zigbuild && pip install ziglang"
    exit 1
  }
  cargo zigbuild --release -p musik-faktory --target "$TARGET"
else
  # ring (TLS crypto) has some C code; without a musl C compiler the system
  # gcc works fine for a same-CPU build.
  if [[ "$TARGET" == *musl* ]] && ! command -v musl-gcc >/dev/null; then
    export "CC_${TARGET//-/_}=${CC:-gcc}"
  fi
  cargo build --release -p musik-faktory --target "$TARGET"
fi
BIN="target/$TARGET/release/musik-faktory"
cp "$BIN" dist/musik-faktory.new && mv -f dist/musik-faktory.new dist/musik-faktory   # rename works even while it runs
cp scripts/install-linux.sh dist/
cp crates/server/static/icon-512.png dist/

if command -v dpkg-deb >/dev/null; then
  echo "==> Packaging the installer (.deb)"
  DEB="$(scripts/package-deb.sh "$BIN" "$TARGET")"
else
  DEB="(skipped: dpkg-deb not found)"
fi

echo
echo "Done:"
echo "  program:    dist/musik-faktory ($(du -h dist/musik-faktory | cut -f1))"
echo "  installer:  $DEB"
echo "Try it without an API key:  dist/musik-faktory serve --mock"
