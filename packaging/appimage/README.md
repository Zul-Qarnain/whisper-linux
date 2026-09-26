# AppImage

```sh
cargo build --release
mkdir -p AppDir/usr/bin AppDir/usr/share/applications AppDir/usr/share/icons/hicolor/256x256/apps
cp target/release/whisper-flow AppDir/usr/bin/
cp packaging/deb/whisper-flow.desktop AppDir/usr/share/applications/
cp assets/icon-256.png AppDir/usr/share/icons/hicolor/256x256/apps/whisper-flow.png 2>/dev/null || true

cat > AppDir/AppRun <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/whisper-flow" "$@"
EOF
chmod +x AppDir/AppRun

appimagetool AppDir   # AUR: appimagetool; produces WhisperFlow-x86_64.AppImage
```

Runtime deps satisfied by any desktop: PipeWire/ALSA (via cpal), libwayland (gpui). `wtype` (Wayland paste) or `xdotool` (X11 paste) suggested, with clipboard-only fallback if absent.
