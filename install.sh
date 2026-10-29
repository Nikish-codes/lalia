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

if [ ! -s "$HOME/.config/lalia/key" ]; then
  echo "==> No API key yet. Get one at https://aistudio.google.com/apikey"
  "$bin" set-key
fi

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Note: ~/.local/bin is not on your PATH. Add: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

cat <<EOF

Done.

  Run now:            lalia
  Quick mic test:     lalia selftest 8
  Start on login:     lalia autostart enable
  Show settings:      lalia config
  Edit settings:      ~/.config/lalia/config.toml

Hold F12 to dictate. F11 pauses/resumes.
EOF
