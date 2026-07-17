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

## Quick start

### 1. Build

```powershell
cargo build --release
```

The binary is written to `target\release\glossa.exe`.

### 2. Install assets and config

```powershell
$appLocal = Join-Path $env:LOCALAPPDATA "glossa"
$appRoam  = Join-Path $env:APPDATA "glossa"
New-Item -ItemType Directory -Force -Path "$appLocal\assets\tray", "$appLocal\assets\sounds", $appRoam | Out-Null

Copy-Item contrib\assets\tray\*   "$appLocal\assets\tray\"
Copy-Item contrib\assets\sounds\* "$appLocal\assets\sounds\"
Copy-Item target\release\glossa.exe "$appLocal\glossa.exe"

# Create a starter config if none exists
if (-not (Test-Path "$appRoam\config.toml")) {
  $user = $env:USERNAME
  (Get-Content contrib\examples\config.toml) `
    -replace '<user>', $user `
    | Set-Content "$appRoam\config.toml"
}
```

Set your API key in the user environment, or put a literal key in `config.toml`:

```powershell
[System.Environment]::SetEnvironmentVariable("GROQ_API_KEY", "gsk_...", "User")
```

### 3. Run the daemon

```powershell
& "$env:LOCALAPPDATA\glossa\glossa.exe" --config "$env:APPDATA\glossa\config.toml" daemon
```

### 4. Optional: start at login

```powershell
& "$env:LOCALAPPDATA\glossa\glossa.exe" service start
```

## CLI commands

| Command | Description |
|---------|-------------|
| `glossa daemon` | Run the daemon in the foreground (`--config` required) |
| `glossa service start\|stop\|restart` | Manage Startup-folder autostart |
| `glossa ctl toggle` | Toggle recording over IPC |
| `glossa ctl shutdown` | Ask the daemon to shut down |
| `glossa status` | Print the current daemon status |
| `glossa doctor` | Run environment and configuration diagnostics |
| `glossa update` | Run the local updater when installed |

## Configuration

Most day-to-day toggles are available from the tray menu:

- Toggle recording
- AI enhancer
- Append space
- Mode: Toggle / Push-to-talk
- Open config…
- Quit

Edit `%APPDATA%\glossa\config.toml` for provider keys, models, and paths. Use [contrib/examples/config.toml](contrib/examples/config.toml) as the reference.

### Hotkey

```toml
[input]
backend = "hotkey"          # or "portal" (same meaning on Windows)
shortcut = "Ctrl+Alt+Space" # also accepts <Ctrl><Alt>space
mode = "push-to-talk"       # or "toggle"
```

### IPC

```toml
[control]
enable_cli = true
socket_path = "auto"   # resolves to 127.0.0.1:47823
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
        └── doctor / autostart / updater stubs
```

See [AGENTS.md](AGENTS.md) for the full agent-oriented architecture notes.

## License

MIT — see [LICENSE](LICENSE).
