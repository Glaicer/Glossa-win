<p align="center">
  <img src="resources/glossa-logo.png" alt="glossa" height="80">
</p>

<p align="left" style="border-left: 3px solid; padding-left: 12px; margin-left: 12px; font-size: 16px; font-weight: 600; margin-block: 20px">
  <em>Press a hotkey, speak, and drop the transcription straight into whatever field is focused</em>
</p>

Glossa for Windows is a free, open source speech-to-text daemon. It records audio from your microphone, sends it to a speech-to-text provider, copies the result to the clipboard, and pastes it into the active window.

It works with Groq, OpenAI, and other OpenAI-compatible speech-to-text providers, including self-hosted ones.

This repository is the **Windows port** of the [Linux Glossa project](https://github.com/Glaicer/glossa).

I personally recommend using [Groq](https://groq.com/) as it offers the following benefits:

- Generous free limits (2,000 requests per day) and low prices (starting at $0.04 per hour) when you need more
- Amazingly fast LPU, capable of processing minutes of audio in seconds
- Whisper V3 Large and Whisper V3 Large Turbo, which support multilingual input out of the box

## Features

- Runs as a background daemon with a system tray icon
- Supports both toggle and push-to-talk modes via a global hotkey
- Works with Groq, OpenAI, and other OpenAI-compatible STT providers (including self-hosted setups)
- Optional OpenAI-compatible LLM enhancer to fix punctuation and obvious transcription mistakes
- Configurable via `%APPDATA%\glossa\config.toml`
- Autostart via Startup folder (`glossa service start`)
- Pastes using standard clipboard shortcuts such as `Ctrl+V`, `Ctrl+Shift+V`, or `Shift+Insert`
- Restores the previous clipboard contents after pasting
- Esc cancels an in-progress recording

## Requirements

- Windows 10 or Windows 11
- A working microphone
- An API key for your STT provider (for example `GROQ_API_KEY`)

No extra clipboard or keystroke tools are required — paste and clipboard use native Windows APIs.

## Install

### Easy install (recommended)

1. Get `glossa.exe` — download the latest release from [GitHub Releases](https://github.com/Glaicer/Glossa-win/releases), or build from source (below).
2. Run it once:

On first run (when no config exists), Glossa opens a **setup wizard** that installs everything for you:

- Copies the binary and tray/sound assets to `%LOCALAPPDATA%\glossa`
- Writes `%APPDATA%\glossa\config.toml`
- Stores your speech-to-text API key in Windows Credential Manager
- Optionally enables start at login
- Optionally starts the daemon immediately

You can also launch the wizard explicitly at any time:

```powershell
.\glossa.exe setup
```

After setup, use the tray icon or hold/press your hotkey (default `Ctrl+Alt+Space`) to dictate. Esc cancels an in-progress recording.

### Build from source

Requires a recent [Rust toolchain](https://rustup.rs/) on Windows.

```powershell
cargo build --release
.\target\release\glossa.exe setup
```

That runs the same wizard as above, using the binary you just built.

Alternatively, copy files without the wizard:

```powershell
powershell -ExecutionPolicy Bypass -File contrib\windows\install.ps1
```

Then set your API key and start the daemon:

```powershell
# Prefer the setup wizard, or set a session env var (daemon migrates it into Credential Manager):
$env:GROQ_API_KEY = "gsk_..."
& "$env:LOCALAPPDATA\glossa\glossa.exe" --config "$env:APPDATA\glossa\config.toml" daemon

# Optional: start at login
& "$env:LOCALAPPDATA\glossa\glossa.exe" service start
```

## CLI commands

| Command | Description |
|---------|-------------|
| `glossa` / `glossa daemon` | Run the daemon; first run opens the setup wizard if no config exists |
| `glossa setup` | Interactive first-run installer (API key, hotkey, assets, autostart) |
| `glossa service start\|stop\|restart` | Manage Startup-folder autostart |
| `glossa ctl toggle` | Toggle recording over IPC |
| `glossa ctl shutdown` | Ask the daemon to shut down |
| `glossa status` | Print the current daemon status |
| `glossa doctor` | Run environment and configuration diagnostics |
| `glossa update` | Check GitHub releases and install the latest update |

## Configuration

Most day-to-day controls are available from the tray menu:

- Change shortcut…
- Settings (full configuration dialog)
- Update…
- AI enhancer (quick toggle)
- Exit

The Settings dialog covers input backend/mode, CLI control, STT provider, paste, theme, and the LLM enhancer. Edit `%APPDATA%\glossa\config.toml` directly for advanced options. Use [contrib/examples/config.toml](contrib/examples/config.toml) as the reference.

### Hotkey

```toml
[input]
backend = "hotkey"          # legacy alias: "portal"
shortcut = "Ctrl+Alt+Space" # also accepts <Ctrl><Alt>space
mode = "push-to-talk"       # or "toggle"
```

### IPC

```toml
[control]
enable_cli = true
socket_path = "auto"   # loopback TCP: 127.0.0.1:47823
```

### Logging

```toml
[logging]
level = "info"
console = true         # legacy alias: journal
```

## Development

```powershell
cargo build
cargo test --workspace
cargo clippy --workspace
cargo run --package glossa-bin -- --config contrib\examples\config.toml daemon
```

When developing from the repo, point the `[ui]` icon/sound paths in your config at `contrib\assets\...`.

## Architecture

```
glossa-bin
  ├── glossa-app          (state machine + port traits)
  │     └── glossa-core
  ├── glossa-audio        (CPAL + rodio + silence trim)
  ├── glossa-stt          (HTTP transcription clients)
  └── glossa-platform-windows
        ├── global hotkeys
        ├── native clipboard + paste
        ├── tray icon
        ├── loopback TCP IPC
        └── doctor / autostart / GitHub-release updater
```

## Updating

Installed builds ship `update.ps1` next to `glossa.exe` under `%LOCALAPPDATA%\glossa`.

- **Tray:** right-click the tray icon → **Update…**
- **CLI:** `glossa update`

The bootstrapper downloads the versioned `glossa-update.ps1` and `sha256sums.txt` from the latest [GitHub release](https://github.com/Glaicer/Glossa-win/releases), verifies the checksum, then installs `glossa-windows-x86_64.zip` (binary, assets, `VERSION`) and restarts the daemon.

To package a release from a local build:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File build-release-zip.ps1 -Version 0.9.0
```

Upload the three files under `target\release\github\<version>\` to a GitHub release:

- `glossa-windows-x86_64.zip`
- `glossa-update.ps1`
- `sha256sums.txt`

See [AGENTS.md](AGENTS.md) for the full agent-oriented architecture notes.

## License

MIT — see [LICENSE](LICENSE).
