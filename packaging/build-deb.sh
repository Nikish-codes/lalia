#!/usr/bin/env bash
# Build a Debian package for Lalia.
#
#   ./packaging/build-deb.sh
#
# Produces ./dist/lalia_<version>_<arch>.deb
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
arch="$(dpkg --print-architecture 2>/dev/null || echo amd64)"

echo "==> Building release binary (lalia $version, $arch)"
cargo build --release

stage="$root/dist/stage"
rm -rf "$stage"
mkdir -p "$stage/DEBIAN" \
         "$stage/usr/bin" \
         "$stage/usr/share/applications" \
         "$stage/usr/share/icons/hicolor/scalable/apps" \
         "$stage/usr/share/doc/lalia"

echo "==> Assembling package tree"
install -m 0755 "$root/target/release/lalia" "$stage/usr/bin/lalia"
install -m 0644 "$root/packaging/lalia.desktop" "$stage/usr/share/applications/lalia.desktop"
install -m 0644 "$root/packaging/lalia.svg" "$stage/usr/share/icons/hicolor/scalable/apps/lalia.svg"
# PNG icons for panels and menus that don't render SVG, drawn by the binary.
icons="$root/dist/icons"
"$root/target/release/lalia" render-icons "$icons" >/dev/null
for size in 16 22 24 32 48 64 128 256 512; do
  dir="$stage/usr/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$dir"
  install -m 0644 "$icons/lalia-$size.png" "$dir/lalia.png"
done
rm -rf "$icons"
install -m 0644 "$root/README.md" "$stage/usr/share/doc/lalia/README.md"
install -m 0644 "$root/LICENSE" "$stage/usr/share/doc/lalia/copyright"
install -m 0644 "$root/assets/fonts/Inter-LICENSE.txt" "$stage/usr/share/doc/lalia/Inter-LICENSE.txt"

cat > "$stage/DEBIAN/control" <<EOF
Package: lalia
Version: $version
Section: utils
Priority: optional
Architecture: $arch
Depends: xdotool, libasound2t64 | libasound2, libx11-6, libxcursor1, libxrandr2, libxi6, libxkbcommon-x11-0, libegl1, libgl1
Recommends: libnotify-bin, libcanberra-gtk3-module, xdg-utils
Maintainer: Lalia <noreply@localhost>
Homepage: https://github.com/Nikish-codes/lalia
Description: Lalia — system-wide live dictation for Linux (X11)
 Hold F12, speak, and the transcribed text is typed into the focused
 window. Uses Gemini 3.5 Transcribe Live (the same model Antigravity's
 record button uses). Greek λαλιά = "speech".
 .
 Lives in the system tray, shows a small on-screen indicator with a live
 level meter while you talk, and has a settings window with guided setup,
 history and every option: push-to-talk or toggle, a pause hotkey,
 streaming or on-release insertion, verbatim or smart formatting, custom
 vocabulary, microphone choice and start-on-login.
EOF

cat > "$stage/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
# Refresh the desktop database so the launcher/icon appear immediately.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
cat <<'MSG'

Lalia installed.

  Open "Lalia" from your applications menu (or run `lalia`).
  The setup window walks you through the API key and microphone.

Hold F12 to dictate. F11 pauses/resumes.
MSG
EOF
chmod 0755 "$stage/DEBIAN/postinst"


mkdir -p "$root/dist"
deb="$root/dist/lalia_${version}_${arch}.deb"
echo "==> Building $deb"
dpkg-deb --build --root-owner-group "$stage" "$deb" >/dev/null

rm -rf "$stage"
echo
echo "Done: $deb"
dpkg-deb -I "$deb" | sed -n '1,20p'
