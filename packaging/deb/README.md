# .deb

```
package/
├── DEBIAN/control
└── usr/bin/whisper-flow
└── usr/share/applications/whisper-flow.desktop
```

`DEBIAN/control`:

```
Package: whisper-flow
Version: 0.1.0
Section: sound
Priority: optional
Architecture: amd64
Depends: libasound2t64 | libpipewire-0.3-0t64, libwayland-client0, libxkbcommon0
Suggests: wtype | xdotool
Maintainer: you <you@example.com>
Description: Offline voice typing with whisper.cpp
 Hold a shortcut, speak, and the transcription is pasted into the focused
 app. Runs offline after a one-time ~32 MB model download.
```

```sh
cargo build --release
mkdir -p package/DEBIAN package/usr/bin package/usr/share/applications
cp target/release/whisper-flow package/usr/bin/
cp packaging/deb/whisper-flow.desktop package/usr/share/applications/
# write DEBIAN/control as above
dpkg-deb --build package whisper-flow_0.1.0_amd64.deb
sudo dpkg -i whisper-flow_0.1.0_amd64.deb
```

`whisper-flow.desktop`:

```
[Desktop Entry]
Type=Application
Name=Whisper Flow
Exec=whisper-flow run
Icon=whisper-flow
Categories=Utility;AudioVideo;
```
