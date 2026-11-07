# Audio Boost

Small desktop volume tool for Fedora and Windows. Volume goes from 0 % to 300 %.
A look-ahead limiter keeps boosted audio clean instead of crackling.

## How it works
* **Fedora (PipeWire):** creates a virtual output "Audio Boost", sends all system audio
  through it, applies gain + limiter and plays it on your real speakers.
  Needs `pulseaudio-utils` (installed by default). Original volume and default
  output are restored when you turn it off or close the app.
* **Windows:** install the free [VB-Audio Virtual Cable](https://vb-audio.com/Cable/),
  set **CABLE Input** as the default playback device, then pick your real
  speakers inside the app.

## Build
```bash
npm install
npx tauri icon app-icon.png
npx tauri build
```
Installers for both systems are built by the GitHub Actions workflow.

## Font
The UI uses Roca Two. Put `RocaTwo-Regular.woff2` and `RocaTwo-Bold.woff2` in `ui/fonts/`.
