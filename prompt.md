Build a **native Linux voice-typing application inspired by Wispr Flow**, focused on being extremely lightweight, fast, responsive, and easy for anyone to run.

### Core requirements

* Language: **Rust**
* UI: **GPUI (the Rust UI framework used by Zed)**
* OS: Linux
* Architecture: native desktop application
* Speech engine: **whisper.cpp**
* Do NOT use Electron, Python, or a browser-based UI.
* Avoid unnecessary dependencies and background services.

### Main goal

Create a Wispr Flow-style workflow:

**Global hotkey → hold/speak → speech detection → fast transcription → automatically insert text into the currently focused application.**

The application should feel nearly instant and stay in the background with very low idle CPU/RAM usage.

### Speech model

Use Whisper locally/offline.

Design the model system so it can support:

* `base.en` for very low-resource machines
* `small.en` for better English accuracy

Make **`small.en` the default quality model**, but detect available system resources and allow automatic switching to `base.en` when appropriate.

Do not load multiple Whisper models into memory at the same time.

Use the quantized model format where practical to reduce RAM and disk usage.

The application should make model downloading optional and keep the core application itself small.

### Performance requirements

Optimize aggressively for:

* low RAM consumption
* low idle CPU usage
* fast startup
* low transcription latency
* low disk footprint
* efficient multithreading
* CPU SIMD acceleration where available
* GPU acceleration when available, but GPU must NOT be required
* automatic selection of an appropriate number of inference threads
* reuse the loaded Whisper context instead of repeatedly loading the model

Do not spawn a new Whisper process for every transcription.

Use a persistent inference engine.

### Audio pipeline

Implement:

```text
Microphone
   ↓
Audio capture
   ↓
Lightweight VAD / silence detection
   ↓
Audio buffering
   ↓
Whisper inference
   ↓
Text cleanup
   ↓
Text injection
```

Use a lightweight audio capture implementation and avoid unnecessary audio conversions/copies.

Support normal desktop microphones and headset microphones.

### Voice interaction

Implement a global hotkey such as:

**Hold hotkey → record**

**Release → transcribe**

Also provide a toggle mode.

Show a small unobtrusive recording indicator.

The application should continue running in the system tray/background without keeping the main window active.

### Text insertion

After transcription, automatically insert the text into the currently focused application.

Support Linux environments as broadly as practical, including **X11 and Wayland**.

Use a robust abstraction for text injection rather than hard-coding one desktop environment.

Handle applications where clipboard paste is more reliable than simulated keystrokes.

Preserve the user's clipboard contents when using clipboard-based insertion.

### UI

Build the UI entirely with **GPUI**.

Keep it minimal and lightweight.

Include:

* recording status
* microphone selector
* model selector
* language/model status
* transcription preview
* settings
* hotkey configuration
* startup option
* CPU/GPU backend information
* model download/remove controls

Do not create a large dashboard.

The UI should feel similar to a modern developer tool such as Zed: minimal, fast, keyboard-friendly, and native.

### Architecture

Use a clean Rust workspace such as:

```text
whisper-flow-linux/
├── crates/
│   ├── app/
│   ├── ui/
│   ├── audio/
│   ├── vad/
│   ├── transcription/
│   ├── whisper/
│   ├── hotkey/
│   ├── text_injection/
│   ├── settings/
│   └── system/
├── assets/
├── models/
├── Cargo.toml
└── README.md
```

Keep components modular so the Whisper backend can later be replaced with another local ASR engine.

### Whisper integration

Prefer direct library integration through Rust FFI or a maintained Rust binding to **whisper.cpp**.

Do not rely on repeatedly executing the `whisper-cli` command.

Create a persistent transcription service with:

* model initialization
* audio queue
* inference worker
* cancellation support
* result streaming
* error handling
* configurable thread count

Make the API asynchronous so the GPUI interface never blocks during transcription.

### Streaming / latency

Do not wait unnecessarily for large recordings.

Implement short audio chunks and endpoint detection so the application can produce text quickly while still preventing excessive partial-result flicker.

Optimize for **voice typing**, not long-form audio transcription.

### Resource management

Implement an automatic resource profile:

```text
Low Resource
→ base.en

Balanced
→ small.en

Power User
→ allow larger models later
```

Detect RAM, CPU cores, and available GPU backend.

Never consume excessive RAM simply because more memory is available.

Unload optional resources when idle when possible.

### Reliability

Handle:

* microphone disconnects
* permission failures
* model missing
* model download failure
* transcription errors
* hotkey conflicts
* Wayland/X11 differences
* GPU unavailable
* corrupted model files
* application shutdown during recording

The application must fail gracefully rather than freezing.

### Packaging

Provide a production-ready Linux build.

Target at minimum:

* x86_64
* Linux
* Wayland
* X11

Prepare packaging instructions for:

* AppImage
* `.deb`

Make the release easy for a normal Linux user to install.

### Important engineering rules

1. **Performance over visual complexity.**
2. **Native Rust everywhere possible.**
3. **GPUI for the UI.**
4. **whisper.cpp for local transcription.**
5. **No Electron.**
6. **No Python runtime.**
7. **No unnecessary network dependency.**
8. **Offline transcription must work after the model is downloaded.**
9. **Never block the UI thread.**
10. **Benchmark RAM, startup time, CPU usage, and transcription latency.**

### Deliverables

Produce:

1. Complete project architecture.
2. Working Rust implementation.
3. GPUI interface.
4. Whisper integration.
5. Audio capture.
6. VAD/silence detection.
7. Global hotkey.
8. Text injection for Linux.
9. Settings system.
10. Model management.
11. X11 and Wayland handling.
12. AppImage and `.deb` packaging.
13. README with installation and usage.
14. Performance benchmark results.
15. A section explaining how RAM and CPU usage were minimized.

Before finalizing, benchmark **base.en vs small.en** on a normal Linux CPU and use the benchmark results to tune the default configuration.
