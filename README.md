# Whisper Flow Linux

Native Rust voice typing for Linux — hold a shortcut, speak, text appears in the focused app.
Powered by [whisper.cpp](https://github.com/ggml-org/whisper.cpp) (offline) and [GPUI](https://github.com/zed-industries/zed). **Entire program stays under 200 MB RAM** (measured peak: 136 MB with the default model; see [Performance](#performance)).

## How it works

```
KDE/X11 shortcut → whisper-flow toggle → mic capture (16 kHz mono)
  → energy VAD endpointing → persistent whisper.cpp worker (tiny.en Q5)
  → clipboard + synthesized Ctrl+V → your clipboard is restored
```

One binary, one crate, zero background services: capture starts only when you trigger it, and the model unloads itself after idle.

## Build

```sh
sudo pacman -S base-devel cmake alsa-lib        # Arch
# Debian/Ubuntu: sudo apt install build-essential cmake libasound2-dev libxkbcommon-dev libwayland-dev

cargo build --release
```

Binary lands at `target/release/whisper-flow`. The CPU-only backend is the default — no GPU needed (it will use one if present later; not required).

## Run

```sh
./target/release/whisper-flow run        # starts the daemon (tray-free; tiny idle footprint)
```

Then bind your shortcut:

- **KDE/Wayland** (global hotkeys cannot be grabbed from apps): System Settings → Shortcuts → Custom Shortcuts → new *Command/URL* → command `whisper-flow toggle` (full path, e.g. `~/wishper-linux/target/release/whisper-flow toggle`) → assign e.g. `Meta+Shift+Space`.
- **X11**: same command works; any WM keybinding tool (`sxhkd`, KDE on X11) can call `whisper-flow toggle`.

Toggle starts recording (an overlay pill appears at the bottom of the screen); toggling again (or ~0.7 s of silence, max 30 s per clip) transcribes and pastes into whatever has focus.

### Wayland overlay z-order

GPUI 0.2.2 has no always-on-top flag for native Wayland windows. Two options:

1. **Zero-config**: start the daemon as `WHISPER_FLOW_X11=1 whisper-flow run` — the pill opens as an XWayland notification window, which KWin keeps above everything without stealing focus.
2. **Native Wayland**: add a KDE window rule once — System Settings → Window Management → Window Rules → match app_id `whisper-flow` → *Keep above other windows* + *No titlebar*.

## Commands

| Command | Purpose |
|---|---|
| `whisper-flow run` | start the daemon |
| `whisper-flow toggle` | start/stop a dictation (bind to a shortcut) |
| `whisper-flow settings` | open the settings window |
| `whisper-flow quit` | stop the daemon |
| `whisper-flow models list / download <id> / remove <id>` | model management |
| `whisper-flow transcribe-file <wav> [--model tiny.en-q5] [--threads 4]` | offline transcribe + RAM report |
| `whisper-flow record <out.wav> [--transcribe]` | capture one utterance from the mic |

Models: `tiny.en-q5` (default, ~136 MB peak), `base.en-q5` (~190 MB peak), `small.en-q5` (**exceeds the 200 MB budget** — opt-in only). Stored in `~/.local/share/whisper-flow/models/`, downloaded from HuggingFace; everything after that is fully offline.

## Settings

`~/.config/whisper-flow/settings.json` (model, language `en`/`auto`, microphone, `idle_unload_secs`, `vad_threshold`, `silence_timeout_ms`, `start_on_login`). The GPUI settings window edits the same file: pick model/mic/language, download/remove models, see live RSS/peak in MB, toggle autostart.

## Architecture

Single crate, module-per-responsibility (the 10-crate skeleton was consolidated — same seams, far less ceremony):

```
src/
  main.rs       CLI dispatch (daemon / socket client / offline tools)
  app.rs        GPUI: overlay pill (AppState) + settings window; event pump
  events.rs     UiEvent bus (futures mpsc; the only way into the UI thread)
  audio.rs      cpal capture, resample to 16k, endpointing (preroll 300ms)
  vad.rs        RMS energy gate
  service.rs    persistent inference worker: lazy load, cancel, idle-unload watchdog
  engine.rs     whisper-rs wrapper (AsrBackend trait = swappable ASR seam)
  hotkey.rs     Unix-socket control server ($XDG_RUNTIME_DIR) + client
  inject.rs     arboard clipboard save → paste (wtype/xdotool) → restore
  model_mgr.rs  catalog + verified-size downloads (atomic .part rename)
  settings.rs   JSON settings
  system.rs     session detect, thread sizing, VmRSS/VmHWM reader
  wav.rs        WAV read (PCM16/24/32, f32, any rate) / write (PCM16 16k)
  autostart.rs  XDG autostart entry
```

Threads: UI never blocks. Capture and inference live on their own threads; all results arrive as events. The Whisper context is created once and reused; a watchdog unloads it (`malloc_trim`) after `idle_unload_secs` so idle RSS collapses to ~7 MB.

## Performance

Measured on this machine (12-core, AMD; release build):

| Metric | tiny.en-q5 (default) |
|---|---|
| Idle daemon (model unloaded) | **7 MB** RSS |
| Model load | 0.12 s (lazy, first dictation) |
| Peak RSS during transcription of 11 s clip | **136 MB** |
| Decode 11 s clip, 4 threads | 1.86 s |
| WAV test suite | 9/9 unit tests |

Budget levers (all in code): quantized Q5 weights, per-clip `audio_ctx` sizing (short clips never allocate the 30 s window), `single_segment` + greedy decode, ≤4 inference threads, idle unload, zero windows when idle. Re-measure before switching to `base.en-q5` (est. ~190 MB, still inside) — `small.en` cannot fit 200 MB and is marked over-budget in the UI.

## Packaging

- **AppImage**: stage `usr/bin/whisper-flow`, `.desktop`, icon in an AppDir; run `appimagetool`. See `packaging/appimage/README.md`.
- **.deb**: `dpkg-deb --build` with `packaging/deb` layout; `Depends: libasound2t64 | libpipewire-0.3-0t64`, `Suggests: wtype (Wayland) | xdotool (X11)`.
Models are deliberately not bundled (downloaded on demand, ~32 MB default).

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no whisper-flow daemon is running` | start `whisper-flow run` |
| Overlay not visible on Wayland | add the `whisper-flow` window rule (above) |
| Text copied but not pasted (KDE) | ensure `/dev/uinput` writable (`wtype` needs it); fallback message tells you to press Ctrl+V |
| Nothing heard | check mic in settings window; lower `vad_threshold` |
| Model missing | `whisper-flow models download tiny.en-q5` |
| Hotkey "conflict" | socket approach is conflict-free — the compositor owns the binding |

## Roadmap

Windows port (modules are already OS-seams: hotkey/injection backends), multilingual models, `base`/`small` presets, GPU (CUDA/Vulkan) backend flag, VAC/streaming partials.
