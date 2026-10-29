#!/usr/bin/env bash
# Developer install: build Lalia, put the binary in ~/.local/bin, and offer to
# enable start-on-login. For normal users, prefer the .deb (see README).
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
bin="$HOME/.local/bin/lalia"

echo "==> Building (release)…"
cargo build --release --manifest-path "$here/Cargo.toml"

echo "==> Installing to $bin"
mkdir -p "$HOME/.local/bin"
install -m 0755 "$here/target/release/lalia" "$bin"

# Menu entry + icon so Lalia shows up in the applications menu.
apps="$HOME/.local/share/applications"
mkdir -p "$apps" "$HOME/.local/share/icons/hicolor/scalable/apps"
sed "s|^Exec=lalia|Exec=$bin|" "$here/packaging/lalia.desktop" > "$apps/lalia.desktop"
install -m 0644 "$here/packaging/lalia.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/lalia.svg"

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Note: ~/.local/bin is not on your PATH. Add: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

cat <<EOF

Done.

  Run now:            lalia          (opens setup the first time)
  Settings window:    lalia gui
  Quick mic test:     lalia selftest 8
  Start on login:     lalia autostart enable

Hold F12 to dictate. F11 pauses/resumes. Lalia lives in your tray.
EOF
