# Audio Boost

Small desktop volume tool for Fedora and Windows. Volume goes from 0 % to 300 %.
A look-ahead limiter keeps boosted audio clean instead of crackling.

## Download
Latest builds: [Windows](https://github.com/DevAuto473/audio-control/releases/latest/download/AudioBoost-Windows-Setup.exe) · [Fedora (.rpm)](https://github.com/DevAuto473/audio-control/releases/latest/download/AudioBoost-Fedora.x86_64.rpm)

Fedora, one command:
```bash
curl -L https://github.com/DevAuto473/audio-control/releases/latest/download/AudioBoost-Fedora.x86_64.rpm -o /tmp/ab.rpm && sudo dnf install -y /tmp/ab.rpm
```

## How it works
* **Fedora (PipeWire):** creates a virtual output "Audio Boost", sends all system audio
  through it, applies gain + limiter and plays it on your real speakers.
  Needs `pulseaudio-utils` (installed by default). Original volume and default
  output are restored when you turn it off or close the app.
* **Windows:** install the free [VB-Audio Virtual Cable](https://vb-audio.com/Cable/),
  set **CABLE Input** as the default playback device, then pick your real
  speakers inside the app.

## Start with system
Optional checkbox in the app. When enabled the app starts hidden in the tray at login.

## Language
English or Arabic (right-to-left), chosen inside the app.

## Background mode
Closing the window hides it to the system tray and the engine keeps running
(the UI does no work while hidden, only the lightweight audio loop runs).
Use the tray icon to show the window again, or "Quit" to stop and restore normal audio.
On GNOME (Fedora Workstation) the tray needs the "AppIndicator and KStatusNotifierItem Support" extension:
`sudo dnf install gnome-shell-extension-appindicator`, then enable it.

## Build
```bash
npm install
npx tauri icon app-icon.png
npx tauri build
```
Installers for both systems are built by the GitHub Actions workflow.

## Font
The UI uses Roca Two. Put `RocaTwo-Regular.woff2` and `RocaTwo-Bold.woff2` in `ui/fonts/`.
