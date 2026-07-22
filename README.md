<p align="center">
  <img src="resources/glossa-logo.png" alt="glossa" height="80">
</p>

<p align="left" style="border-left: 3px solid; padding-left: 12px; margin-left: 12px; font-size: 16px; font-weight: 600; margin-block: 20px">
  <em>Press a hotkey, speak, and drop the transcription straight into whatever field is focused.</em>
</p>

Glossa is a free, open-source dictation app for Windows. It runs quietly in the system tray: press or hold a global hotkey, speak, and your words appear wherever your cursor is.

Under the hood, Glossa records your microphone, sends the audio to your chosen speech-to-text provider, and pastes the result into the active app. It works with Groq, OpenAI, other OpenAI-compatible providers, and self-hosted services.

This repository is the **Windows port** of the [Linux Glossa project](https://github.com/Glaicer/glossa).

## Why Glossa?

- Dictate into almost any Windows app without switching windows
- Choose between push-to-talk and toggle recording modes
- Use multilingual Whisper models through Groq, OpenAI, or a compatible provider
- Optionally polish transcripts with an LLM before they are pasted
- Keep your clipboard intact: Glossa restores its previous contents after pasting
- Start Glossa automatically when you sign in
- Cancel a recording at any time with Esc
- No extra clipboard or keyboard automation tools required

## Before you start

You will need:

- Windows 10 or Windows 11
- A working microphone
- An API key for a speech-to-text provider

For most people, I recommend [Groq](https://groq.com/). Its Whisper models are fast, multilingual, and available with a generous free tier. `whisper-large-v3-turbo` is a good default for everyday dictation.

### Get a Groq API key for STT

1. Create or sign in to your [Groq account](https://console.groq.com/).
2. Open the [Groq API Keys page](https://console.groq.com/keys) and select **Create API Key**.
3. Copy the key and keep it somewhere safe. Treat it like a password.
4. Paste it into the Glossa setup wizard when asked. Glossa stores it as a Windows user environment variable, not as plain text in your config file.

That is all you need for speech-to-text. The same Groq key can also power the optional LLM Enhancer.

### LLM Enhancer

Raw speech-to-text often gets the words right but leaves you with missing punctuation, filler words, or the occasional obvious mistake. The LLM Enhancer cleans up the transcript before Glossa pastes it, while keeping your wording, meaning, and language intact.

It can:

- Add punctuation and capitalization
- Correct clear speech-recognition mistakes
- Remove hesitation words and accidental repetitions
- Preserve spoken questions and commands instead of answering them

When the enhancer is enabled, Glossa sends the transcript text—not your audio—to a separate OpenAI-compatible language model. This adds a small amount of processing time and uses that provider's API quota.

I recommend either:

- **Groq with `openai/gpt-oss-120b`** for a fast, consistent model. You can reuse your Groq API key.
- **OpenRouter with `openrouter/free`** if you want it to choose from [currently available free models](https://openrouter.ai/docs/guides/routing/routers/free-router). The model selected by OpenRouter may vary between requests.

You can configure and enable the enhancer from **Settings → LLM enhancer…**. After that, the **AI enhancer** tray item gives you a quick on/off toggle. Manual configuration examples are included in the [Configuration](#configuration) section below.

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

You can run the setup wizard again at any time:

```powershell
.\glossa.exe setup
```

## Using Glossa

The default shortcut is `Ctrl+Alt+Space` in push-to-talk mode:

1. Hold the shortcut.
2. Speak.
3. Release the shortcut to transcribe and paste.

In toggle mode, press the shortcut once to start recording and again to stop. Press Esc if you want to cancel a recording without transcribing it.

Right-click the tray icon to:

- Change the global shortcut
- Open all settings
- Quickly enable or disable the AI enhancer
- Check for updates
- Exit Glossa

## Updating Glossa

The easiest way to update is to right-click the tray icon and select **Update…**. You can also run:

```powershell
& "$env:LOCALAPPDATA\glossa\glossa.exe" update
```

Glossa downloads the latest release, verifies its checksum, installs it, and restarts the daemon.

## Configuration

Most settings are available from the tray menu, so you normally do not need to edit a file by hand. For advanced changes, edit `%APPDATA%\glossa\config.toml`. See [contrib/examples/config.toml](contrib/examples/config.toml) for the complete reference.

### LLM Enhancer settings

The Settings dialog is the easiest way to configure the enhancer. If you prefer to edit `config.toml`, use one of these presets.

Groq with GPT-OSS 120B:

```toml
[LLM]
enabled = true
base_url = "https://api.groq.com/openai/v1"
model = "openai/gpt-oss-120b"
api_key = "env:GROQ_API_KEY"
```

OpenRouter's free-model router:

```toml
[LLM]
enabled = true
base_url = "https://openrouter.ai/api/v1"
model = "openrouter/free"
api_key = "env:OPENROUTER_API_KEY"
```

Create an OpenRouter key on the [OpenRouter Keys page](https://openrouter.ai/settings/keys). Set it as a Windows user environment variable before using the manual configuration:

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

### CLI control

```toml
[control]
enable_cli = true
socket_path = "auto" # loopback TCP: 127.0.0.1:47823
```

### Logging

```toml
[logging]
level = "warn"
console = true         # legacy alias: journal
```

## Development

```powershell
cargo build
cargo test --workspace
cargo clippy --workspace
cargo run --package glossa-bin -- --config contrib\examples\config.toml daemon
```

When running from the repository, point the `[ui]` icon and sound paths in your config to `contrib\assets\...`.

### Architecture

```text
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

See [AGENTS.md](AGENTS.md) for the full agent-oriented architecture notes.

### Packaging a release

Installed builds include `update.ps1` next to `glossa.exe` under `%LOCALAPPDATA%\glossa`. The updater downloads the versioned `glossa-update.ps1` and `sha256sums.txt` from the latest GitHub release before installing `glossa-windows-x86_64.zip`.

To package a release from a local build:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File build-release-zip.ps1 -Version 0.9.0
```

Upload these files from `target\release\github\<version>\` to a GitHub release:

- `glossa-windows-x86_64.zip`
- `glossa-update.ps1`
- `sha256sums.txt`

## License

MIT — see [LICENSE](LICENSE).
