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
         "$stage/usr/lib/systemd/user" \
         "$stage/usr/share/doc/lalia"

echo "==> Assembling package tree"
install -m 0755 "$root/target/release/lalia" "$stage/usr/bin/lalia"
install -m 0644 "$root/packaging/lalia.desktop" "$stage/usr/share/applications/lalia.desktop"
install -m 0644 "$root/packaging/lalia.svg" "$stage/usr/share/icons/hicolor/scalable/apps/lalia.svg"
install -m 0644 "$root/packaging/lalia.service" "$stage/usr/lib/systemd/user/lalia.service"
install -m 0644 "$root/README.md" "$stage/usr/share/doc/lalia/README.md"

cat > "$stage/DEBIAN/control" <<EOF
Package: lalia
Version: $version
Section: utils
Priority: optional
Architecture: $arch
Depends: xdotool, libasound2
Recommends: libnotify-bin, libcanberra-gtk3-module
Maintainer: Lalia <noreply@localhost>
Homepage: https://github.com/Nikish-codes/lalia
Description: Lalia — system-wide live dictation for Linux (X11)
 Hold F12, speak, and the transcribed text is typed into the focused
 window. Uses Gemini 3.5 Transcribe Live (the same model Antigravity's
 record button uses). Greek λαλιά = "speech".
 .
 Features: push-to-talk or toggle, a pause hotkey, streaming or on-release
 insertion, verbatim or smart formatting, custom vocabulary, desktop
 notifications and sound, and optional start-on-login.
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

  1) Store your Gemini API key:   lalia set-key
     (get one at https://aistudio.google.com/apikey)
  2) Run it:                      lalia
  3) Start at login:             lalia autostart enable

Hold F12 to dictate. F11 pauses/resumes.
MSG
EOF
chmod 0755 "$stage/DEBIAN/postinst"

cat > "$stage/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
# Stop any user service on uninstall (best effort, non-fatal).
systemctl --user disable --now lalia.service 2>/dev/null || true
exit 0
EOF
chmod 0755 "$stage/DEBIAN/prerm"

mkdir -p "$root/dist"
deb="$root/dist/lalia_${version}_${arch}.deb"
echo "==> Building $deb"
dpkg-deb --build --root-owner-group "$stage" "$deb" >/dev/null

rm -rf "$stage"
echo
echo "Done: $deb"
dpkg-deb -I "$deb" | sed -n '1,20p'
